//! Transactions waiting for a block, keyed by sender and nonce so each sender's run in order.

use {
    crate::chain::Fees,
    crate::crypto::verifiable::{Verifiable, VerifyError},
    crate::db::{DbError, State},
    crate::registry::Accounts,
    crate::structs::address::Address,
    crate::structs::tx::SignedTx,
    crate::structs::u256::u256,
    r#macro::document,
    std::cmp::Reverse,
    std::collections::btree_map::Entry,
    std::collections::{BTreeMap, BinaryHeap},
    std::fmt,
};

#[document(
    summary = "Transactions waiting for a block. Admission checks everything that does not change with state, so a selected transaction can only fail at the nonce if another one with the same nonce landed first.",
    category = "Node",
    see_also = "SignedTx, Node",
)]
pub struct Mempool {
    txs: BTreeMap<(Address, u64), SignedTx>,
    capacity: usize,
}

#[document(
    summary = "Why a transaction was not admitted.",
    category = "Node",
)]
#[derive(Debug, PartialEq, Eq)]
pub enum MempoolError {
    #[document(summary = "The transaction, key or signature is not valid.")]
    Invalid(VerifyError),
    #[document(summary = "The sender has already used this nonce.")]
    Stale,
    #[document(summary = "A transaction from this sender with this nonce is already waiting.")]
    Duplicate,
    #[document(summary = "The mempool is full.")]
    Full,
    #[document(summary = "Its gas limit does not cover its intrinsic cost, or is more than a block holds.")]
    BadGasLimit,
    #[document(summary = "Its max price is under the least base price any block can have.")]
    PriceTooLow,
    #[document(summary = "The sender cannot cover its gas limit at its max price.")]
    CannotPay,
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl fmt::Display for MempoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MempoolError::Invalid(err) => write!(f, "{err}"),
            MempoolError::Stale => write!(f, "nonce already used"),
            MempoolError::Duplicate => write!(f, "already waiting"),
            MempoolError::Full => write!(f, "mempool full"),
            MempoolError::BadGasLimit => write!(f, "gas limit under its intrinsic cost, or over the block gas limit"),
            MempoolError::PriceTooLow => write!(f, "max price under the least base price"),
            MempoolError::CannotPay => write!(f, "the sender cannot pay for its gas limit"),
            MempoolError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl From<DbError> for MempoolError {
    fn from(err: DbError) -> MempoolError {
        MempoolError::Db(err)
    }
}

impl Mempool {
    #[document(
        parent = "Mempool",
        summary = "An empty mempool.",
        params = { capacity = "The most transactions it holds." },
    )]
    pub fn new(capacity: usize) -> Mempool {
        Mempool {
            txs: BTreeMap::new(),
            capacity,
        }
    }

    #[document(parent = "Mempool", summary = "How many transactions are waiting.")]
    pub fn len(&self) -> usize {
        self.txs.len()
    }

    #[document(parent = "Mempool", summary = "The transactions waiting, by sender and then nonce.")]
    pub fn iter(&self) -> impl Iterator<Item = &SignedTx> {
        self.txs.values()
    }

    #[document(parent = "Mempool", summary = "Whether nothing is waiting.")]
    pub fn is_empty(&self) -> bool {
        self.txs.is_empty()
    }

    #[document(
        parent = "Mempool",
        summary = "Admits a transaction after checking its signature, that its nonce is not already used, and on a chain with fees, that its gas limit could fit a block, its price could be taken, and its sender can pay for it now.",
        params = {
            tx = "The transaction.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
    )]
    pub fn add(&mut self, tx: SignedTx, state: &State, fees: Option<&Fees>) -> Result<(), MempoolError> {
        if self.txs.len() >= self.capacity {
            return Err(MempoolError::Full);
        }

        let checked = tx.verify();

        self.add_checked(tx, checked, state, fees)
    }

    #[document(
        parent = "Mempool",
        summary = "Admits a transaction whose signature was checked already, such as on the node's verifier thread, going by what that check found. Everything else is checked as add checks it.",
        params = {
            tx = "The transaction.",
            checked = "What its verify returned.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
    )]
    pub fn add_checked(&mut self, tx: SignedTx, checked: Result<(), VerifyError>, state: &State, fees: Option<&Fees>) -> Result<(), MempoolError> {
        if self.txs.len() >= self.capacity {
            return Err(MempoolError::Full);
        }

        checked.map_err(MempoolError::Invalid)?;
        let sender = tx.sender();
        let account = Accounts::new(state).get(&sender)?;

        if tx.nonce < account.nonce {
            return Err(MempoolError::Stale);
        }

        if let Some(fees) = fees {
            let limit = tx.fee.gas_limit;

            if limit < fees.schedule().intrinsic(&tx) || limit > fees.block_gas_limit() {
                return Err(MempoolError::BadGasLimit);
            }

            if tx.fee.max_price < fees.pricing().floor() {
                return Err(MempoolError::PriceTooLow);
            }

            if tx.fee.reserve().is_none_or(|reserve| reserve > account.balance) {
                return Err(MempoolError::CannotPay);
            }
        }

        if self.txs.contains_key(&(sender, tx.nonce)) {
            return Err(MempoolError::Duplicate);
        }

        self.txs.insert((sender, tx.nonce), tx);

        Ok(())
    }

    #[document(
        parent = "Mempool",
        summary = "Picks transactions for a block, the highest priority first: for each sender, their waiting transactions in nonce order from their current nonce, stopping at the first gap; across senders, whichever sender's next transaction pays the producer most, ties going to the lower address.",
        params = {
            state = "The state at the tip.",
            max = "The most to pick.",
            priority = "What a transaction is worth to the producer, such as Chain::priority.",
        },
    )]
    pub fn select(&self, state: &State, max: usize, priority: &dyn Fn(&SignedTx) -> u256) -> Result<Vec<SignedTx>, DbError> {
        let accounts = Accounts::new(state);
        // Each sender's run: the transactions that can go in order from its next nonce.
        let mut runs: Vec<Vec<&SignedTx>> = Vec::new();
        let mut current: Option<(Address, u64)> = None;

        for ((sender, nonce), tx) in &self.txs {
            let next = match current {
                Some((address, next)) if address == *sender => next,
                _ => {
                    runs.push(Vec::new());
                    accounts.get(sender)?.nonce
                }
            };

            if *nonce == next {
                runs.last_mut().expect("pushed for this sender").push(tx);
                current = Some((*sender, next + 1));
            } else {
                // A gap, or a nonce already used: nothing after it can run yet.
                current = Some((*sender, u64::MAX));
            }
        }

        // Each run's next transaction, best first; taking one puts the run's next in its place.
        let mut heads: BinaryHeap<(u256, Reverse<usize>)> = runs
            .iter()
            .enumerate()
            .filter_map(|(run, txs)| txs.first().map(|tx| (priority(tx), Reverse(run))))
            .collect();
        let mut taken = vec![0; runs.len()];
        let mut picked = Vec::new();

        while picked.len() < max
            && let Some((_, Reverse(run))) = heads.pop()
        {
            picked.push(runs[run][taken[run]].clone());
            taken[run] += 1;

            if let Some(next) = runs[run].get(taken[run]) {
                heads.push((priority(next), Reverse(run)));
            }
        }

        Ok(picked)
    }

    #[document(
        parent = "Mempool",
        summary = "Drops transactions whose nonce is already used, after a block landed.",
        params = { state = "The state at the new tip." },
    )]
    pub fn prune(&mut self, state: &State) -> Result<(), DbError> {
        let accounts = Accounts::new(state);
        let mut nonces: BTreeMap<Address, u64> = BTreeMap::new();

        for sender in self.txs.keys().map(|(sender, _)| *sender) {
            if let Entry::Vacant(entry) = nonces.entry(sender) {
                entry.insert(accounts.get(&sender)?.nonce);
            }
        }

        self.txs.retain(|(sender, nonce), _| *nonce >= nonces[sender]);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::DevAuthority;
    use crate::crypto::keypair::Keypair;
    use crate::db::MemoryDb;
    use crate::structs::amount::Amount;
    use crate::chain::FeeConfig;
    use crate::structs::tx::{FeeOffer, Tx};
    use std::sync::Arc;

    fn keypair() -> Keypair {
        DevAuthority::dev_keypair()
    }

    fn transfer(keypair: &Keypair, nonce: u64) -> SignedTx {
        let from = keypair.public().address();
        let tx = Tx::Transfer {
            from,
            to: Keypair::generate(keypair.public().key_type()).unwrap().public().address(),
            amount: Amount::U128(1),
        };

        SignedTx::sign(tx, nonce, FeeOffer::NONE, keypair)
    }

    /// Every transaction worth the same, as on a chain without fees.
    fn free(_: &SignedTx) -> u256 {
        u256::ZERO
    }

    fn state() -> State {
        State::new(Arc::new(MemoryDb::default()))
    }

    #[test]
    fn selection_follows_nonce_order_and_stops_at_gaps() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();

        for nonce in [2, 0, 1, 4] {
            mempool.add(transfer(&keypair, nonce), &state, None).unwrap();
        }

        let nonces: Vec<u64> = mempool.select(&state, 10, &free).unwrap().iter().map(|tx| tx.nonce).collect();

        assert_eq!(nonces, [0, 1, 2]);
        assert_eq!(mempool.select(&state, 2, &free).unwrap().len(), 2);
    }

    #[test]
    fn bad_and_repeated_transactions_are_refused() {
        let state = state();
        let mut mempool = Mempool::new(2);
        let keypair = keypair();
        let mut forged = transfer(&keypair, 0);
        forged.nonce = 1;

        assert!(matches!(mempool.add(forged, &state, None), Err(MempoolError::Invalid(_))));

        mempool.add(transfer(&keypair, 0), &state, None).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &state, None), Err(MempoolError::Duplicate));

        mempool.add(transfer(&keypair, 1), &state, None).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 2), &state, None), Err(MempoolError::Full));
    }

    #[test]
    fn used_nonces_are_stale_and_pruned() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();
        mempool.add(transfer(&keypair, 0), &state, None).unwrap();
        mempool.add(transfer(&keypair, 1), &state, None).unwrap();
        Accounts::new(&state).bump_nonce(&keypair.public().address()).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &state, None), Err(MempoolError::Stale));

        mempool.prune(&state).unwrap();

        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool.select(&state, 10, &free).unwrap()[0].nonce, 1);
    }

    #[test]
    fn with_fees_only_payable_transactions_get_in_and_better_offers_go_first() {
        let state = state();
        let fees = FeeConfig::dev().build();
        let mut mempool = Mempool::new(10);
        let (rich, poor) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());
        Accounts::new(&state).credit(&rich.public().address(), u256::from(10u64).pow(18)).unwrap();
        let floor = fees.pricing().floor();
        let offer = |gas_limit: u64, max_price: u256, tip: u64| FeeOffer {
            gas_limit,
            max_price,
            tip: u256::from(tip),
        };
        let signed = |keypair: &Keypair, nonce: u64, fee: FeeOffer| {
            let tx = Tx::Burn {
                from: keypair.public().address(),
                amount: Amount::U128(1),
            };

            SignedTx::sign(tx, nonce, fee, keypair)
        };
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &state, Some(&fees));

        assert_eq!(add(&mut mempool, signed(&rich, 0, offer(10, floor, 0))), Err(MempoolError::BadGasLimit));
        assert_eq!(add(&mut mempool, signed(&rich, 0, offer(u64::MAX, floor, 0))), Err(MempoolError::BadGasLimit));
        assert_eq!(add(&mut mempool, signed(&rich, 0, offer(100_000, floor - u256::ONE, 0))), Err(MempoolError::PriceTooLow));
        assert_eq!(add(&mut mempool, signed(&poor, 0, offer(100_000, floor, 0))), Err(MempoolError::CannotPay));

        // Two senders: the one whose next transaction tips more goes first, then each in order.
        let other = Keypair::generate(rich.public().key_type()).unwrap();
        Accounts::new(&state).credit(&other.public().address(), u256::from(10u64).pow(18)).unwrap();
        let price = floor * u256::from(100u64);
        add(&mut mempool, signed(&rich, 0, offer(100_000, price, 1))).unwrap();
        add(&mut mempool, signed(&rich, 1, offer(100_000, price, 50))).unwrap();
        add(&mut mempool, signed(&other, 0, offer(100_000, price, 9))).unwrap();
        let base = floor;
        let picked = mempool.select(&state, 10, &|tx| fees.priority(base, &tx.fee)).unwrap();
        let order: Vec<(Address, u64)> = picked.iter().map(|tx| (tx.sender(), tx.nonce)).collect();

        assert_eq!(order, [
            (other.public().address(), 0),
            (rich.public().address(), 0),
            (rich.public().address(), 1),
        ]);
    }
}
