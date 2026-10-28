//! Mining pools' claims. A pool's block sends its miners' share of the reward and fees to the
//! pool's escrow, an account no key controls, and commits a Merkle root over every miner's
//! earnings over the pool's life (a PoolCommitment). A miner claims with a Merkle path to its
//! leaf under any root its pool committed (Tx::Claim), and is paid what the leaf says it earned
//! less what it claimed before, from the escrow. So any number of miners fit in a block in 44
//! bytes, one claim pays for any number of blocks, and the pool never holds what it owes: it can
//! only say who is owed, which every root makes public.

use {
    crate::crypto::hashable::Hashable,
    crate::crypto::merkle,
    crate::db::{Column, DbError, State, address_key},
    crate::registry::{Accounts, RegistryError},
    crate::structs::address::Address,
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::Tx,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A pool's escrow: the account its blocks send their miners' share to, and claims are paid from. A Contract address with no code, so no key and no contract can spend it; only Tx::Claim does. Anyone may send it more.",
    category = "Mining",
    see_also = "PoolCommitment, Tx::Claim",
)]
pub fn pool_escrow(pool: &Address, hash: HashType) -> Address {
    let mut hasher = crate::crypto::hasher::Hasher::new(hash);
    hasher.update(b"chainv4 pool escrow");
    hasher.update(&pool.hashable_bytes_with_tag());
    let bytes: [u8; 32] = hasher.finalize().to_bytes().try_into().expect("genesis only allows 256-bit chain hashes");

    Address::Contract(bytes)
}

#[document(
    summary = "A claim's leaf in a pool's root: a miner and everything it has earned from the pool, as a Merkle leaf, with a domain of its own so it is no other leaf the chain hashes.",
    category = "Mining",
    see_also = "PoolCommitment, Tx::Claim",
)]
pub fn claim_leaf(hash: HashType, to: &Address, earned: u256) -> Hash {
    let mut data = b"chainv4 pool claim".to_vec();
    data.extend(to.hashable_bytes_with_tag());
    data.extend(earned.to_le_bytes());

    merkle::leaf(hash, &data)
}

#[document(
    summary = "A root a pool's block committed to: whose pool, the root, and how many claims it holds.",
    category = "Mining",
    see_also = "Pools",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct PoolRoot {
    #[document(summary = "The pool: the block's producer, whose escrow its claims are paid from.")]
    pub pool: Address,
    #[document(summary = "The root over its claims.")]
    pub root: Hash,
    #[document(summary = "How many claims it holds.")]
    pub count: u64,
}

impl Hashable for PoolRoot {}

/// How much a miner has claimed from a pool, over every claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
struct Claimed {
    #[wincode(with = "U256Le")]
    total: u256,
}

impl Hashable for Claimed {}

#[document(
    summary = "Why a claim pays nothing.",
    category = "Mining",
    see_also = "Pools::claim",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimError {
    #[document(summary = "No pool's root at that block: it carried none, or is not on this chain.")]
    NoRoot,
    #[document(summary = "The path does not lead from the leaf to the root.")]
    BadPath,
    #[document(summary = "The miner has claimed all the leaf says it earned.")]
    Claimed,
    #[document(summary = "The pool's escrow holds less than the claim, which only a pool that promised more than it sent can cause.")]
    Escrow,
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimError::NoRoot => write!(f, "no mining pool's root at that block"),
            ClaimError::BadPath => write!(f, "the path does not lead to the pool's root"),
            ClaimError::Claimed => write!(f, "nothing left to claim: all it earned is claimed"),
            ClaimError::Escrow => write!(f, "the pool's escrow holds less than the claim"),
            ClaimError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl From<DbError> for ClaimError {
    fn from(err: DbError) -> ClaimError {
        ClaimError::Db(err)
    }
}

/// Where the root of the block at a height is kept: by height, as a block's id hangs on the state
/// it leaves, this among it, and a branch holds one block at each height.
fn root_key(height: u64) -> Vec<u8> {
    [b"r".as_slice(), &height.to_be_bytes()].concat()
}

/// Where a miner's claimed total from a pool is kept.
fn claimed_key(pool: &Address, miner: &Address) -> Vec<u8> {
    [b"c".as_slice(), &address_key(pool), &address_key(miner)].concat()
}

#[document(
    summary = "The mining pools' claims, in a State: each pool block's root, by its height, and what each miner has claimed from each pool.",
    category = "Mining",
    see_also = "PoolCommitment, Tx::Claim, pool_escrow",
)]
pub struct Pools<'a> {
    state: &'a State,
    hash: HashType,
}

impl<'a> Pools<'a> {
    #[document(parent = "Pools", summary = "The claims in a State, on a chain with this hash.")]
    pub fn new(state: &'a State, hash: HashType) -> Pools<'a> {
        Pools { state, hash }
    }

    #[document(parent = "Pools", summary = "Keeps the root a pool's block, at this height, committed to.")]
    pub fn commit(&self, height: u64, root: PoolRoot) {
        self.state.put_value(Column::Pools, root_key(height), &root);
    }

    #[document(parent = "Pools", summary = "The root the block at a height committed to, if it carried one.")]
    pub fn root(&self, height: u64) -> Result<Option<PoolRoot>, DbError> {
        self.state.get_value(Column::Pools, &root_key(height))
    }

    #[document(parent = "Pools", summary = "How much a miner has claimed from a pool, every claim added up.")]
    pub fn claimed(&self, pool: &Address, miner: &Address) -> Result<u256, DbError> {
        Ok(self.state.get_value::<Claimed>(Column::Pools, &claimed_key(pool, miner))?.unwrap_or_default().total)
    }

    #[document(
        parent = "Pools",
        summary = "Pays a miner what a leaf under a block's root says it earned from the pool, less what it claimed before, from the pool's escrow. Anyone may claim for a miner: the coins go to the miner whoever asks.",
        params = {
            height = "The height of a block whose root holds the leaf.",
            to = "The miner.",
            earned = "All the miner earned from the pool, as the leaf says.",
            index = "The leaf's place among the root's claims.",
            path = "The Merkle path from the leaf to the root.",
        },
        returns = "What it paid.",
    )]
    pub fn claim(&self, height: u64, to: &Address, earned: u256, index: u64, path: &[Hash]) -> Result<u256, ClaimError> {
        let Some(root) = self.root(height)? else {
            return Err(ClaimError::NoRoot);
        };
        let leaf = claim_leaf(self.hash, to, earned);
        let (Ok(index), Ok(count)) = (usize::try_from(index), usize::try_from(root.count)) else {
            return Err(ClaimError::BadPath);
        };

        if index >= count || !merkle::verify(&root.root, &leaf, index, count, path) {
            return Err(ClaimError::BadPath);
        }

        let claimed = self.claimed(&root.pool, to)?;

        if earned <= claimed {
            return Err(ClaimError::Claimed);
        }

        let owed = earned - claimed;
        let escrow = pool_escrow(&root.pool, self.hash);
        Accounts::new(self.state).transfer(&escrow, to, owed).map_err(|err| match err {
            RegistryError::Db(err) => ClaimError::Db(err),
            _ => ClaimError::Escrow,
        })?;
        self.state.put_value(Column::Pools, claimed_key(&root.pool, to), &Claimed { total: earned });

        Ok(owed)
    }
}

#[document(
    summary = "A miner's claim on a pool: all it earned at one of the pool's roots, and the Merkle path to it, as the pool's node serves it and as a miner keeps it. With it a miner claims from any node, the pool's or not, even once the pool is gone: the chain holds the root and the escrow, and only the leaf and its path were the pool's to give.",
    category = "Mining",
    see_also = "SavedClaims, Tx::Claim, Pools::claim",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct PoolClaim {
    #[document(summary = "The pool: its blocks' producer, whose escrow pays the claim.")]
    pub pool: Address,
    #[document(summary = "The height of the pool's block whose root holds the claim.")]
    pub height: u64,
    #[document(summary = "That block's id, when the claim was given.")]
    pub block: Hash,
    #[document(summary = "The miner.")]
    pub to: Address,
    #[document(summary = "All the miner earned from the pool, as its leaf says.")]
    #[wincode(with = "U256Le")]
    pub earned: u256,
    #[document(summary = "The leaf's place among the root's claims.")]
    pub index: u64,
    #[document(summary = "How many claims the root holds.")]
    pub count: u64,
    #[document(summary = "The Merkle path from the leaf to the root.")]
    pub path: Vec<Hash>,
}

impl PoolClaim {
    #[document(parent = "PoolClaim", summary = "The claim as a transaction, which anyone may send: the coins go to the miner.")]
    pub fn tx(&self) -> Tx {
        Tx::Claim { height: self.height, to: self.to, earned: self.earned, index: self.index, path: self.path.clone() }
    }

    #[document(
        parent = "PoolClaim",
        summary = "Whether the claim holds under a root on chain, the one at its height: the same pool, as many claims, and a path from its leaf to the root. One that does not, its block undone since, pays nothing.",
    )]
    pub fn holds(&self, root: &PoolRoot, hash: HashType) -> bool {
        let (Ok(index), Ok(count)) = (usize::try_from(self.index), usize::try_from(self.count)) else {
            return false;
        };

        root.pool == self.pool && root.count == self.count && merkle::verify(&root.root, &claim_leaf(hash, &self.to, self.earned), index, count, &self.path)
    }
}

#[document(
    summary = "How many claims on a pool not yet settled a miner keeps, at most: the oldest, the one to settle next, and the newest after it.",
    category = "Mining",
    see_also = "SavedClaims",
)]
pub const KEPT_PENDING: usize = 32;

#[document(
    summary = "How many settled claims on a pool a miner keeps: the newest, and one before it should the newest's block have been dropped.",
    category = "Mining",
    see_also = "SavedClaims",
)]
pub const KEPT_SETTLED: usize = 2;

/// The order claims are kept in: by pool and miner, then newest first.
fn in_order(a: &PoolClaim, b: &PoolClaim) -> std::cmp::Ordering {
    (a.pool, a.to).cmp(&(b.pool, b.to)).then(b.height.cmp(&a.height))
}

#[document(
    summary = "The claims a miner keeps, on each pool it mines for: enough that one holds whatever happens to the pool. The newest pays the most, but its block is the newest the pool sealed, the likeliest to be undone; one at or under a settled height can no longer be. So it keeps the newest few, the oldest not yet settled, which settles next, and the newest settled.",
    category = "Mining",
    see_also = "PoolClaim, KEPT_PENDING, KEPT_SETTLED",
    note = "Every claim on a pool holds all the miner earned there up to its root, so one that holds pays everything up to it, less what was claimed before. Older claims pay less, never more.",
)]
#[derive(Clone, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct SavedClaims {
    /// Each pool's and miner's claims, newest first, in_order.
    claims: Vec<PoolClaim>,
    /// The highest settled height told so far, so a claim kept later without one still knows
    /// which of those kept can no longer be undone.
    settled: Option<u64>,
}

impl SavedClaims {
    #[document(
        parent = "SavedClaims",
        summary = "Keeps a claim, in place of one at its height, and forgets the claims on its pool for its miner it no longer needs.",
        params = {
            claim = "The claim, as the pool gave it.",
            settled = "The highest height the pool's blocks can no longer be undone at, as far as is known: None while none is known. The highest told before stands when it is lower or None.",
        },
        returns = "Whether anything changed, so whether to write it out again.",
    )]
    pub fn keep(&mut self, claim: PoolClaim, settled: Option<u64>) -> bool {
        let before = (self.claims.clone(), self.settled);
        // Settled stays settled: a height told before holds whatever this call knows.
        let settled = settled.max(self.settled);
        self.settled = settled;
        let (pool, to) = (claim.pool, claim.to);
        let (mut theirs, others): (Vec<PoolClaim>, Vec<PoolClaim>) = std::mem::take(&mut self.claims).into_iter().partition(|kept| kept.pool == pool && kept.to == to);
        theirs.retain(|kept| kept.height != claim.height);
        theirs.push(claim);
        theirs.sort_by_key(|claim| std::cmp::Reverse(claim.height));

        let is_settled = |claim: &PoolClaim| settled.is_some_and(|settled| claim.height <= settled);
        let (done, mut pending): (Vec<PoolClaim>, Vec<PoolClaim>) = theirs.into_iter().partition(is_settled);

        if pending.len() > KEPT_PENDING {
            let oldest = pending.pop().expect("more than any");
            pending.truncate(KEPT_PENDING - 1);
            pending.push(oldest);
        }

        self.claims = others;
        self.claims.extend(pending);
        self.claims.extend(done.into_iter().take(KEPT_SETTLED));
        self.claims.sort_by(in_order);

        (self.claims.clone(), self.settled) != before
    }

    #[document(parent = "SavedClaims", summary = "The claims kept for a miner, on one pool or any, newest first within each pool.")]
    pub fn of<'a>(&'a self, pool: Option<&'a Address>, to: &'a Address) -> impl Iterator<Item = &'a PoolClaim> + 'a {
        self.claims.iter().filter(move |claim| claim.to == *to && pool.is_none_or(|pool| claim.pool == *pool))
    }

    #[document(
        parent = "SavedClaims",
        summary = "The newest claim kept for a miner on one pool, which pays the most there should it hold. Claims on different pools pay from different escrows: their heights say nothing of which pays more.",
    )]
    pub fn newest(&self, pool: &Address, to: &Address) -> Option<&PoolClaim> {
        self.claims.iter().filter(|claim| claim.pool == *pool && claim.to == *to).max_by_key(|claim| claim.height)
    }

    #[document(parent = "SavedClaims", summary = "The highest settled height told so far, or None.")]
    pub fn settled(&self) -> Option<u64> {
        self.settled
    }

    #[document(parent = "SavedClaims", summary = "Claims kept before, as they were, with the highest settled height told then: none forgotten, as keeping them did that already.")]
    pub fn kept(mut claims: Vec<PoolClaim>, settled: Option<u64>) -> SavedClaims {
        claims.sort_by(in_order);

        SavedClaims { claims, settled }
    }

    #[document(parent = "SavedClaims", summary = "Every claim kept.")]
    pub fn all(&self) -> &[PoolClaim] {
        &self.claims
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::db::MemoryDb, std::sync::Arc};

    fn state() -> State {
        State::new(Arc::new(MemoryDb::default()))
    }

    #[test]
    fn a_miner_claims_what_it_earned_less_what_it_claimed_from_any_root_of_its_pool() {
        let hash = HashType::default_chain();
        let state = state();
        let pools = Pools::new(&state, hash);
        let (pool, alice, bob) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]), Address::Ed25519([3; 32]));
        let escrow = pool_escrow(&pool, hash);
        Accounts::new(&state).credit(&escrow, u256::new(1_000)).unwrap();

        // The pool's first root: alice earned 300, bob 100.
        let first = [claim_leaf(hash, &alice, u256::new(300)), claim_leaf(hash, &bob, u256::new(100))];
        let (one, two) = (1, 2);
        pools.commit(one, PoolRoot { pool, root: merkle::root(hash, &first), count: 2 });

        assert_eq!(pools.claim(one, &alice, u256::new(300), 0, &merkle::audit_path(hash, &first, 0)), Ok(u256::new(300)));
        assert_eq!(pools.claim(one, &alice, u256::new(300), 0, &merkle::audit_path(hash, &first, 0)), Err(ClaimError::Claimed));
        assert_eq!(pools.claim(one, &alice, u256::new(999), 0, &merkle::audit_path(hash, &first, 0)), Err(ClaimError::BadPath), "more than the leaf says");
        assert_eq!(pools.claim(one, &bob, u256::new(100), 0, &merkle::audit_path(hash, &first, 0)), Err(ClaimError::BadPath), "another's place");
        assert_eq!(pools.claim(two, &bob, u256::new(100), 1, &merkle::audit_path(hash, &first, 1)), Err(ClaimError::NoRoot));

        // A later root: alice earned 500 in all, so 200 more.
        let later = [claim_leaf(hash, &alice, u256::new(500)), claim_leaf(hash, &bob, u256::new(100))];
        pools.commit(two, PoolRoot { pool, root: merkle::root(hash, &later), count: 2 });

        assert_eq!(pools.claim(two, &alice, u256::new(500), 0, &merkle::audit_path(hash, &later, 0)), Ok(u256::new(200)));
        assert_eq!(pools.claim(one, &bob, u256::new(100), 1, &merkle::audit_path(hash, &first, 1)), Ok(u256::new(100)), "any root of its pool");
        assert_eq!(pools.claimed(&pool, &alice), Ok(u256::new(500)));
        assert_eq!(Accounts::new(&state).get(&escrow).unwrap().balance, u256::new(400));
        assert_eq!(Accounts::new(&state).get(&alice).unwrap().balance, u256::new(500));
    }

    #[test]
    fn a_claim_past_what_the_escrow_holds_pays_nothing() {
        let hash = HashType::default_chain();
        let state = state();
        let pools = Pools::new(&state, hash);
        let (pool, alice) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]));
        let leaves = [claim_leaf(hash, &alice, u256::new(50))];
        pools.commit(1, PoolRoot { pool, root: merkle::root(hash, &leaves), count: 1 });
        Accounts::new(&state).credit(&pool_escrow(&pool, hash), u256::new(49)).unwrap();

        assert_eq!(pools.claim(1, &alice, u256::new(50), 0, &[]), Err(ClaimError::Escrow));
        assert_eq!(pools.claimed(&pool, &alice), Ok(u256::ZERO), "nothing changed");
    }

    fn claim(pool: Address, to: Address, height: u64) -> PoolClaim {
        PoolClaim { pool, height, block: Hash::zero(HashType::default_chain()), to, earned: u256::new(u128::from(height)), index: 0, count: 1, path: Vec::new() }
    }

    #[test]
    fn a_saved_claim_holds_under_its_pools_root_and_no_other() {
        let hash = HashType::default_chain();
        let (pool, alice, bob) = (Address::Ed25519([1; 32]), Address::Ed25519([2; 32]), Address::Ed25519([3; 32]));
        let leaves = [claim_leaf(hash, &alice, u256::new(300)), claim_leaf(hash, &bob, u256::new(100))];
        let root = PoolRoot { pool, root: merkle::root(hash, &leaves), count: 2 };
        let saved = PoolClaim { pool, height: 7, block: Hash::zero(hash), to: bob, earned: u256::new(100), index: 1, count: 2, path: merkle::audit_path(hash, &leaves, 1) };

        assert!(saved.holds(&root, hash));
        assert!(!PoolClaim { earned: u256::new(101), ..saved.clone() }.holds(&root, hash), "more than its leaf");
        assert!(!saved.holds(&PoolRoot { pool: alice, ..root }, hash), "another pool's root");
        assert!(!saved.holds(&PoolRoot { root: leaves[0], ..root }, hash), "a root its block was undone for");
        assert_eq!(saved.tx(), Tx::Claim { height: 7, to: bob, earned: u256::new(100), index: 1, path: saved.path.clone() });

        // It claims on a chain as the pool's node would have.
        let state = state();
        let pools = Pools::new(&state, hash);
        pools.commit(7, root);
        Accounts::new(&state).credit(&pool_escrow(&pool, hash), u256::new(100)).unwrap();
        assert_eq!(pools.claim(saved.height, &saved.to, saved.earned, saved.index, &saved.path), Ok(u256::new(100)));
    }

    #[test]
    fn a_miner_keeps_its_newest_claims_the_next_to_settle_and_the_newest_settled() {
        let (pool, other, alice, bob) = (Address::Ed25519([1; 32]), Address::Ed25519([4; 32]), Address::Ed25519([2; 32]), Address::Ed25519([3; 32]));
        let mut saved = SavedClaims::default();

        // Nothing settled yet: all kept, up to the newest and the oldest.
        for height in 1..=100 {
            assert!(saved.keep(claim(pool, alice, height), None));
        }

        let heights: Vec<u64> = saved.of(Some(&pool), &alice).map(|claim| claim.height).collect();
        assert_eq!(heights.len(), KEPT_PENDING);
        assert_eq!((heights[0], heights[KEPT_PENDING - 2], heights[KEPT_PENDING - 1]), (100, 70, 1), "the newest, and the oldest, which settles first");

        // Bob's and another pool's are kept beside alice's, each by its own.
        saved.keep(claim(pool, bob, 50), None);
        saved.keep(claim(other, alice, 60), Some(60));
        assert_eq!(saved.of(Some(&pool), &bob).count(), 1);
        assert_eq!(saved.newest(&pool, &alice).map(|claim| claim.height), Some(100));

        // The same claim again changes nothing; one at its height from another block takes its place.
        assert!(!saved.keep(claim(pool, alice, 100), None));
        assert!(saved.keep(PoolClaim { block: Hash::from_bytes(HashType::default_chain(), &[9; 32]).unwrap(), ..claim(pool, alice, 100) }, None));
        assert_eq!(saved.of(Some(&pool), &alice).count(), KEPT_PENDING);

        // Once 75 is settled, the settled ones down to the newest two go.
        saved.keep(claim(pool, alice, 101), Some(75));
        let heights: Vec<u64> = saved.of(Some(&pool), &alice).map(|claim| claim.height).collect();
        assert_eq!(heights.first(), Some(&101));
        assert_eq!(&heights[heights.len() - 2..], &[75, 74], "the newest settled, and one before it");
        assert!(heights.iter().filter(|height| **height <= 75).count() == KEPT_SETTLED);
        assert_eq!(saved.of(Some(&other), &alice).count(), 1, "another pool's, untouched");

        // A claim told with no settled height still knows 75 is: the newest settled stays.
        saved.keep(claim(pool, alice, 102), None);
        let heights: Vec<u64> = saved.of(Some(&pool), &alice).map(|claim| claim.height).collect();
        assert_eq!(saved.settled(), Some(75));
        assert_eq!(&heights[heights.len() - 2..], &[75, 74], "not trimmed away as though it were pending");
    }
}
