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
}
