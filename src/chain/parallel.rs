//! Running a block's transactions on every core, with the result they would have one by one.
//!
//! Every transaction runs at once, each on a layer of its own over the block's starting state,
//! which records the keys it reads. Then, in block order, each one's result is kept if no earlier
//! transaction in the block wrote anything it read: what it read is then what it would have read
//! in its turn, so its result is the one it would have had. A transaction that read something an
//! earlier one wrote runs again, in its turn, on what the block holds by then. Either way every
//! transaction ends up with exactly the result running them one by one gives, so this is no part
//! of consensus, only of speed. Transactions that touch nothing in common, such as transfers
//! between different accounts, run side by side; ones that do, such as two from one sender, cost
//! their sender's later ones a second run, and nothing else.
//!
//! Checking signatures is most of what a simple transaction costs, and runs ahead with the rest. A
//! transaction run again does not check its signature again.

use {
    super::execute::{self, BlockContext, ExecuteError, Executed, Rules},
    super::{BlockError, ImportError, Receipt, TxError},
    crate::db::{Changes, Column, Keys, State, address_key},
    crate::registry::{Accounts, RegistryError},
    crate::structs::tx::SignedTx,
    crate::structs::u256::u256,
    crate::vm::Vm,
    r#macro::document,
    rayon::prelude::*,
    std::sync::{Arc, OnceLock},
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
    /// Which had to run again in their turn, having read what an earlier one wrote, by position.
    pub(super) again: Vec<usize>,
}

/// What a block's transactions run with: the chain's rules and engines, and its workers.
pub(super) struct Runner<'a> {
    pub(super) rules: &'a Rules,
    pub(super) vm: &'a Arc<Vm>,
    pub(super) workers: &'a Workers,
}

/// One transaction run ahead of its turn: what came of it, what it wrote and what it read.
struct Ahead {
    outcome: Result<Executed, ExecuteError>,
    changes: Changes,
    reads: Keys,
}

impl Runner<'_> {
    /// Runs transactions in order on a block's layer, on every core, crediting the producer its
    /// share of each fee as soon as each is in. The result is exactly what running them one by
    /// one gives.
    pub(super) fn run(&self, layer: &Arc<State>, txs: &[SignedTx], context: &BlockContext, unfit: Unfit) -> Result<Ran, ImportError> {
        let ahead = self.workers.map(txs, |tx| self.ahead(layer, tx, context));
        let mut ran = Ran::default();
        // Everything the transactions kept so far wrote.
        let mut written = Keys::new();

        for (index, (tx, ahead)) in txs.iter().zip(ahead).enumerate() {
            if let Some(left) = self.gas_left(ran.gas_used)
                && tx.fee.gas_limit > left
            {
                match unfit {
                    Unfit::Refuse => return Err(ImportError::Invalid(BlockError::Tx { index, error: TxError::BlockFull { left } })),
                    Unfit::Skip => continue,
                }
            }

            let (outcome, changes) = match ahead {
                Ahead { outcome: Err(ExecuteError::Db(err)), .. } => return Err(ImportError::Db(err)),
                Ahead { outcome, changes, reads } if reads.is_disjoint(&written) => (outcome, changes),
                _ => {
                    ran.again.push(index);
                    self.again(layer, tx, context)
                }
            };

            match outcome {
                Ok(executed) => {
                    written.extend(changes.keys().cloned());
                    layer.merge(changes);
                    self.pay_producer(layer, context, executed.to_producer, &mut written)?;
                    ran.gas_used += executed.receipt.gas_used;
                    ran.kept.push(index);
                    ran.receipts.push(executed.receipt);
                }
                Err(ExecuteError::Invalid(error)) => match unfit {
                    Unfit::Refuse => return Err(ImportError::Invalid(BlockError::Tx { index, error })),
                    Unfit::Skip => {}
                },
                Err(ExecuteError::Db(err)) => return Err(ImportError::Db(err)),
            }
        }

        Ok(ran)
    }

    /// Runs a transaction ahead of its turn, on the block's starting state, recording its reads.
    fn ahead(&self, layer: &Arc<State>, tx: &SignedTx, context: &BlockContext) -> Ahead {
        let own = Arc::new(State::recording(layer.clone()));
        let outcome = execute::execute(&own, tx, self.rules, self.vm, context);

        Ahead {
            outcome,
            changes: own.take_changes(),
            reads: own.take_reads(),
        }
    }

    /// Runs a transaction again in its turn, on what the block holds now. It passed its checks when
    /// it ran ahead, since a transaction that fails them reads nothing and never runs again.
    fn again(&self, layer: &Arc<State>, tx: &SignedTx, context: &BlockContext) -> (Result<Executed, ExecuteError>, Changes) {
        let turn = Arc::new(State::new(layer.clone()));
        let outcome = execute::execute_verified(&turn, tx, self.rules, self.vm, context);

        (outcome, turn.take_changes())
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

    /// The gas a block has left once its transactions so far used this much, or None when blocks
    /// are unbounded.
    pub(super) fn gas_left(&self, used: u64) -> Option<u64> {
        self.rules.fees.as_ref().map(|fees| fees.block_gas_limit().saturating_sub(used))
    }
}

#[cfg(test)]
mod tests;
