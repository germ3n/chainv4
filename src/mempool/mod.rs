//! Transactions waiting for a block, keyed by sender and nonce so each sender's run in order.

use {
    crate::chain::{Fees, tx_bytes},
    crate::crypto::verifiable::VerifyError,
    crate::db::{DbError, State},
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    r#macro::document,
    std::cmp::Reverse,
    std::collections::btree_map::Entry,
    std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap},
    std::fmt,
};

#[document(
    summary = "Transactions waiting for a block. Admission checks everything that does not change with state, so a selected transaction can only fail at the nonce if another one with the same nonce landed first. Its limits keep any one sender to a share of it, and a full one makes room for a better offer by pushing out the worst.",
    category = "Node",
    see_also = "SignedTx, Node, MempoolLimits",
)]
pub struct Mempool {
    txs: BTreeMap<(Address, u64), Waiting>,
    /// Every waiting transaction but each sender's next, the first to go first: what a full
    /// mempool pushes out to make room. A sender's next is left out, as its whole run waits on it.
    evictable: BTreeSet<Slot>,
    /// The nonce at the tip of each sender with anything waiting, as last read: which of its
    /// transactions is its next, and which can run in a row after it.
    nonces: HashMap<Address, u64>,
    limits: MempoolLimits,
    /// The next block's base price as the last prune heard it, which offers are weighed at.
    base: u256,
}

/// Where a waiting transaction stands for eviction, lowest first: one that cannot run for a gap
/// before it, then the least paid, then, of a sender's, the highest nonce, so a run loses its end
/// before its middle.
type Slot = (bool, u256, Reverse<u64>, Address);

/// A transaction waiting for a block, and how long it has waited unable to go in.
struct Waiting {
    tx: SignedTx,
    /// Blocks in a row whose base price its offer was under, so a producer could not take it.
    underpriced: u32,
    /// Blocks in a row a missing nonce before it kept it from running.
    gapped: u32,
    /// What it pays a producer per gas at the base price last heard: what eviction weighs.
    worth: u256,
    /// Whether every nonce from its sender's at the tip up to it waits, so it can run in turn.
    runnable: bool,
}

impl Waiting {
    fn slot(&self, sender: Address, nonce: u64) -> Slot {
        (self.runnable, self.worth, Reverse(nonce), sender)
    }
}

#[document(
    summary = "How many blocks in a row a transaction may offer less than the base price before the mempool drops it, unless --underpriced-for says otherwise: 32, about a minute of 2-second blocks and close to three of the devnet's 5-second ones, long enough for a base price that just rose to come back down.",
    category = "Node",
    see_also = "Mempool::prune, MempoolLimits::underpriced_blocks",
)]
pub const MAX_UNDERPRICED_BLOCKS: u32 = 32;

#[document(
    summary = "How many blocks in a row a transaction may wait behind a missing nonce of its sender's before the mempool drops it: 64, about two minutes of 2-second blocks and five of the devnet's 5-second ones. A gap that gossip opened by delivering out of order fills within a block or two; one still open after this is a nonce nobody sent.",
    category = "Node",
    see_also = "Mempool::prune, MempoolLimits",
)]
pub const MAX_GAPPED_BLOCKS: u32 = 64;

#[document(
    summary = "What a node's mempool holds, what it takes in place of what waits, and how long it keeps an offer under the base price. Set with --mempool-size, --mempool-per-sender, --max-nonce-gap, --replace-bump-percent and --underpriced-for.",
    category = "Node",
    see_also = "Mempool, MempoolError",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MempoolLimits {
    #[document(summary = "The most transactions waiting at once: 50,000 by default, a few seconds of the most blocks hold and some 20 MB of plain transfers. Full, a newcomer that pays more pushes out the least paid that is not its sender's next.")]
    pub capacity: usize,
    #[document(summary = "The most one sender may have waiting: 64 by default, more than a wallet or a bridge signer sends between blocks, and under a thousandth of the mempool, so one key can never fill it.")]
    pub per_sender: usize,
    #[document(summary = "How far past its sender's nonce at the tip a transaction's nonce may be: 64 by default, so a sender's full share fits in a row with one nonce to spare. Further ahead it could not run for a long while, and would hold room meanwhile.")]
    pub nonce_gap: u64,
    #[document(summary = "How much a transaction replacing a waiting one of its sender's, at its nonce, must raise both its max price and its tip, in percent: 10 by default, as Ethereum's nodes ask. Its max price must rise at least 1 whatever the percent, so a replacement always costs more and cannot be sent round for free.")]
    pub replace_bump: u32,
    #[document(summary = "How many blocks in a row a transaction may offer less than the base price before it is dropped: MAX_UNDERPRICED_BLOCKS by default. --underpriced-for takes it in blocks, or as a time turned into blocks at the chain's block time as the node starts.")]
    pub underpriced_blocks: u32,
}

impl MempoolLimits {
    #[document(parent = "MempoolLimits", summary = "The defaults: 50,000 in all, 64 per sender, 64 nonces ahead, a 10% bump to replace and MAX_UNDERPRICED_BLOCKS blocks under the base price.")]
    pub const DEFAULT: MempoolLimits = MempoolLimits {
        capacity: 50_000,
        per_sender: 64,
        nonce_gap: 64,
        replace_bump: 10,
        underpriced_blocks: MAX_UNDERPRICED_BLOCKS,
    };

    #[document(
        parent = "MempoolLimits",
        summary = "Whether a nonce is close enough to its sender's at the tip to wait. Cheap, so a node checks it before it verifies a signature.",
        params = { nonce = "The transaction's nonce.", next = "Its sender's nonce at the tip." },
    )]
    pub fn reaches(&self, nonce: u64, next: u64) -> Result<(), MempoolError> {
        match nonce.saturating_sub(next) > self.nonce_gap {
            true => Err(MempoolError::TooFarAhead { next, most: self.nonce_gap }),
            false => Ok(()),
        }
    }

    /// Whether an offer replacing another at the same nonce raised both prices by the bump.
    fn outbids(&self, old: &FeeOffer, new: &FeeOffer) -> bool {
        let raised = |price: u256| price.saturating_add(price.saturating_mul(u256::from(self.replace_bump)).saturating_add(u256::new(99)) / u256::new(100));

        new.max_price >= raised(old.max_price).max(old.max_price.saturating_add(u256::ONE)) && new.tip >= raised(old.tip)
    }
}

impl Default for MempoolLimits {
    fn default() -> MempoolLimits {
        MempoolLimits::DEFAULT
    }
}

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
    #[document(summary = "This very transaction is already waiting, or, on a chain without fees, which has no price to raise, another from its sender with its nonce is.")]
    Duplicate,
    #[document(summary = "Another from its sender with its nonce is waiting, and this one does not raise both the max price and the tip by the bump in percent to replace it.")]
    ReplacementTooLow { bump: u32 },
    #[document(summary = "Its nonce is more than most past its sender's nonce at the tip, next: send the ones before it first.")]
    TooFarAhead { next: u64, most: u64 },
    #[document(summary = "Its sender has most waiting already, and this one's nonce comes after them all: wait for some to land.")]
    TooMany { most: usize },
    #[document(summary = "The mempool is full, and this one pays no more than anything it could push out, or cannot run until a nonce of its sender's that is missing arrives.")]
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
    #[document(summary = "A multisig's transaction its members do not back, and why.")]
    Multisig(RegistryError),
}

impl fmt::Display for MempoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MempoolError::Invalid(err) => write!(f, "{err}"),
            MempoolError::Stale => write!(f, "nonce already used"),
            MempoolError::Duplicate => write!(f, "already waiting"),
            MempoolError::ReplacementTooLow { bump } => write!(f, "a transaction with this nonce is already waiting: to replace it, raise both its max price and its tip by at least {bump}%"),
            MempoolError::TooFarAhead { next, most } => write!(f, "nonce too far ahead: the account's next is {next}, and this node takes nonces at most {most} past it"),
            MempoolError::TooMany { most } => write!(f, "this sender has {most} transactions waiting already, the most this node keeps: wait for some to land"),
            MempoolError::Full => write!(f, "mempool full: offer a higher tip to take the place of one that pays less"),
            MempoolError::BadGasLimit => write!(f, "gas limit under its intrinsic cost, or over the block gas limit"),
            MempoolError::PriceTooLow => write!(f, "max price under the least base price"),
            MempoolError::CannotPay => write!(f, "the sender cannot pay for its gas limit"),
            MempoolError::Syncing => write!(f, "the node is still syncing its state from a snapshot"),
            MempoolError::Db(err) => write!(f, "{err}"),
            MempoolError::Multisig(err) => write!(f, "{err}"),
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
        summary = "An empty mempool, with the default limits but its size.",
        params = { capacity = "The most transactions it holds." },
    )]
    pub fn new(capacity: usize) -> Mempool {
        Mempool::with_limits(MempoolLimits { capacity, ..MempoolLimits::DEFAULT })
    }

    #[document(parent = "Mempool", summary = "An empty mempool with these limits.")]
    pub fn with_limits(limits: MempoolLimits) -> Mempool {
        Mempool {
            txs: BTreeMap::new(),
            evictable: BTreeSet::new(),
            nonces: HashMap::new(),
            limits,
            base: u256::ZERO,
        }
    }

    #[document(parent = "Mempool", summary = "Its limits.")]
    pub fn limits(&self) -> MempoolLimits {
        self.limits
    }

    #[document(parent = "Mempool", summary = "Holds it to other limits from now on. What waits past them stays until it lands or is pruned.")]
    pub fn set_limits(&mut self, limits: MempoolLimits) {
        self.limits = limits;
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
        summary = "The highest nonce this sender has waiting, so a pending nonce count can follow it. A range over the sender's keys, not a scan of the whole mempool.",
        returns = "The highest nonce waiting from the sender, or None when it has none waiting.",
    )]
    pub fn highest_nonce(&self, sender: &Address) -> Option<u64> {
        self.txs.range((*sender, 0)..=(*sender, u64::MAX)).next_back().map(|((_, nonce), _)| *nonce)
    }

    #[cfg(feature = "ethereum")]
    #[document(
        parent = "Mempool",
        summary = "An Ethereum-signed transaction waiting by the keccak256 of its raw bytes, the hash its own nodes name it by. Scans the mempool once rather than copying it out for the caller to scan.",
        returns = "The transaction, or None when none waits by that hash.",
    )]
    pub fn ethereum_by_hash(&self, hash: &[u8; 32]) -> Option<SignedTx> {
        self.txs.values().find_map(|waiting| match &waiting.tx.tx {
            Tx::Ethereum { raw } if &crate::crypto::ethereum::tx_hash(raw) == hash => Some(waiting.tx.clone()),
            _ => None,
        })
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
        let checked = tx.verify_on(genesis);

        self.add_checked(tx, checked, genesis, state, fees)
    }

    #[document(
        parent = "Mempool",
        summary = "Admits a transaction whose signature was checked already, such as on the node's verifier thread, going by what that check found. Everything else is checked as add checks it.",
        params = {
            tx = "The transaction.",
            checked = "What its verify returned.",
            genesis = "The chain's genesis id, which a multisig's members sign over.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
    )]
    pub fn add_checked(&mut self, tx: SignedTx, checked: Result<(), VerifyError>, genesis: &Hash, state: &State, fees: Option<&Fees>) -> Result<(), MempoolError> {
        let next = Mempool::check(&tx, checked, genesis, state, fees)?;

        self.insert(tx, next, fees)
    }

    #[document(
        parent = "Mempool",
        summary = "Checks what admission checks that does not depend on what else waits: the signature, going by what checking it found; for a multisig's own transaction, that enough of its members back it; that its nonce is not used; and on a chain with fees, that its gas limit could fit a block, its price could be taken and its sender can pay for it now. It needs no mempool, so a node checks off its loop and locks the mempool only to insert.",
        params = {
            tx = "The transaction.",
            checked = "What its verify returned.",
            genesis = "The chain's genesis id, which a multisig's members sign over.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
        returns = "Its sender's nonce at the tip, for insert.",
        see_also = "Mempool::insert",
    )]
    pub fn check(tx: &SignedTx, checked: Result<(), VerifyError>, genesis: &Hash, state: &State, fees: Option<&Fees>) -> Result<u64, MempoolError> {
        // Cheap state checks first, so a transaction that cannot pay or whose nonce is used is
        // turned away before its signature, and a multisig's members' signatures, are verified.
        let next = Self::affordable(tx, state, fees)?;
        Self::backed(tx, checked, genesis, state)?;

        Ok(next)
    }

    #[document(
        parent = "Mempool",
        summary = "The checks that need no cryptography: that the nonce is not already used, and on a chain with fees, that the gas limit could fit a block, the price could be taken, and the sender can pay for the limit now. A node runs these before verifying a signature, so unpaid work is turned away cheaply.",
        params = {
            tx = "The transaction, whose sender's state these are checked against.",
            state = "The state at the tip.",
            fees = "The chain's fees, if it has any.",
        },
        returns = "Its sender's nonce at the tip, which MempoolLimits::reaches and insert go by.",
        see_also = "Mempool::check, Mempool::backed",
    )]
    pub fn affordable(tx: &SignedTx, state: &State, fees: Option<&Fees>) -> Result<u64, MempoolError> {
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

        Ok(account.nonce)
    }

    #[document(
        parent = "Mempool",
        summary = "The checks that cost cryptography: the transaction's own signature, going by what verify found, and for a multisig's own transaction, that enough of its members back it. Run after affordable, so a transaction that could not be admitted anyway never costs a verify.",
        params = {
            tx = "The transaction.",
            checked = "What its verify returned.",
            genesis = "The chain's genesis id, which a multisig's members sign over.",
            state = "The state at the tip.",
        },
        see_also = "Mempool::check, Mempool::affordable",
    )]
    pub fn backed(tx: &SignedTx, checked: Result<(), VerifyError>, genesis: &Hash, state: &State) -> Result<(), MempoolError> {
        checked.map_err(MempoolError::Invalid)?;

        // A multisig's transaction that its members do not back yet never waits: it could not run,
        // and nobody would pay for the room it took.
        if let Tx::Multisig { account, tx: inner, signatures } = &tx.tx {
            let body = crate::registry::multisig::MultisigBody { account: *account, tx: (**inner).clone(), nonce: tx.nonce, fee: tx.fee };

            match crate::registry::Multisigs::new(state).backing(&body, signatures, genesis) {
                Ok((policy, backed)) if backed >= policy.threshold => {}
                Ok((policy, backed)) => return Err(MempoolError::Multisig(RegistryError::NotEnoughMembers { backed, needed: policy.threshold })),
                Err(RegistryError::Db(err)) => return Err(MempoolError::Db(err)),
                Err(err) => return Err(MempoolError::Multisig(err)),
            }
        }

        Ok(())
    }

    #[document(
        parent = "Mempool",
        summary = "Puts in a transaction that check passed. At a nonce its sender has waiting already, it replaces that one if it raises both prices by the bump, on a chain with fees. Otherwise its nonce must be within the gap of its sender's, and its sender under its share: at its share, a nonce below the others' pushes out the sender's highest. When the mempool is full, a newcomer that can run in turn pushes out the least paid transaction that is not its sender's next, with the rest of that sender's after it, which could not run without it; one that pays no more is refused.",
        params = {
            tx = "The transaction.",
            next = "Its sender's nonce at the tip, as check found it.",
            fees = "The chain's fees, if it has any: what offers are weighed by.",
        },
        see_also = "Mempool::check, MempoolLimits",
    )]
    pub fn insert(&mut self, tx: SignedTx, next: u64, fees: Option<&Fees>) -> Result<(), MempoolError> {
        let (sender, nonce) = (tx.sender(), tx.nonce);

        if nonce < next {
            return Err(MempoolError::Stale);
        }

        let worth = self.worth(&tx.fee, fees);

        if let Some(old) = self.txs.get(&(sender, nonce)) {
            // Without fees there is no price to raise, and replacing for free would let a sender
            // send round as much as it liked.
            if old.tx == tx || fees.is_none() {
                return Err(MempoolError::Duplicate);
            }

            if !self.limits.outbids(&old.tx.fee, &tx.fee) {
                return Err(MempoolError::ReplacementTooLow { bump: self.limits.replace_bump });
            }

            // Its place in its sender's run stays, so the run stays whole.
            let old = self.take(sender, nonce).expect("it waits");
            self.put(sender, nonce, Waiting { tx, underpriced: 0, gapped: old.gapped, worth, runnable: old.runnable });
            self.reindex(sender, next);

            return Ok(());
        }

        self.limits.reaches(nonce, next)?;

        let waiting = self.txs.range((sender, 0)..=(sender, u64::MAX)).count();

        if waiting >= self.limits.per_sender {
            match self.highest_nonce(&sender) {
                // A nonce the others wait on takes the place of the last: the run loses its end,
                // which could not go before this one anyway.
                Some(highest) if highest > nonce => {
                    self.take(sender, highest);
                }
                _ => return Err(MempoolError::TooMany { most: self.limits.per_sender }),
            }
        }

        if self.txs.len() >= self.limits.capacity {
            self.evict_for(sender, nonce, next, worth)?;
        }

        self.put(sender, nonce, Waiting { tx, underpriced: 0, gapped: 0, worth, runnable: false });
        self.reindex(sender, next);

        Ok(())
    }

    /// Makes room in a full mempool for a newcomer worth this much, by pushing out the lowest slot
    /// and the rest of its sender's after it; or refuses it, when it pays no more than that slot
    /// or could not run anyway.
    fn evict_for(&mut self, sender: Address, nonce: u64, next: u64, worth: u256) -> Result<(), MempoolError> {
        // Only a newcomer that can run once those before it do may push anything out: one behind
        // a gap could otherwise empty the mempool of transactions that run, for some that never do.
        let runnable = nonce == next || nonce.checked_sub(1).and_then(|before| self.txs.get(&(sender, before))).is_some_and(|before| before.runnable);

        if !runnable {
            return Err(MempoolError::Full);
        }

        // Never one of the newcomer's own sender's below it, which it needs to run.
        let Some(&(kept, paid, Reverse(at), victim)) = self.evictable.iter().find(|&&(_, _, Reverse(at), victim)| !(victim == sender && at < nonce)) else {
            return Err(MempoolError::Full);
        };

        if (kept, paid) >= (true, worth) {
            return Err(MempoolError::Full);
        }

        let after: Vec<u64> = self.txs.range((victim, at)..=(victim, u64::MAX)).map(|((_, nonce), _)| *nonce).collect();

        for nonce in after {
            self.take(victim, nonce);
        }

        Ok(())
    }

    /// What an offer pays a producer per gas at the base price last heard; 0 without fees, or
    /// for an offer under it.
    fn worth(&self, offer: &FeeOffer, fees: Option<&Fees>) -> u256 {
        fees.map_or(u256::ZERO, |fees| fees.priority(self.base, offer))
    }

    /// Puts a transaction in, slotted for eviction unless it is its sender's next.
    fn put(&mut self, sender: Address, nonce: u64, waiting: Waiting) {
        if self.nonces.get(&sender) != Some(&nonce) {
            self.evictable.insert(waiting.slot(sender, nonce));
        }

        self.txs.insert((sender, nonce), waiting);
    }

    /// Takes a transaction out, and its slot, forgetting its sender's nonce once nothing of its waits.
    fn take(&mut self, sender: Address, nonce: u64) -> Option<Waiting> {
        let waiting = self.txs.remove(&(sender, nonce))?;
        self.evictable.remove(&waiting.slot(sender, nonce));

        if self.txs.range((sender, 0)..=(sender, u64::MAX)).next().is_none() {
            self.nonces.remove(&sender);
        }

        Some(waiting)
    }

    /// Sets which of a sender's can run in turn, from its nonce at the tip, and slots them again.
    /// A sender has at most its share waiting, so this is cheap.
    fn reindex(&mut self, sender: Address, next: u64) {
        self.nonces.insert(sender, next);
        let mut expected = next;

        for ((_, nonce), waiting) in self.txs.range_mut((sender, 0)..=(sender, u64::MAX)) {
            self.evictable.remove(&waiting.slot(sender, *nonce));
            waiting.runnable = *nonce == expected;

            if waiting.runnable {
                expected += 1;
            }

            if *nonce != next {
                self.evictable.insert(waiting.slot(sender, *nonce));
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
        summary = "Brings the mempool in line with a new tip: drops transactions whose nonce is used; on a chain with fees, those that can no longer pay, as their sender's funds went since they were admitted, and those that have offered less than the base price for MempoolLimits::underpriced_blocks blocks in a row; and those that have waited behind a missing nonce for MAX_GAPPED_BLOCKS blocks in a row. An offer under the base price is kept until then, since the base price moves a step a block and may come back down. What waits after a transaction dropped here is kept, behind a gap, so its sender need only send the one again; it goes too if it is not.",
        params = {
            state = "The state at the new tip.",
            fees = "The chain's fees, if it has any.",
            base = "The base price of the next block: Chain::next_base_price.",
        },
        returns = "How many were dropped other than for landing: for a sender that cannot pay, an offer under the base price for too long, or a gap that stayed open.",
    )]
    pub fn prune(&mut self, state: &State, fees: Option<&Fees>, base: u256) -> Result<usize, DbError> {
        let accounts = Accounts::new(state);
        let mut senders: BTreeMap<Address, (u64, u256)> = BTreeMap::new();
        self.base = base;

        for sender in self.txs.keys().map(|(sender, _)| *sender) {
            if let Entry::Vacant(entry) = senders.entry(sender) {
                let account = accounts.get(&sender)?;
                entry.insert((account.nonce, account.balance));
            }
        }

        self.txs.retain(|(sender, nonce), _| *nonce >= senders[sender].0);
        let before = self.txs.len();

        if let Some(fees) = fees {
            let most = self.limits.underpriced_blocks;
            self.txs.retain(|(sender, _), waiting| {
                if waiting.tx.fee.reserve().is_none_or(|reserve| reserve > senders[sender].1) {
                    return false;
                }

                waiting.underpriced = match fees.pricing().price(base, &waiting.tx.fee) {
                    Some(_) => 0,
                    None => waiting.underpriced + 1,
                };

                waiting.underpriced <= most
            });
        }

        // Which can run in turn now, and for how long each that cannot has been kept from it.
        let mut expected: Option<(Address, u64)> = None;

        for ((sender, nonce), waiting) in self.txs.iter_mut() {
            let next = match expected {
                Some((address, next)) if address == *sender => next,
                _ => senders[sender].0,
            };
            waiting.runnable = *nonce == next;
            waiting.gapped = if waiting.runnable { 0 } else { waiting.gapped + 1 };
            waiting.worth = fees.map_or(u256::ZERO, |fees| fees.priority(base, &waiting.tx.fee));
            expected = Some((*sender, next + u64::from(waiting.runnable)));
        }

        self.txs.retain(|_, waiting| waiting.gapped <= MAX_GAPPED_BLOCKS);
        self.nonces = self.txs.keys().map(|(sender, _)| (*sender, senders[sender].0)).collect();
        self.evictable = self
            .txs
            .iter()
            .filter(|((sender, nonce), _)| self.nonces[sender] != *nonce)
            .map(|((sender, nonce), waiting)| waiting.slot(*sender, *nonce))
            .collect();

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

        SignedTx::sign(tx, nonce, FeeOffer::NONE, keypair, &genesis())
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

        SignedTx::sign(tx, nonce, fee, keypair, &genesis())
    }

    #[test]
    fn a_transaction_signed_for_another_chain_does_not_wait_here_and_one_for_this_chain_does() {
        let keypair = keypair();
        let state = state();
        let mut mempool = Mempool::new(10);
        let tx = Tx::Burn { from: keypair.public().address(), amount: Amount::U128(1) };
        let elsewhere = crate::crypto::hasher::Hasher::digest(crate::structs::hash::HashType::default_chain(), b"another chain");
        let foreign = SignedTx::sign(tx.clone(), 0, FeeOffer::NONE, &keypair, &elsewhere);

        assert_eq!(mempool.add(foreign, &genesis(), &state, None), Err(MempoolError::Invalid(VerifyError::InvalidSignature)));
        assert_eq!(mempool.len(), 0);

        mempool.add(SignedTx::sign(tx, 0, FeeOffer::NONE, &keypair, &genesis()), &genesis(), &state, None).unwrap();

        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn the_highest_nonce_a_sender_has_waiting_comes_from_a_range_not_a_scan() {
        let state = state();
        let mut mempool = Mempool::new(100);
        let (one, two) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());

        assert_eq!(mempool.highest_nonce(&one.public().address()), None, "none waiting");

        for nonce in 0..5 {
            mempool.add(transfer(&one, nonce), &genesis(), &state, None).unwrap();
        }
        mempool.add(transfer(&two, 0), &genesis(), &state, None).unwrap();

        assert_eq!(mempool.highest_nonce(&one.public().address()), Some(4), "the sender's own run, not another's");
        assert_eq!(mempool.highest_nonce(&two.public().address()), Some(0));
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

        assert_eq!(Mempool::check(&tx, Ok(()), &Hash::zero(crate::structs::hash::HashType::default_chain()), &state, None), Ok(0), "the sender's nonce at the tip");
        assert!(matches!(Mempool::check(&tx, Err(VerifyError::InvalidSignature), &Hash::zero(crate::structs::hash::HashType::default_chain()), &state, None), Err(MempoolError::Invalid(_))));

        let mut mempool = Mempool::new(1);

        assert_eq!(mempool.insert(tx.clone(), 0, None), Ok(()));
        assert_eq!(mempool.insert(tx, 0, None), Err(MempoolError::Duplicate), "the very same one");
        assert_eq!(mempool.insert(transfer(&keypair, 1), 0, None), Err(MempoolError::Full));

        let mut roomy = Mempool::new(5);
        let again = transfer(&keypair, 3);
        roomy.insert(again.clone(), 0, None).unwrap();

        assert_eq!(roomy.insert(again, 0, None), Err(MempoolError::Duplicate));
    }

    #[test]
    fn the_cheap_checks_come_before_the_signature_so_an_unpayable_transaction_costs_no_verify() {
        let state = state();
        let fees = FeeConfig::dev().build();
        let genesis = genesis();
        // A poor sender's transaction, whose signature does not hold either: affordable catches it
        // first, so the result is CannotPay, not Invalid, which proves the verify was never reached.
        let poor = Keypair::generate(keypair().public().key_type()).unwrap();
        let tx = SignedTx::sign(Tx::Burn { from: poor.public().address(), amount: Amount::U128(1) }, 0, FeeOffer { gas_limit: 100_000, max_price: fees.pricing().floor(), tip: u256::ZERO }, &poor, &genesis);

        assert_eq!(Mempool::affordable(&tx, &state, Some(&fees)), Err(MempoolError::CannotPay));
        assert_eq!(Mempool::check(&tx, Err(VerifyError::InvalidSignature), &genesis, &state, Some(&fees)), Err(MempoolError::CannotPay), "cheap first");
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

            SignedTx::sign(tx, nonce, fee, keypair, &genesis())
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

            SignedTx::sign(tx, nonce, fee, keypair, &genesis())
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

    #[test]
    fn underpriced_for_keeps_an_offer_under_the_base_price_as_many_blocks_as_the_operator_said() {
        let (state, fees, floor, keys) = paying(1);
        let mut mempool = Mempool::with_limits(MempoolLimits { underpriced_blocks: 3, ..MempoolLimits::DEFAULT });
        let tx = SignedTx::sign(Tx::Burn { from: keys[0].public().address(), amount: Amount::U128(1) }, 0, FeeOffer { gas_limit: 100_000, max_price: floor, tip: u256::ZERO }, &keys[0], &genesis());
        mempool.add(tx, &genesis(), &state, Some(&fees)).unwrap();
        let base = floor * u256::from(2u64);

        // Three blocks under the base price it waits, where the default waits 32; the fourth drops it.
        for _ in 0..3 {
            assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 0);
        }

        assert_eq!(mempool.prune(&state, Some(&fees), base).unwrap(), 1);
        assert!(mempool.is_empty());
    }

    /// A chain with fees, its floor, and senders each funded with plenty.
    fn paying(senders: usize) -> (State, Fees, u256, Vec<Keypair>) {
        let state = state();
        let fees = FeeConfig::dev().build();
        let floor = fees.pricing().floor();
        let keys: Vec<Keypair> = (0..senders).map(|_| Keypair::generate(keypair().public().key_type()).unwrap()).collect();

        for key in &keys {
            Accounts::new(&state).credit(&key.public().address(), u256::from(10u64).pow(24)).unwrap();
        }

        (state, fees, floor, keys)
    }

    /// A burn with a max price well over the floor, so what it pays a producer is its tip.
    fn tipping(keypair: &Keypair, nonce: u64, floor: u256, tip: u64) -> SignedTx {
        let tx = Tx::Burn { from: keypair.public().address(), amount: Amount::U128(1) };
        let fee = FeeOffer { gas_limit: 100_000, max_price: floor * u256::from(1_000u64), tip: u256::from(tip) };

        SignedTx::sign(tx, nonce, fee, keypair, &genesis())
    }

    /// Who waits, by which of the keys sent it and its nonce, in order.
    fn waiting(mempool: &Mempool, keys: &[Keypair]) -> Vec<(usize, u64)> {
        mempool.iter().map(|tx| (keys.iter().position(|key| key.public().address() == tx.sender()).unwrap(), tx.nonce)).collect::<BTreeSet<_>>().into_iter().collect()
    }

    #[test]
    fn a_sender_keeps_to_its_share_and_a_nonce_the_others_wait_on_takes_the_place_of_its_highest() {
        let state = state();
        let keypair = keypair();
        let mut mempool = Mempool::with_limits(MempoolLimits { per_sender: 3, ..MempoolLimits::DEFAULT });
        let add = |mempool: &mut Mempool, nonce: u64| mempool.add(transfer(&keypair, nonce), &genesis(), &state, None);

        for nonce in 1..=3 {
            add(&mut mempool, nonce).unwrap();
        }

        assert_eq!(add(&mut mempool, 4), Err(MempoolError::TooMany { most: 3 }), "past its share, and after all it has waiting");
        assert_eq!(add(&mut mempool, 0), Ok(()), "the nonce the rest wait on");
        assert_eq!(mempool.iter().map(|tx| tx.nonce).collect::<Vec<_>>(), [0, 1, 2], "its highest made room");
        assert_eq!(mempool.select(&state, Budget::default(), &free).unwrap().len(), 3, "all of them run now");
    }

    #[test]
    fn a_nonce_too_far_past_the_senders_is_refused() {
        let state = state();
        let keypair = keypair();
        let limits = MempoolLimits { nonce_gap: 5, ..MempoolLimits::DEFAULT };
        let mut mempool = Mempool::with_limits(limits);
        Accounts::new(&state).bump_nonce(&keypair.public().address()).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 6), &genesis(), &state, None), Ok(()), "5 past the account's 1");
        assert_eq!(mempool.add(transfer(&keypair, 7), &genesis(), &state, None), Err(MempoolError::TooFarAhead { next: 1, most: 5 }));
        assert_eq!(limits.reaches(7, 1), Err(MempoolError::TooFarAhead { next: 1, most: 5 }), "what a node checks before any verify");
        assert_eq!(MempoolLimits { nonce_gap: 0, ..limits }.reaches(2, 1), Err(MempoolError::TooFarAhead { next: 1, most: 0 }), "0 takes only the next");
    }

    #[test]
    fn a_replacement_must_raise_both_prices_by_the_bump_and_keeps_its_place_in_the_run() {
        let (state, fees, floor, keys) = paying(1);
        let mut mempool = Mempool::new(100);
        mempool.prune(&state, Some(&fees), floor).unwrap();
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees));
        let offered = |max_price: u256, tip: u64| {
            let tx = Tx::Burn { from: keys[0].public().address(), amount: Amount::U128(2) };

            SignedTx::sign(tx, 1, FeeOffer { gas_limit: 100_000, max_price, tip: u256::from(tip) }, &keys[0], &genesis())
        };
        let max = floor * u256::from(1_000u64);

        for nonce in 0..3 {
            add(&mut mempool, tipping(&keys[0], nonce, floor, 100)).unwrap();
        }

        let too_low = || MempoolError::ReplacementTooLow { bump: 10 };
        assert_eq!(add(&mut mempool, tipping(&keys[0], 1, floor, 100)), Err(MempoolError::Duplicate), "the very same one");
        assert_eq!(add(&mut mempool, offered(max * u256::from(105u64) / u256::from(100u64), 110)), Err(too_low()), "its max price rose 5%");
        assert_eq!(add(&mut mempool, offered(max * u256::from(11u64) / u256::from(10u64), 105)), Err(too_low()), "its tip rose 5%");

        let better = offered(max * u256::from(11u64) / u256::from(10u64), 110);
        add(&mut mempool, better.clone()).unwrap();

        // The producer's pick takes the replacement, in its place between the two either side.
        let picked = mempool.select(&state, Budget::default(), &|tx| Some(fees.priority(floor, &tx.fee))).unwrap();

        assert_eq!(mempool.len(), 3);
        assert_eq!(picked.iter().map(|tx| tx.nonce).collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(picked[1], better);
    }

    #[test]
    fn a_tip_of_nothing_may_stay_nothing_but_the_max_price_must_still_rise_and_a_chain_without_fees_replaces_nothing() {
        let limits = MempoolLimits::DEFAULT;
        let offer = |max_price: u64, tip: u64| FeeOffer { gas_limit: 1, max_price: u256::from(max_price), tip: u256::from(tip) };

        assert!(limits.outbids(&offer(100, 0), &offer(110, 0)));
        assert!(!limits.outbids(&offer(100, 0), &offer(109, 0)));
        assert!(!limits.outbids(&offer(0, 0), &offer(0, 0)), "nothing raised is no replacement, at any bump");
        assert!(MempoolLimits { replace_bump: 0, ..limits }.outbids(&offer(0, 0), &offer(1, 0)));
        assert!(!limits.outbids(&offer(5, 0), &offer(5, 0)), "a 10% rise of 5 rounds up to 1");

        let state = state();
        let mut mempool = Mempool::new(10);
        let keypair = keypair();
        mempool.add(transfer(&keypair, 0), &genesis(), &state, None).unwrap();

        assert_eq!(mempool.add(transfer(&keypair, 0), &genesis(), &state, None), Err(MempoolError::Duplicate), "another transfer, at the same nonce");
    }

    #[test]
    fn a_full_mempool_pushes_out_the_least_paid_that_is_not_its_senders_next_with_what_follows_it() {
        let (state, fees, floor, keys) = paying(4);
        let mut mempool = Mempool::new(4);
        mempool.prune(&state, Some(&fees), floor).unwrap();
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees));

        // Key 0's next pays least of all, but its run waits on it, so it stays; its second and
        // third, which pays well but cannot go without the second, go together.
        add(&mut mempool, tipping(&keys[0], 0, floor, 1)).unwrap();
        add(&mut mempool, tipping(&keys[0], 1, floor, 2)).unwrap();
        add(&mut mempool, tipping(&keys[0], 2, floor, 90)).unwrap();
        add(&mut mempool, tipping(&keys[1], 0, floor, 50)).unwrap();

        assert_eq!(add(&mut mempool, tipping(&keys[2], 0, floor, 2)), Err(MempoolError::Full), "it pays no more than the least");

        add(&mut mempool, tipping(&keys[2], 0, floor, 3)).unwrap();

        assert_eq!(waiting(&mempool, &keys), [(0, 0), (1, 0), (2, 0)]);

        // Nothing is left to push out but each sender's next.
        add(&mut mempool, tipping(&keys[3], 0, floor, 1)).unwrap();

        assert_eq!(add(&mut mempool, tipping(&keys[2], 1, floor, 1_000)), Err(MempoolError::Full));
        assert_eq!(waiting(&mempool, &keys), [(0, 0), (1, 0), (2, 0), (3, 0)]);
    }

    #[test]
    fn a_full_mempool_pushes_out_what_waits_behind_a_gap_first_and_takes_nothing_new_behind_one() {
        let (state, fees, floor, keys) = paying(3);
        let mut mempool = Mempool::new(3);
        mempool.prune(&state, Some(&fees), floor).unwrap();
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees));
        add(&mut mempool, tipping(&keys[0], 0, floor, 1)).unwrap();
        add(&mut mempool, tipping(&keys[0], 1, floor, 1)).unwrap();
        // Paying well, but behind a nonce its sender never sent.
        add(&mut mempool, tipping(&keys[1], 3, floor, 500)).unwrap();

        assert_eq!(add(&mut mempool, tipping(&keys[2], 2, floor, 1_000)), Err(MempoolError::Full), "behind a gap itself, it could push out what runs for nothing that does");

        add(&mut mempool, tipping(&keys[2], 0, floor, 2)).unwrap();

        assert_eq!(waiting(&mempool, &keys), [(0, 0), (0, 1), (2, 0)], "the gapped one went first, though it paid more");
    }

    #[test]
    fn one_key_flooding_the_mempool_leaves_room_for_everyone_else() {
        let (state, fees, floor, keys) = paying(38);
        let mut mempool = Mempool::new(100);
        mempool.prune(&state, Some(&fees), floor).unwrap();
        let add = |mempool: &mut Mempool, tx: SignedTx| mempool.add(tx, &genesis(), &state, Some(&fees));

        // A thousand nonces in no order, near and far, all at a good price: it keeps its share,
        // the lowest, in a run.
        for nonce in 0..1_000 {
            let _ = add(&mut mempool, tipping(&keys[0], nonce * 7 % 1_000, floor, 10));
        }

        assert_eq!(mempool.iter().map(|tx| tx.nonce).collect::<Vec<_>>(), (0..64).collect::<Vec<_>>());

        // Everyone else still gets in, paying less, until it is full.
        for key in &keys[1..37] {
            add(&mut mempool, tipping(key, 0, floor, 1)).unwrap();
        }

        assert_eq!(mempool.len(), 100);

        // And a better offer still finds room, at the end of the flooder's run.
        add(&mut mempool, tipping(&keys[37], 0, floor, 20)).unwrap();

        assert_eq!(mempool.len(), 100);
        assert_eq!(mempool.highest_nonce(&keys[0].public().address()), Some(62));
    }

    #[test]
    fn what_waits_behind_a_gap_too_long_is_pruned_and_a_gap_filled_in_time_keeps_it() {
        let state = state();
        let mut mempool = Mempool::new(10);
        let (stuck, filled) = (keypair(), Keypair::generate(keypair().public().key_type()).unwrap());
        let add = |mempool: &mut Mempool, keypair: &Keypair, nonce: u64| mempool.add(transfer(keypair, nonce), &genesis(), &state, None).unwrap();
        add(&mut mempool, &stuck, 2);
        add(&mut mempool, &filled, 1);

        for _ in 0..MAX_GAPPED_BLOCKS - 1 {
            assert_eq!(mempool.prune(&state, None, u256::ZERO).unwrap(), 0);
        }

        // The gap before filled's closes in time: running, it waits on as long as it must.
        add(&mut mempool, &filled, 0);

        assert_eq!(mempool.prune(&state, None, u256::ZERO).unwrap(), 0);
        assert_eq!(mempool.prune(&state, None, u256::ZERO).unwrap(), 1, "stuck's has waited behind its gap for more than MAX_GAPPED_BLOCKS");
        assert_eq!(mempool.iter().map(|tx| (tx.sender() == filled.public().address(), tx.nonce)).collect::<Vec<_>>(), [(true, 0), (true, 1)]);

        // Nonces that landed go too, and what follows them runs.
        Accounts::new(&state).bump_nonce(&filled.public().address()).unwrap();

        assert_eq!(mempool.prune(&state, None, u256::ZERO).unwrap(), 0, "landing is not dropping");
        assert_eq!(mempool.iter().map(|tx| tx.nonce).collect::<Vec<_>>(), [1]);
    }
}
