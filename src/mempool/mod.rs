//! Transactions waiting for a block, keyed by sender and nonce so each sender's run in order.

use {
    crate::chain::{Fees, tx_bytes},
    crate::crypto::verifiable::VerifyError,
    crate::db::{DbError, State},
    crate::registry::Accounts,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
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
    txs: BTreeMap<(Address, u64), Waiting>,
    capacity: usize,
}

/// A transaction waiting for a block, and how long it has waited unable to go in.
struct Waiting {
    tx: SignedTx,
    /// Blocks in a row whose base price its offer was under, so a producer could not take it.
    underpriced: u32,
}

#[document(
    summary = "How many blocks in a row a transaction may offer less than the base price before the mempool drops it: 32, about a minute of 2-second blocks, long enough for a base price that just rose to come back down.",
    category = "Node",
    see_also = "Mempool::prune",
)]
pub const MAX_UNDERPRICED_BLOCKS: u32 = 32;

#[document(
    summary = "How much a producer takes from the mempool for one block: as many transactions as fit every bound it has. With none, it takes everything that can run.",
    category = "Node",
    see_also = "Mempool::select",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Budget {
    #[document(summary = "The most transactions, when the node caps them with --max-block-txs.")]
    pub count: Option<usize>,
    #[document(summary = "The most bytes they may take, encoded: genesis's max_block_bytes, which blocks are checked against.")]
    pub bytes: Option<u64>,
    #[document(summary = "The most their gas limits may add up to. Twice the block gas limit lets running fill the block however little of their limits they use, without running far more than fits.")]
    pub gas: Option<u64>,
}

impl Budget {
    /// Whether a transaction of this size and gas limit still fits, after what is taken so far.
    fn fits(&self, count: usize, bytes: u64, gas: u64) -> bool {
        self.count.is_none_or(|most| count <= most) && self.bytes.is_none_or(|most| bytes <= most) && self.gas.is_none_or(|most| gas <= most)
    }
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
    #[document(summary = "The node is still taking its state from a snapshot, so it cannot check transactions yet.")]
    Syncing,
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
            MempoolError::Syncing => write!(f, "the node is still syncing its state from a snapshot"),
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
        self.txs.values().map(|waiting| &waiting.tx)
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
            genesis = "The chain's genesis id, which its signature is checked on.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
    )]
    pub fn add(&mut self, tx: SignedTx, genesis: &Hash, state: &State, fees: Option<&Fees>) -> Result<(), MempoolError> {
        if self.txs.len() >= self.capacity {
            return Err(MempoolError::Full);
        }

        let checked = tx.verify_on(genesis);

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

        Mempool::check(&tx, checked, state, fees)?;

        self.insert(tx)
    }

    #[document(
        parent = "Mempool",
        summary = "Checks what admission checks that does not depend on what else waits: the signature, going by what checking it found; that its nonce is not used; and on a chain with fees, that its gas limit could fit a block, its price could be taken and its sender can pay for it now. It needs no mempool, so a node checks off its loop and locks the mempool only to insert.",
        params = {
            tx = "The transaction.",
            checked = "What its verify returned.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
        see_also = "Mempool::insert",
    )]
    pub fn check(tx: &SignedTx, checked: Result<(), VerifyError>, state: &State, fees: Option<&Fees>) -> Result<(), MempoolError> {
        checked.map_err(MempoolError::Invalid)?;
        let account = Accounts::new(state).get(&tx.sender())?;

        if tx.nonce < account.nonce {
            return Err(MempoolError::Stale);
        }

        if let Some(fees) = fees {
            let limit = tx.fee.gas_limit;

            if limit < fees.schedule().intrinsic(tx) || limit > fees.block_gas_limit() {
                return Err(MempoolError::BadGasLimit);
            }

            if tx.fee.max_price < fees.pricing().floor() {
                return Err(MempoolError::PriceTooLow);
            }

            if tx.fee.reserve().is_none_or(|reserve| reserve > account.balance) {
                return Err(MempoolError::CannotPay);
            }
        }

        Ok(())
    }

    #[document(
        parent = "Mempool",
        summary = "Puts in a transaction that check passed, if there is room and nothing from its sender with its nonce waits already.",
        see_also = "Mempool::check",
    )]
    pub fn insert(&mut self, tx: SignedTx) -> Result<(), MempoolError> {
        if self.txs.len() >= self.capacity {
            return Err(MempoolError::Full);
        }

        match self.txs.entry((tx.sender(), tx.nonce)) {
            Entry::Occupied(_) => Err(MempoolError::Duplicate),
            Entry::Vacant(entry) => {
                entry.insert(Waiting { tx, underpriced: 0 });

                Ok(())
            }
        }
    }

    #[document(
        parent = "Mempool",
        summary = "Picks transactions for a block, the highest priority first: for each sender, their waiting transactions in nonce order from their current nonce, stopping at the first gap or at the first one the block cannot take; across senders, whichever sender's next transaction pays the producer most, ties going to the lower address. A transaction that would pass the budget is left out with the rest of its sender's, and the others still go on filling it.",
        params = {
            state = "The state at the tip.",
            budget = "How much to pick: a count, bytes and gas limits, each if bounded.",
            worth = "What a transaction is worth to the producer, or None for one the next block cannot take, as its offer is under the base price: Chain::worth.",
        },
    )]
    pub fn select(&self, state: &State, budget: Budget, worth: &dyn Fn(&SignedTx) -> Option<u256>) -> Result<Vec<SignedTx>, DbError> {
        let accounts = Accounts::new(state);
        // Each sender's run: the transactions that can go in order from its next nonce, each with
        // what it is worth.
        let mut runs: Vec<Vec<(&SignedTx, u256)>> = Vec::new();
        let mut current: Option<(Address, u64)> = None;

        for ((sender, nonce), waiting) in &self.txs {
            let next = match current {
                Some((address, next)) if address == *sender => next,
                _ => {
                    runs.push(Vec::new());
                    accounts.get(sender)?.nonce
                }
            };

            match (*nonce == next, worth(&waiting.tx)) {
                (true, Some(priority)) => {
                    runs.last_mut().expect("pushed for this sender").push((&waiting.tx, priority));
                    current = Some((*sender, next + 1));
                }
                // A gap, a nonce already used, or one the block cannot take: nothing after it
                // can go yet.
                _ => current = Some((*sender, u64::MAX)),
            }
        }

        // Each run's next transaction, best first; taking one puts the run's next in its place.
        let mut heads: BinaryHeap<(u256, Reverse<usize>)> = runs
            .iter()
            .enumerate()
            .filter_map(|(run, txs)| txs.first().map(|(_, priority)| (*priority, Reverse(run))))
            .collect();
        let mut taken = vec![0; runs.len()];
        let mut picked = Vec::new();
        let (mut bytes, mut gas) = (0u64, 0u64);

        while budget.count.is_none_or(|most| picked.len() < most)
            && let Some((_, Reverse(run))) = heads.pop()
        {
            let (tx, _) = runs[run][taken[run]];
            let (more_bytes, more_gas) = (bytes + tx_bytes(tx), gas.saturating_add(tx.fee.gas_limit));

            // Past the budget: the sender's later ones cannot go without it, so its run ends here.
            if !budget.fits(picked.len() + 1, more_bytes, more_gas) {
                continue;
            }

            (bytes, gas) = (more_bytes, more_gas);
            picked.push(tx.clone());
            taken[run] += 1;

            if let Some((_, priority)) = runs[run].get(taken[run]) {
                heads.push((*priority, Reverse(run)));
            }
        }

        Ok(picked)
    }

    #[document(
        parent = "Mempool",
        summary = "Brings the mempool in line with a new tip: drops transactions whose nonce is used, and on a chain with fees, those that can no longer pay, as their sender's funds went since they were admitted, and those that have offered less than the base price for MAX_UNDERPRICED_BLOCKS blocks in a row. An offer under the base price is kept until then, since the base price moves a step a block and may come back down.",
        params = {
            state = "The state at the new tip.",
            fees = "The chain's fees, if it has any.",
            base = "The base price of the next block: Chain::next_base_price.",
        },
        returns = "How many were dropped for not paying: for a sender that cannot pay, or an offer under the base price for too long.",
    )]
    pub fn prune(&mut self, state: &State, fees: Option<&Fees>, base: u256) -> Result<usize, DbError> {
        let accounts = Accounts::new(state);
        let mut senders: BTreeMap<Address, (u64, u256)> = BTreeMap::new();

        for sender in self.txs.keys().map(|(sender, _)| *sender) {
            if let Entry::Vacant(entry) = senders.entry(sender) {
                let account = accounts.get(&sender)?;
                entry.insert((account.nonce, account.balance));
            }
        }

        self.txs.retain(|(sender, nonce), _| *nonce >= senders[sender].0);

        let Some(fees) = fees else {
            return Ok(0);
        };
        let before = self.txs.len();

        self.txs.retain(|(sender, _), waiting| {
            if waiting.tx.fee.reserve().is_none_or(|reserve| reserve > senders[sender].1) {
                return false;
            }

            waiting.underpriced = match fees.pricing().price(base, &waiting.tx.fee) {
                Some(_) => 0,
                None => waiting.underpriced + 1,
            };

            waiting.underpriced <= MAX_UNDERPRICED_BLOCKS
        });

        Ok(before - self.txs.len())
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

    /// The chain the tests' transactions are checked on.
    fn genesis() -> Hash {
        Hash::zero(crate::structs::hash::HashType::default_chain())
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
    fn free(_: &SignedTx) -> Option<u256> {
        Some(u256::ZERO)
    }

    fn state() -> State {
        State::new(Arc::new(MemoryDb::default()))
    }

    /// A transaction with this gas limit and this many bytes of data riding along.
    fn sized(keypair: &Keypair, nonce: u64, gas_limit: u64, data: usize) -> SignedTx {
        let tx = Tx::ContractCall {
            to: crate::structs::address::Address::Contract([3; 32]),
            func: crate::structs::hash::Hash::Xxh3_64(1),
            amount: Amount::U128(0),
            data: vec![crate::structs::tx::TxCallArgumentType::Bytes(vec![0; data])],
        };
        let fee = FeeOffer { gas_limit, ..FeeOffer::NONE };

        SignedTx::sign(tx, nonce, fee, keypair)
    }

    #[test]
    fn selection_stops_at_each_bound_of_its_budget() {
        let state = state();
        let mut mempool = Mempool::new(100);
        let (one, two) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());

        for nonce in 0..5 {
            mempool.add(sized(&one, nonce, 1_000, 100), &genesis(), &state, None).unwrap();
            mempool.add(sized(&two, nonce, 1_000, 100), &genesis(), &state, None).unwrap();
        }

        let size = tx_bytes(&sized(&one, 0, 1_000, 100));
        let picked = |budget: Budget| mempool.select(&state, budget, &free).unwrap().len();

        assert_eq!(picked(Budget::default()), 10, "no bound: everything that can run");
        assert_eq!(picked(Budget { count: Some(3), ..Budget::default() }), 3);
        assert_eq!(picked(Budget { bytes: Some(size * 4 + size / 2), ..Budget::default() }), 4);
        assert_eq!(picked(Budget { gas: Some(6_500), ..Budget::default() }), 6);
        assert_eq!(picked(Budget { count: Some(8), bytes: Some(size * 7), gas: Some(9_000) }), 7, "the tightest wins");
    }

    #[test]
    fn a_transaction_past_the_budget_ends_its_senders_run_and_others_fill_in() {
        let state = state();
        let mut mempool = Mempool::new(100);
        let (big, small) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());
        // The big sender's first is worth the most, but does not fit; its second cannot go alone.
        mempool.add(sized(&big, 0, 9_000, 100), &genesis(), &state, None).unwrap();
        mempool.add(sized(&big, 1, 10, 100), &genesis(), &state, None).unwrap();

        for nonce in 0..4 {
            mempool.add(sized(&small, nonce, 1_000, 100), &genesis(), &state, None).unwrap();
        }

        let gas_limit = |tx: &SignedTx| Some(u256::from(tx.fee.gas_limit));
        let picked = mempool.select(&state, Budget { gas: Some(5_000), ..Budget::default() }, &gas_limit).unwrap();

        assert_eq!(picked.iter().map(|tx| (tx.sender() == small.public().address(), tx.nonce)).collect::<Vec<_>>(), [(true, 0), (true, 1), (true, 2), (true, 3)]);
    }

    #[test]
    fn checking_needs_no_mempool_and_inserting_refuses_duplicates_and_a_full_one() {
        let state = state();
        let keypair = keypair();
        let tx = transfer(&keypair, 0);

        assert_eq!(Mempool::check(&tx, Ok(()), &state, None), Ok(()));
        assert!(matches!(Mempool::check(&tx, Err(VerifyError::InvalidSignature), &state, None), Err(MempoolError::Invalid(_))));

        let mut mempool = Mempool::new(1);

        assert_eq!(mempool.insert(tx.clone()), Ok(()));
        assert_eq!(mempool.insert(tx), Err(MempoolError::Full));

        let mut roomy = Mempool::new(5);
        let again = transfer(&keypair, 3);
        roomy.insert(again.clone()).unwrap();

        assert_eq!(roomy.insert(again), Err(MempoolError::Duplicate));
    }

    #[test]
    fn selection_follows_nonce_order_and_stops_at_gaps() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();

        for nonce in [2, 0, 1, 4] {
            mempool.add(transfer(&keypair, nonce), &genesis(), &state, None).unwrap();
        }

        let nonces: Vec<u64> = mempool.select(&state, Budget { count: Some(10), ..Budget::default() }, &free).unwrap().iter().map(|tx| tx.nonce).collect();

        assert_eq!(nonces, [0, 1, 2]);
        assert_eq!(mempool.select(&state, Budget { count: Some(2), ..Budget::default() }, &free).unwrap().len(), 2);
    }

    #[test]
    fn bad_and_repeated_transactions_are_refused() {
        let state = state();
        let mut mempool = Mempool::new(2);
        let keypair = keypair();
        let mut forged = transfer(&keypair, 0);
        forged.nonce = 1;

        assert!(matches!(mempool.add(forged, &genesis(), &state, None), Err(MempoolError::Invalid(_))));

        mempool.add(transfer(&keypair, 0), &genesis(), &state, None).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &genesis(), &state, None), Err(MempoolError::Duplicate));

        mempool.add(transfer(&keypair, 1), &genesis(), &state, None).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 2), &genesis(), &state, None), Err(MempoolError::Full));
    }

    #[test]
    fn used_nonces_are_stale_and_pruned() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();
        mempool.add(transfer(&keypair, 0), &genesis(), &state, None).unwrap();
        mempool.add(transfer(&keypair, 1), &genesis(), &state, None).unwrap();
        Accounts::new(&state).bump_nonce(&keypair.public().address()).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &genesis(), &state, None), Err(MempoolError::Stale));

        assert_eq!(mempool.prune(&state, None, u256::ZERO).unwrap(), 0);
        assert_eq!(mempool.len(), 1);
        assert_eq!(mempool.select(&state, Budget { count: Some(10), ..Budget::default() }, &free).unwrap()[0].nonce, 1);
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
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees));

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
        let picked = mempool.select(&state, Budget { count: Some(10), ..Budget::default() }, &|tx| Some(fees.priority(base, &tx.fee))).unwrap();
        let order: Vec<(Address, u64)> = picked.iter().map(|tx| (tx.sender(), tx.nonce)).collect();

        assert_eq!(order, [
            (other.public().address(), 0),
            (rich.public().address(), 0),
            (rich.public().address(), 1),
        ]);
    }

    #[test]
    fn what_can_no_longer_pay_is_pruned_and_what_is_underpriced_waits_a_while() {
        let state = state();
        let fees = FeeConfig::dev().build();
        let mut mempool = Mempool::new(10);
        let (rich, broke) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());
        let floor = fees.pricing().floor();
        let coin = u256::from(10u64).pow(18);
        Accounts::new(&state).credit(&rich.public().address(), coin).unwrap();
        Accounts::new(&state).credit(&broke.public().address(), coin).unwrap();
        let burn = |keypair: &Keypair, nonce: u64, max_price: u256| {
            let tx = Tx::Burn {
                from: keypair.public().address(),
                amount: Amount::U128(1),
            };
            let fee = FeeOffer { gas_limit: 100_000, max_price, tip: u256::ZERO };

            SignedTx::sign(tx, nonce, fee, keypair)
        };
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees)).unwrap();
        add(&mut mempool, burn(&rich, 0, floor * u256::from(4u64)));
        add(&mut mempool, burn(&rich, 1, floor));
        add(&mut mempool, burn(&broke, 0, floor * u256::from(4u64)));

        // The base price is twice the floor: rich's second cannot go in the next block, and
        // selection stops its run there rather than hand the builder what it would skip.
        let base = floor * u256::from(2u64);
        let worth = |tx: &SignedTx| fees.pricing().price(base, &tx.fee).map(|_| fees.priority(base, &tx.fee));
        let picked = mempool.select(&state, Budget::default(), &worth).unwrap();
        let mut order: Vec<(Address, u64)> = picked.iter().map(|tx| (tx.sender(), tx.nonce)).collect();
        order.sort();
        let mut expected = [(broke.public().address(), 0), (rich.public().address(), 0)];
        expected.sort();

        assert_eq!(order, expected, "both tip nothing, so either may go first");

        // Broke's funds go: the next prune drops its transaction. Rich's underpriced one stays.
        let debit = Accounts::new(&state).get(&broke.public().address()).unwrap().balance;
        Accounts::new(&state).debit(&broke.public().address(), debit).unwrap();

        assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 1);
        assert_eq!(mempool.len(), 2);

        // Underpriced for MAX_UNDERPRICED_BLOCKS blocks, it still waits; one more and it goes.
        for _ in 1..MAX_UNDERPRICED_BLOCKS {
            assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 0);
        }

        assert_eq!(mempool.len(), 2);
        assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 1);
        assert_eq!(mempool.iter().map(|tx| tx.nonce).collect::<Vec<_>>(), [0]);

        // A base price that came back down resets the count.
        add(&mut mempool, burn(&rich, 1, floor));
        mempool.prune(&state, Some(&fees), base).unwrap();
        assert_eq!(mempool.prune(&state, Some(&fees), floor).unwrap(), 0);
        for _ in 0..MAX_UNDERPRICED_BLOCKS {
            assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 0);
        }
        assert_eq!(mempool.len(), 2);
    }
}
