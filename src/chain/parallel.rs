//! Running a block's transactions on every core, with the result they would have one by one.
//!
//! The transactions run in rounds. A transaction waits for the last one before it that names
//! something it names: its sender, its recipient or contract, a deploy's address, staking or
//! signer records, or, under fees, the producer every transaction pays. Every signature is checked
//! at once alongside the first round, since that is most of what a simple transaction costs, so
//! those that wait have theirs checked by the time their turn comes. Each round runs every
//! transaction whose wait is over, side by side, each on a layer of its own over what the block
//! holds so far, which records the keys it reads. Then results go in in block order for as long as
//! they hold, which ends some waits for the next round.
//!
//! What a transaction names is a guess at what it touches: a contract can reach accounts and
//! contracts nobody named. So a result only goes in if nothing it read was written by a
//! transaction that went in after it ran. One that was runs again, the only kind that ever runs
//! twice. Either way every transaction ends up with exactly the result running them one by one
//! gives, so none of this is consensus, only speed. A run of transactions that depend on each
//! other runs one at a time, as it must, and everything beside it runs alongside.

use {
    super::execute::{self, BlockContext, ExecuteError, Executed, Rules},
    super::{BlockError, ImportError, Receipt, TxError},
    crate::crypto::verifiable::VerifyError,
    crate::db::{Changes, Column, Key, Keys, State, address_key},
    crate::registry::contracts::contract_address,
    crate::chain::REWARD_POOL,
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::hash::HashType,
    crate::structs::tx::{SignedTx, Tx},
    crate::structs::u256::u256,
    crate::vm::Vm,
    r#macro::document,
    rayon::prelude::*,
    std::collections::{BTreeMap, BTreeSet, HashMap},
    std::sync::{Arc, OnceLock},
    std::time::Instant,
};

#[document(
    summary = "The threads a chain runs a block's transactions and checks signatures on, as many as the machine has cores. Owned by the chain, never global, and started the first time there is more than one thing to share out.",
    category = "Chain",
    see_also = "Chain",
)]
#[derive(Default)]
pub struct Workers {
    /// None when the threads could not start, so everything runs on the caller's thread.
    pool: OnceLock<Option<rayon::ThreadPool>>,
}

impl Workers {
    #[document(parent = "Workers", summary = "Workers that start on their first use.")]
    pub fn new() -> Workers {
        Workers::default()
    }

    #[document(
        parent = "Workers",
        summary = "Maps items on the workers, keeping their order. One item, or workers that cannot start, are mapped on this thread instead.",
    )]
    pub fn map<T, R, F>(&self, items: &[T], f: F) -> Vec<R>
    where
        T: Sync,
        R: Send,
        F: Fn(&T) -> R + Sync + Send,
    {
        match items.len() > 1 {
            true => match self.pool() {
                Some(pool) => pool.install(|| items.par_iter().map(f).collect()),
                None => items.iter().map(f).collect(),
            },
            false => items.iter().map(f).collect(),
        }
    }

    fn pool(&self) -> Option<&rayon::ThreadPool> {
        self.pool
            .get_or_init(|| {
                rayon::ThreadPoolBuilder::new()
                    .thread_name(|index| format!("exec {index}"))
                    .build()
                    .ok()
            })
            .as_ref()
    }
}

/// What to do with a transaction that cannot run or does not fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unfit {
    /// The block holding it is invalid: what importing a block does.
    Refuse,
    /// Leave it out: what building a block does.
    Skip,
}

/// What a block's transactions came to.
#[derive(Debug, Default)]
pub(super) struct Ran {
    /// Which of the transactions it was given went in, by position.
    pub(super) kept: Vec<usize>,
    /// Their receipts, in order.
    pub(super) receipts: Vec<Receipt>,
    /// The gas they used together.
    pub(super) gas_used: u64,
    /// Which ran more than once, by position: those that touched what no transaction before them
    /// named, and read what one of those wrote.
    pub(super) again: Vec<usize>,
    /// Whether the deadline cut the block short: the transactions after the last kept one were
    /// left out unrun, not because they could not run.
    pub(super) cut: bool,
}

/// What a block's transactions run with: the chain's rules and engines, and its workers.
pub(super) struct Runner<'a> {
    pub(super) rules: &'a Rules,
    pub(super) vm: &'a Arc<Vm>,
    pub(super) workers: &'a Workers,
}

/// One run of a transaction: what came of it, what it wrote and what it read.
struct Attempt {
    outcome: Result<Executed, ExecuteError>,
    changes: Changes,
    reads: Keys,
}

/// What a transaction names, and so is expected to touch: accounts and contracts by address, and
/// the records every staking or signer transaction shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Touch {
    Address(Address),
    Staking,
    Signers,
    Governance,
}

fn touches(tx: &SignedTx, hash: HashType) -> Vec<Touch> {
    let sender = tx.sender();
    let mut touched = vec![Touch::Address(sender)];

    match &tx.tx {
        Tx::Transfer { to, .. } | Tx::Mint { to, .. } | Tx::ContractCall { to, .. } => touched.push(Touch::Address(*to)),
        Tx::Deploy { .. } => touched.push(Touch::Address(contract_address(&sender, tx.nonce, hash))),
        Tx::Upgrade { contract, .. } => touched.push(Touch::Address(*contract)),
        Tx::Burn { .. } => {}
        Tx::Bond { .. } | Tx::Unbond { .. } | Tx::Withdraw { .. } | Tx::Slash { .. } => touched.push(Touch::Staking),
        Tx::ApproveSigners { .. } | Tx::WithdrawApproval { .. } => touched.push(Touch::Signers),
        Tx::ApproveChange { .. } | Tx::WithdrawChange { .. } => touched.push(Touch::Governance),
    }

    touched
}

/// For each transaction, the last one before it that names something it names, which it waits
/// for. Every transaction pays the producer under fees, so one naming the producer waits for the
/// one right before it, and with it for all of them.
fn waits(txs: &[SignedTx], hash: HashType, paid: &[Address]) -> Vec<Option<usize>> {
    let mut last: HashMap<Touch, usize> = HashMap::new();

    txs.iter()
        .enumerate()
        .map(|(index, tx)| {
            let touched = touches(tx, hash);
            // The producer, and the reward pool on a chain that keeps one, are credited after
            // every transaction: one that names them waits for the one before it.
            let mut wait = match paid.iter().any(|account| touched.contains(&Touch::Address(*account))) {
                true => index.checked_sub(1),
                false => None,
            };

            for touch in touched {
                wait = wait.max(last.insert(touch, index));
            }

            wait
        })
        .collect()
}

/// Where each of a block's transactions stands in its rounds.
struct Schedule {
    /// Each transaction's latest attempt, numbered so stale records of older ones are told apart.
    attempts: Vec<Option<(Attempt, u64)>>,
    /// Whether a transaction's attempt read what one that went in since wrote.
    stale: Vec<bool>,
    /// Which attempts read each key, so a transaction going in can tell which it spoiled.
    readers: BTreeMap<Key, Vec<(usize, u64)>>,
    /// The transactions waiting for each one.
    waiting: Vec<Vec<usize>>,
    /// What runs next round.
    runnable: BTreeSet<usize>,
    /// Which have run before.
    ran: Vec<bool>,
    next: u64,
}

impl Schedule {
    fn new(waits: Vec<Option<usize>>) -> Schedule {
        let count = waits.len();
        let mut waiting = vec![Vec::new(); count];
        let mut runnable = BTreeSet::new();

        for (index, wait) in waits.into_iter().enumerate() {
            match wait {
                Some(before) => waiting[before].push(index),
                None => {
                    runnable.insert(index);
                }
            }
        }

        Schedule {
            attempts: (0..count).map(|_| None).collect(),
            stale: vec![false; count],
            readers: BTreeMap::new(),
            waiting,
            runnable,
            ran: vec![false; count],
            next: 0,
        }
    }

    /// What runs this round. The transaction whose turn it is always does, when it has no result
    /// to go in with, so every round lets at least one go in.
    fn round(&mut self, turn: usize) -> Vec<usize> {
        if self.attempts[turn].is_none() || self.stale[turn] {
            self.runnable.insert(turn);
        }

        std::mem::take(&mut self.runnable).into_iter().collect()
    }

    /// Keeps an attempt. Returns whether the transaction had run before.
    fn record(&mut self, index: usize, attempt: Attempt) -> bool {
        let id = self.next;
        self.next += 1;

        for key in &attempt.reads {
            self.readers.entry(key.clone()).or_default().push((index, id));
        }

        self.attempts[index] = Some((attempt, id));
        self.stale[index] = false;

        std::mem::replace(&mut self.ran[index], true)
    }

    /// The attempt the transaction whose turn it is goes in with, if it still holds.
    fn take(&mut self, turn: usize) -> Option<Attempt> {
        match self.stale[turn] {
            true => None,
            false => self.attempts[turn].take().map(|(attempt, _)| attempt),
        }
    }

    /// Notes that a transaction went in, or was left out, having written these keys: attempts
    /// after it that read one run again, and those waiting for it may run.
    fn done(&mut self, turn: usize, written: &Keys) {
        for key in written {
            for (reader, id) in self.readers.remove(key).unwrap_or_default() {
                let current = self.attempts[reader].as_ref().is_some_and(|(_, current)| *current == id);

                if reader > turn && current && !self.stale[reader] {
                    self.stale[reader] = true;
                    self.runnable.insert(reader);
                }
            }
        }

        self.runnable.extend(std::mem::take(&mut self.waiting[turn]));
    }
}

impl Runner<'_> {
    /// Runs transactions in order on a block's layer, on every core, crediting the producer its
    /// share of each fee as soon as each is in. The result is exactly what running them one by
    /// one gives. A deadline, which only building a block sets, stops it once passed: what went
    /// in by then is the block, and the rest is left out unrun, with Ran.cut set.
    pub(super) fn run(
        &self,
        layer: &Arc<State>,
        txs: &[SignedTx],
        context: &BlockContext,
        unfit: Unfit,
        deadline: Option<Instant>,
    ) -> Result<Ran, ImportError> {
        debug_assert!(deadline.is_none() || unfit == Unfit::Skip, "importing runs every transaction");

        let mut paid = Vec::new();

        if self.rules.fees.is_some() {
            paid.push(context.producer);

            if self.rules.pooled_fees {
                paid.push(REWARD_POOL);
            }
        }

        let mut schedule = Schedule::new(waits(txs, self.rules.hash, &paid));
        let mut checked: Vec<Result<(), VerifyError>> = Vec::with_capacity(txs.len());
        let mut ran = Ran::default();
        let mut turn = 0;

        'rounds: while turn < txs.len() {
            let round = schedule.round(turn);

            if checked.is_empty() {
                // The first round checks every signature at once, and runs those with nothing to
                // wait for in the same go.
                let mut now = vec![false; txs.len()];
                round.iter().for_each(|&index| now[index] = true);
                let every: Vec<usize> = (0..txs.len()).collect();
                let first = self.workers.map(&every, |&index| {
                    let check = txs[index].verify_on(&context.genesis);

                    (check, now[index].then(|| self.attempt(layer, &txs[index], check, context)))
                });

                for (index, (check, attempt)) in first.into_iter().enumerate() {
                    checked.push(check);

                    if let Some(attempt) = attempt {
                        schedule.record(index, attempt);
                    }
                }
            } else {
                let attempts = self.workers.map(&round, |&index| self.attempt(layer, &txs[index], checked[index], context));

                for (index, attempt) in round.into_iter().zip(attempts) {
                    if schedule.record(index, attempt) {
                        ran.again.push(index);
                    }
                }
            }

            // In block order, for as long as results hold.
            while let Some(attempt) = (turn < txs.len()).then(|| schedule.take(turn)).flatten() {
                let mut written = Keys::new();

                match self.gas_left(ran.gas_used) {
                    Some(left) if txs[turn].fee.gas_limit > left => {
                        if unfit == Unfit::Refuse {
                            return Err(ImportError::Invalid(BlockError::Tx { index: turn, error: TxError::BlockFull { left } }));
                        }
                    }
                    _ => match attempt.outcome {
                        Ok(executed) => {
                            written.extend(attempt.changes.keys().cloned());
                            layer.merge(attempt.changes);
                            self.pay_producer(layer, context, executed.to_producer, &mut written)?;
                            self.pool_burned(layer, executed.burned, &mut written)?;
                            ran.gas_used += executed.receipt.gas_used;
                            ran.kept.push(turn);
                            ran.receipts.push(executed.receipt);
                        }
                        Err(ExecuteError::Invalid(error)) => {
                            if unfit == Unfit::Refuse {
                                return Err(ImportError::Invalid(BlockError::Tx { index: turn, error }));
                            }
                        }
                        Err(ExecuteError::Db(err)) => return Err(ImportError::Db(err)),
                    },
                }

                schedule.done(turn, &written);
                turn += 1;

                if deadline.is_some_and(|deadline| Instant::now() >= deadline) && turn < txs.len() {
                    ran.cut = true;

                    break 'rounds;
                }
            }
        }

        ran.again.sort_unstable();
        ran.again.dedup();

        Ok(ran)
    }

    /// Runs a transaction on what the block holds now, recording what it reads. One that failed
    /// its checks does not run at all.
    fn attempt(&self, layer: &Arc<State>, tx: &SignedTx, checked: Result<(), VerifyError>, context: &BlockContext) -> Attempt {
        if let Err(err) = checked {
            return Attempt {
                outcome: Err(ExecuteError::Invalid(TxError::Invalid(err))),
                changes: Changes::new(),
                reads: Keys::new(),
            };
        }

        let own = Arc::new(State::recording(layer.clone()));
        let outcome = execute::execute_verified(&own, tx, self.rules, self.vm, context);

        Attempt {
            outcome,
            changes: own.take_changes(),
            reads: own.take_reads(),
        }
    }

    /// Credits the producer its share of a transaction's fee, as a write later transactions may
    /// have read.
    fn pay_producer(&self, layer: &Arc<State>, context: &BlockContext, amount: u256, written: &mut Keys) -> Result<(), ImportError> {
        if amount == u256::ZERO {
            return Ok(());
        }

        Accounts::new(layer).credit(&context.producer, amount).map_err(|err| match err {
            RegistryError::Db(err) => ImportError::Db(err),
            other => ImportError::Invalid(BlockError::Fee(other.to_string())),
        })?;
        written.insert((Column::Accounts, address_key(&context.producer)));

        Ok(())
    }

    /// Puts the burned share of a fee in the reward pool, on a chain that keeps one, as a write
    /// later transactions may have read.
    fn pool_burned(&self, layer: &Arc<State>, amount: u256, written: &mut Keys) -> Result<(), ImportError> {
        if !self.rules.pooled_fees || amount == u256::ZERO {
            return Ok(());
        }

        Accounts::new(layer).credit(&REWARD_POOL, amount).map_err(|err| match err {
            RegistryError::Db(err) => ImportError::Db(err),
            other => ImportError::Invalid(BlockError::Fee(other.to_string())),
        })?;
        written.insert((Column::Accounts, address_key(&REWARD_POOL)));

        Ok(())
    }

    /// The gas a block has left once its transactions so far used this much, or None when blocks
    /// are unbounded.
    pub(super) fn gas_left(&self, used: u64) -> Option<u64> {
        self.rules.fees.as_ref().map(|fees| fees.block_gas_limit().saturating_sub(used))
    }
}

#[cfg(test)]
mod tests;
