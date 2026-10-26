//! The state tree: a sparse Merkle tree over every chain-state entry, whose root each block header
//! commits to as its state_root.
//!
//! Each entry sits at a path, the chain hash of its column and key, and the tree is binary over
//! the path's bits. It is kept canonical, so its root depends only on the entries: an empty
//! subtree has no node and hashes to zero, a subtree of one entry is that entry's leaf, as high
//! up as it can sit, and anything more is a branch over its two halves. Nodes are stored by their
//! place in Column::StateTree, written in the same state layer as the block that changed them, so
//! a commit, a throwaway layer and a reorg's undo record treat them like any other write.

use {
    super::{Column, Database, DbError, State},
    crate::crypto::hasher::Hasher,
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "The columns the state root commits to: what every node computes the same from the blocks. Blocks, receipts, the event index and what finality learned are not among them.",
    category = "Storage",
    see_also = "StateTree, Column",
)]
pub const STATE_COLUMNS: [Column; 8] = [
    Column::Accounts,
    Column::Contracts,
    Column::Storage,
    Column::Validators,
    Column::Epochs,
    Column::Tokens,
    Column::Namespaces,
    Column::Names,
];

/// Separates entry paths from every other hash the chain takes.
const PATH_DOMAIN: &[u8] = b"chainv4 state path";
/// What a leaf's and a branch's hash start with, so neither can pass for the other.
const LEAF: u8 = 0;
const BRANCH: u8 = 1;
/// The bits of a path, and so the deepest a leaf can sit.
const PATH_BITS: u16 = 256;

#[document(summary = "Where an entry sits in the state tree: the chain hash of its column and key.", category = "Storage", see_also = "StateTree")]
pub type Path = [u8; 32];

/// A node as stored.
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
enum Node {
    /// The one entry of its subtree.
    #[wincode(tag = 0)]
    Leaf(Leaf),
    /// Two or more entries: the hashes of its halves, zero for an empty one.
    #[wincode(tag = 1)]
    Branch { left: Hash, right: Hash },
}

/// An entry as its leaf holds it: its path and the hash of its value, which its leaf's hash is
/// made of, and the column and key it is in, so the tree can say which entries a subtree holds.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, SchemaWrite, SchemaRead)]
struct Leaf {
    path: Path,
    value: Hash,
    column: Column,
    key: Vec<u8>,
}

impl Leaf {
    fn hash(&self) -> Hash {
        leaf_hash(&self.path, &self.value)
    }
}

/// A change to one entry: its new value's hash, or None for a delete.
struct Change {
    path: Path,
    value: Option<Hash>,
    column: Column,
    key: Vec<u8>,
}

#[document(
    summary = "The path of an entry: the chain hash of a domain tag, its column and its key.",
    params = {
        hash = "The chain's hash: a 256-bit one.",
        column = "One of STATE_COLUMNS.",
        key = "The entry's key in its column.",
    },
    category = "Storage",
)]
pub fn state_path(hash: HashType, column: Column, key: &[u8]) -> Path {
    let mut hasher = Hasher::new(hash);
    hasher.update(PATH_DOMAIN);
    hasher.update(&wincode::serialize(&column).expect("columns always encode"));
    hasher.update(key);

    hasher.finalize().to_bytes().try_into().expect("the chain hash is 256 bits")
}

fn leaf_hash(path: &Path, value: &Hash) -> Hash {
    let mut hasher = Hasher::new(value.algorithm());
    hasher.update(&[LEAF]);
    hasher.update(path);
    hasher.update(&value.to_bytes());

    hasher.finalize()
}

fn branch_hash(left: &Hash, right: &Hash) -> Hash {
    let mut hasher = Hasher::new(left.algorithm());
    hasher.update(&[BRANCH]);
    hasher.update(&left.to_bytes());
    hasher.update(&right.to_bytes());

    hasher.finalize()
}

/// The bit of a path at a depth, 0 for the left half.
fn bit(path: &Path, depth: u16) -> u8 {
    let depth = usize::from(depth);

    (path[depth / 8] >> (7 - depth % 8)) & 1
}

/// The prefix with the bit at depth set: the right half's place.
fn with_bit(mut prefix: Path, depth: u16) -> Path {
    let depth = usize::from(depth);
    prefix[depth / 8] |= 1 << (7 - depth % 8);

    prefix
}

/// Where a node is stored: its depth, then the path bits above it.
fn node_key(depth: u16, prefix: &Path) -> Vec<u8> {
    let mut key = depth.to_be_bytes().to_vec();
    key.extend_from_slice(prefix);

    key
}

#[document(
    summary = "The root over a set of entries, worked out from them alone: what StateTree keeps as its root for the same entries. For checking a snapshot, or the tree itself.",
    params = {
        hash = "The chain's hash.",
        entries = "Every entry, as its column, key and value.",
    },
    category = "Storage",
    see_also = "StateTree::root",
)]
pub fn root_of<'a>(hash: HashType, entries: impl IntoIterator<Item = (Column, &'a [u8], &'a [u8])>) -> Hash {
    let mut leaves: Vec<(Path, Hash)> = entries
        .into_iter()
        .map(|(column, key, value)| (state_path(hash, column, key), Hasher::digest(hash, value)))
        .collect();
    leaves.sort();

    subtree(hash, 0, &leaves)
}

/// The hash of the canonical subtree over sorted leaves that share the path bits above depth.
fn subtree(hash: HashType, depth: u16, leaves: &[(Path, Hash)]) -> Hash {
    match leaves {
        [] => Hash::zero(hash),
        [(path, value)] => leaf_hash(path, value),
        _ => {
            let split = leaves.partition_point(|(path, _)| bit(path, depth) == 0);

            branch_hash(&subtree(hash, depth + 1, &leaves[..split]), &subtree(hash, depth + 1, &leaves[split..]))
        }
    }
}

#[document(
    summary = "The root over every chain-state entry a database holds, read from the database alone rather than the tree: what a node can check its tree, or a snapshot, against.",
    params = {
        db = "The database.",
        hash = "The chain's hash.",
    },
    category = "Storage",
    see_also = "root_of, STATE_COLUMNS",
)]
pub fn stored_root(db: &dyn Database, hash: HashType) -> Result<Hash, DbError> {
    let mut entries = Vec::new();

    for column in STATE_COLUMNS {
        db.scan(column, &[], &[], &mut |key, value| {
            entries.push((column, key.to_vec(), value.to_vec()));

            true
        })?;
    }

    Ok(root_of(hash, entries.iter().map(|(column, key, value)| (*column, key.as_slice(), value.as_slice()))))
}

#[document(
    summary = "The state tree as a state layer holds it: its root, how it follows a block's changes, and proofs of what it holds.",
    category = "Storage",
    see_also = "STATE_COLUMNS, StateProof, BlockHeader",
    example = "let tree = StateTree::new(&layer, hash);\nlet root = tree.follow()?;\nlet proof = tree.prove(Column::Accounts, &address_key(&address))?;",
)]
pub struct StateTree<'a> {
    state: &'a State,
    hash: HashType,
}

impl<'a> StateTree<'a> {
    #[document(parent = "StateTree", summary = "The tree in this layer, hashed with the chain's hash.")]
    pub fn new(state: &'a State, hash: HashType) -> StateTree<'a> {
        StateTree { state, hash }
    }

    #[document(parent = "StateTree", summary = "The root: zero for a tree that holds nothing.")]
    pub fn root(&self) -> Result<Hash, DbError> {
        self.hash_at(0, &[0; 32])
    }

    #[document(
        parent = "StateTree",
        summary = "Brings the tree in line with every chain-state entry the layer changed, writing the nodes that change in the same layer, and returns the new root. Run it once, after everything the block does to state and before its changes are taken.",
    )]
    pub fn follow(&self) -> Result<Hash, DbError> {
        let mut changes: Vec<Change> = self
            .state
            .changed(&STATE_COLUMNS)
            .into_iter()
            .map(|(column, key, value)| Change {
                path: state_path(self.hash, column, &key),
                value: value.map(|value| Hasher::digest(self.hash, &value)),
                column,
                key,
            })
            .collect();
        changes.sort_by_key(|change| change.path);

        self.update(0, [0; 32], &changes)
    }

    #[document(
        parent = "StateTree",
        summary = "A proof of what the tree holds for an entry, a value or nothing, against its root.",
        params = {
            column = "One of STATE_COLUMNS.",
            key = "The entry's key in its column.",
        },
        see_also = "StateProof::verify",
    )]
    pub fn prove(&self, column: Column, key: &[u8]) -> Result<StateProof, DbError> {
        let path = state_path(self.hash, column, key);
        let (mut depth, mut prefix, mut siblings) = (0, [0; 32], Vec::new());

        loop {
            match self.node(depth, &prefix)? {
                None => return Ok(StateProof { siblings, leaf: None }),
                Some(Node::Leaf(leaf)) => return Ok(StateProof { siblings, leaf: Some((leaf.path, leaf.value)) }),
                Some(Node::Branch { left, right }) => {
                    match bit(&path, depth) {
                        0 => siblings.push(right),
                        _ => {
                            siblings.push(left);
                            prefix = with_bit(prefix, depth);
                        }
                    }

                    depth += 1;
                }
            }
        }
    }

    #[document(
        parent = "StateTree",
        summary = "One position of the tree as a node serves it to another syncing from a snapshot: the hashes beside the way down to it, and its entries when there are at most limit of them, or else its two halves' hashes, to ask for one at a time.",
        params = {
            depth = "How deep the position is: 0 for the root.",
            prefix = "The path bits above it; the rest are zero.",
            limit = "The most entries to send at once.",
        },
        returns = "The part, or None where the tree has no node at that position, such as below a leaf.",
        see_also = "StatePart::verify",
    )]
    pub fn part(&self, depth: u16, prefix: &Path, limit: usize) -> Result<Option<StatePart>, DbError> {
        let mut siblings = Vec::with_capacity(usize::from(depth));
        let mut at = [0; 32];

        for level in 0..depth {
            let Some(Node::Branch { left, right }) = self.node(level, &at)? else {
                return Ok(None);
            };

            match bit(prefix, level) {
                0 => siblings.push(right),
                _ => {
                    siblings.push(left);
                    at = with_bit(at, level);
                }
            }
        }

        let contents = match self.node(depth, &at)? {
            None => return Ok(None),
            Some(Node::Branch { left, right }) => match self.leaves_under(depth, &at, limit)? {
                Some(leaves) => PartContents::Entries(self.entries(leaves)?),
                None => PartContents::Branch { left, right },
            },
            Some(Node::Leaf(leaf)) => PartContents::Entries(self.entries(vec![leaf])?),
        };

        Ok(Some(StatePart { siblings, contents }))
    }

    #[document(
        parent = "StateTree",
        summary = "The hash of whatever this tree holds under a position, whether or not it has a node there: what a node syncing from a snapshot compares with a peer's, to skip what it holds already.",
    )]
    pub fn subtree_hash(&self, depth: u16, prefix: &Path) -> Result<Hash, DbError> {
        match self.covering(depth, prefix)? {
            Covering::Node(node) => Ok(node_hash(&node)),
            Covering::Leaf(leaf) => Ok(leaf.hash()),
            Covering::Nothing => Ok(Hash::zero(self.hash)),
        }
    }

    #[document(
        parent = "StateTree",
        summary = "Every entry this tree holds under a position, as its column and key.",
    )]
    pub fn entries_under(&self, depth: u16, prefix: &Path) -> Result<Vec<(Column, Vec<u8>)>, DbError> {
        let leaves = match self.covering(depth, prefix)? {
            Covering::Node(_) => self.leaves_under(depth, prefix, usize::MAX)?.unwrap_or_default(),
            Covering::Leaf(leaf) => vec![leaf],
            Covering::Nothing => Vec::new(),
        };

        Ok(leaves.into_iter().map(|leaf| (leaf.column, leaf.key)).collect())
    }

    /// What holds the position: its own node, a leaf higher up whose path runs through it, or
    /// nothing.
    fn covering(&self, depth: u16, prefix: &Path) -> Result<Covering, DbError> {
        let mut at = [0; 32];

        for level in 0..=depth {
            match self.node(level, &at)? {
                None => return Ok(Covering::Nothing),
                Some(node) if level == depth => return Ok(Covering::Node(node)),
                Some(Node::Leaf(leaf)) => {
                    let through = (level..depth).all(|below| bit(&leaf.path, below) == bit(prefix, below));

                    return Ok(match through {
                        true => Covering::Leaf(leaf),
                        false => Covering::Nothing,
                    });
                }
                Some(Node::Branch { .. }) => {
                    if bit(prefix, level) == 1 {
                        at = with_bit(at, level);
                    }
                }
            }
        }

        Ok(Covering::Nothing)
    }

    /// The leaves under a position that has a node, if there are at most limit of them.
    fn leaves_under(&self, depth: u16, prefix: &Path, limit: usize) -> Result<Option<Vec<Leaf>>, DbError> {
        let mut leaves = Vec::new();
        let mut waiting = vec![(depth, *prefix)];

        while let Some((depth, prefix)) = waiting.pop() {
            match self.node(depth, &prefix)? {
                None => {}
                Some(Node::Leaf(leaf)) => {
                    if leaves.len() == limit {
                        return Ok(None);
                    }

                    leaves.push(leaf);
                }
                Some(Node::Branch { .. }) => {
                    waiting.push((depth + 1, with_bit(prefix, depth)));
                    waiting.push((depth + 1, prefix));
                }
            }
        }

        leaves.sort();

        Ok(Some(leaves))
    }

    /// The entries of leaves, with their values read from state.
    fn entries(&self, leaves: Vec<Leaf>) -> Result<Vec<StateEntry>, DbError> {
        leaves
            .into_iter()
            .map(|leaf| {
                let value = self
                    .state
                    .get(leaf.column, &leaf.key)?
                    .ok_or_else(|| DbError::Corrupt("the state tree holds an entry its column does not".to_string()))?;

                Ok(StateEntry {
                    column: leaf.column,
                    key: leaf.key,
                    value,
                })
            })
            .collect()
    }

    /// Applies changes, sorted by path, to the subtree at depth whose place is prefix, and returns
    /// its new hash.
    fn update(&self, depth: u16, prefix: Path, changes: &[Change]) -> Result<Hash, DbError> {
        if changes.is_empty() {
            return self.hash_at(depth, &prefix);
        }

        let node = self.node(depth, &prefix)?;

        if let Some(Node::Branch { left, right }) = node {
            let split = changes.partition_point(|change| bit(&change.path, depth) == 0);
            let (low, high) = changes.split_at(split);
            let left = match low.is_empty() {
                true => left,
                false => self.update(depth + 1, prefix, low)?,
            };
            let right = match high.is_empty() {
                true => right,
                false => self.update(depth + 1, with_bit(prefix, depth), high)?,
            };

            return self.settle(depth, prefix, left, right);
        }

        // Nothing or one leaf here, so nothing below: what the subtree will hold is the changes'
        // values, and the leaf unless a change replaces it.
        let existed = node.is_some();
        let mut leaves: Vec<Leaf> = changes
            .iter()
            .filter_map(|change| {
                change.value.map(|value| Leaf {
                    path: change.path,
                    value,
                    column: change.column,
                    key: change.key.clone(),
                })
            })
            .collect();

        if let Some(Node::Leaf(leaf)) = node
            && changes.binary_search_by(|change| change.path.cmp(&leaf.path)).is_err()
        {
            leaves.push(leaf);
            leaves.sort();
        }

        if leaves.is_empty() {
            if existed {
                self.state.delete(Column::StateTree, node_key(depth, &prefix));
            }

            return Ok(Hash::zero(self.hash));
        }

        Ok(self.build(depth, prefix, leaves))
    }

    /// Writes the canonical subtree over sorted leaves, at least one, where nothing is below.
    fn build(&self, depth: u16, prefix: Path, mut leaves: Vec<Leaf>) -> Hash {
        if leaves.len() == 1 {
            let leaf = leaves.pop().expect("one leaf");
            let hash = leaf.hash();
            self.write(depth, &prefix, &Node::Leaf(leaf));

            return hash;
        }

        assert!(depth < PATH_BITS, "two leaves never share a path");
        let split = leaves.partition_point(|leaf| bit(&leaf.path, depth) == 0);
        let high = leaves.split_off(split);
        let side = |prefix: Path, leaves: Vec<Leaf>| match leaves.is_empty() {
            true => Hash::zero(self.hash),
            false => self.build(depth + 1, prefix, leaves),
        };
        let left = side(prefix, leaves);
        let right = side(with_bit(prefix, depth), high);
        self.write(depth, &prefix, &Node::Branch { left, right });

        branch_hash(&left, &right)
    }

    /// Puts a branch whose halves changed back in canonical form: gone when both are empty, the
    /// leaf of one half moved up when the other is empty, a branch otherwise.
    fn settle(&self, depth: u16, prefix: Path, left: Hash, right: Hash) -> Result<Hash, DbError> {
        let zero = Hash::zero(self.hash);

        if left == zero && right == zero {
            self.state.delete(Column::StateTree, node_key(depth, &prefix));

            return Ok(zero);
        }

        if left == zero || right == zero {
            let half = match left == zero {
                true => with_bit(prefix, depth),
                false => prefix,
            };

            if let Some(Node::Leaf(leaf)) = self.node(depth + 1, &half)? {
                let hash = leaf.hash();
                self.state.delete(Column::StateTree, node_key(depth + 1, &half));
                self.write(depth, &prefix, &Node::Leaf(leaf));

                return Ok(hash);
            }
        }

        self.write(depth, &prefix, &Node::Branch { left, right });

        Ok(branch_hash(&left, &right))
    }

    fn hash_at(&self, depth: u16, prefix: &Path) -> Result<Hash, DbError> {
        Ok(match self.node(depth, prefix)? {
            None => Hash::zero(self.hash),
            Some(node) => node_hash(&node),
        })
    }

    fn node(&self, depth: u16, prefix: &Path) -> Result<Option<Node>, DbError> {
        self.state.get_value(Column::StateTree, &node_key(depth, prefix))
    }

    fn write(&self, depth: u16, prefix: &Path, node: &Node) {
        self.state.put(Column::StateTree, node_key(depth, prefix), wincode::serialize(node).expect("nodes always encode"));
    }
}

fn node_hash(node: &Node) -> Hash {
    match node {
        Node::Leaf(leaf) => leaf.hash(),
        Node::Branch { left, right } => branch_hash(left, right),
    }
}

/// What holds a position in the tree.
enum Covering {
    Node(Node),
    Leaf(Leaf),
    Nothing,
}

#[document(summary = "One chain-state entry: its column, key and value.", category = "Storage", see_also = "StatePart")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct StateEntry {
    #[document(summary = "One of STATE_COLUMNS.")]
    pub column: Column,
    #[document(summary = "The key.")]
    pub key: Vec<u8>,
    #[document(summary = "The value.")]
    pub value: Vec<u8>,
}

#[document(
    summary = "A position of the state tree as a node serves it to one syncing from a snapshot: the hashes beside the way down to it, the root's first, and what is there.",
    category = "Storage",
    see_also = "StateTree::part",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct StatePart {
    #[document(summary = "The other half's hash at each branch on the way down.")]
    pub siblings: Vec<Hash>,
    #[document(summary = "What the position holds.")]
    pub contents: PartContents,
}

#[document(summary = "What a position of the state tree holds, as a StatePart sends it.", category = "Storage", see_also = "StatePart")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum PartContents {
    #[document(summary = "Every entry under it, in path order: few enough to send at once.")]
    #[wincode(tag = 0)]
    Entries(Vec<StateEntry>),
    #[document(summary = "Too many to send: the hashes of its two halves, to ask for one at a time.")]
    #[wincode(tag = 1)]
    Branch { left: Hash, right: Hash },
}

impl StatePart {
    #[document(
        parent = "StatePart",
        summary = "Whether the part is what a tree with this root holds at this position: its entries all sit under it and hash to what the siblings fold up to the root.",
        params = {
            root = "The pivot block's state_root.",
            depth = "The position's depth.",
            prefix = "Its path bits.",
        },
    )]
    pub fn verify(&self, root: &Hash, depth: u16, prefix: &Path) -> bool {
        let hash = root.algorithm();

        if hash.digest_len() != 32 || self.siblings.len() != usize::from(depth) {
            return false;
        }

        let mut node = match &self.contents {
            PartContents::Branch { left, right } => branch_hash(left, right),
            PartContents::Entries(entries) => {
                let mut leaves: Vec<(Path, Hash)> = entries
                    .iter()
                    .map(|entry| (state_path(hash, entry.column, &entry.key), Hasher::digest(hash, &entry.value)))
                    .collect();
                leaves.sort();

                let under = leaves.iter().all(|(path, _)| (0..depth).all(|at| bit(path, at) == bit(prefix, at)));
                let state = entries.iter().all(|entry| STATE_COLUMNS.contains(&entry.column));
                let distinct = leaves.windows(2).all(|pair| pair[0].0 != pair[1].0);

                if !under || !state || !distinct {
                    return false;
                }

                subtree(hash, depth, &leaves)
            }
        };

        for (at, sibling) in self.siblings.iter().enumerate().rev() {
            node = match bit(prefix, at as u16) {
                0 => branch_hash(&node, sibling),
                _ => branch_hash(sibling, &node),
            };
        }

        node == *root
    }
}

#[document(
    summary = "A proof of what the state tree holds for one entry: the hashes beside its path, from the root down, to where the path ends, and the leaf there, if any. It proves a value, or, ending empty or at another entry's leaf, that there is none.",
    category = "Storage",
    see_also = "StateTree::prove",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct StateProof {
    #[document(summary = "The other half's hash at each branch on the way down, the root's first.")]
    pub siblings: Vec<Hash>,
    #[document(summary = "The leaf where the path ends, as its path and the hash of its value; None where it ends empty.")]
    pub leaf: Option<(Path, Hash)>,
}

impl StateProof {
    #[document(
        parent = "StateProof",
        summary = "Whether the proof shows that, under this root, the entry holds this value, or with None, that it holds nothing. What a light client checks a value with, needing only a header it trusts.",
        params = {
            root = "A block's state_root.",
            column = "The entry's column.",
            key = "Its key.",
            value = "What it should hold, or None for nothing.",
        },
    )]
    pub fn verify(&self, root: &Hash, column: Column, key: &[u8], value: Option<&[u8]>) -> bool {
        let hash = root.algorithm();

        if hash.digest_len() != 32 || self.siblings.len() > usize::from(PATH_BITS) {
            return false;
        }

        let path = state_path(hash, column, key);
        let depth = self.siblings.len() as u16;
        let shares = |other: &Path| (0..depth).all(|at| bit(other, at) == bit(&path, at));
        let mut node = match (value, &self.leaf) {
            (Some(value), Some((leaf, held))) if *leaf == path && *held == Hasher::digest(hash, value) => leaf_hash(leaf, held),
            (None, None) => Hash::zero(hash),
            (None, Some((leaf, held))) if *leaf != path && shares(leaf) => leaf_hash(leaf, held),
            _ => return false,
        };

        for (at, sibling) in self.siblings.iter().enumerate().rev() {
            node = match bit(&path, at as u16) {
                0 => branch_hash(&node, sibling),
                _ => branch_hash(sibling, &node),
            };
        }

        node == *root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::MemoryDb;
    use std::sync::Arc;

    type Write = (Column, Vec<u8>, Option<Vec<u8>>);

    fn hash() -> HashType {
        HashType::default_chain()
    }

    fn fresh() -> Arc<State> {
        Arc::new(State::new(Arc::new(MemoryDb::default())))
    }

    fn set(column: Column, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Write {
        (column, key.as_ref().to_vec(), Some(value.as_ref().to_vec()))
    }

    fn unset(column: Column, key: impl AsRef<[u8]>) -> Write {
        (column, key.as_ref().to_vec(), None)
    }

    /// Makes writes in a layer above state, follows them, and keeps the layer.
    fn change(state: &Arc<State>, writes: Vec<Write>) -> Hash {
        let layer = State::new(state.clone());

        for (column, key, value) in writes {
            match value {
                Some(value) => layer.put(column, key, value),
                None => layer.delete(column, key),
            }
        }

        let root = StateTree::new(&layer, hash()).follow().unwrap();
        state.merge(layer.take_changes());

        root
    }

    fn keys(count: u32) -> Vec<Vec<u8>> {
        (0..count).map(|n| n.to_be_bytes().to_vec()).collect()
    }

    #[test]
    fn the_root_depends_on_the_entries_alone() {
        let entries = keys(40);
        let (one, other) = (fresh(), fresh());

        // All at once, and one at a time backwards with others written and deleted on the way.
        let at_once = change(&one, entries.iter().map(|key| set(Column::Storage, key, key)).collect());

        for key in entries.iter().rev() {
            change(&other, vec![set(Column::Storage, key, key), set(Column::Accounts, "passing", "through")]);
            change(&other, vec![unset(Column::Accounts, "passing")]);
        }

        let expected = root_of(hash(), entries.iter().map(|key| (Column::Storage, key.as_slice(), key.as_slice())));

        assert_eq!(at_once, expected);
        assert_eq!(StateTree::new(&other, hash()).root().unwrap(), expected);
        assert_ne!(expected, Hash::zero(hash()));

        // Deleting everything leaves nothing behind, nodes included.
        assert_eq!(change(&one, entries.iter().map(|key| unset(Column::Storage, key)).collect()), Hash::zero(hash()));
        assert!(
            one.take_changes().iter().all(|((column, _), value)| *column != Column::StateTree || value.is_none()),
            "no node outlives its entries"
        );
    }

    #[test]
    fn values_and_columns_change_the_root() {
        let state = fresh();
        let first = change(&state, vec![set(Column::Storage, "k", "1")]);

        assert_ne!(change(&state, vec![set(Column::Storage, "k", "2")]), first);
        assert_eq!(change(&state, vec![set(Column::Storage, "k", "1")]), first);
        assert_ne!(root_of(hash(), [(Column::Accounts, &b"k"[..], &b"1"[..])]), first, "the column is part of the path");
        assert_eq!(change(&state, vec![set(Column::Receipts, "k", "1")]), first, "only chain state counts");
    }

    #[test]
    fn proofs_show_what_is_held_and_what_is_not() {
        let state = fresh();
        let entries = keys(25);
        let root = change(&state, entries.iter().map(|key| set(Column::Accounts, key, "held")).collect());
        let tree = StateTree::new(&state, hash());

        for key in &entries {
            let proof = tree.prove(Column::Accounts, key).unwrap();

            assert!(proof.verify(&root, Column::Accounts, key, Some(b"held")));
            assert!(!proof.verify(&root, Column::Accounts, key, Some(b"other")), "a value it does not hold");
            assert!(!proof.verify(&root, Column::Accounts, key, None), "nothing, where something is");
            assert!(!proof.verify(&root, Column::Storage, key, Some(b"held")), "another column");
            assert!(!proof.verify(&Hash::zero(hash()), Column::Accounts, key, Some(b"held")), "another root");
        }

        for missing in [&b"missing"[..], b"absent", b"nowhere"] {
            let proof = tree.prove(Column::Accounts, missing).unwrap();

            assert!(proof.verify(&root, Column::Accounts, missing, None));
            assert!(!proof.verify(&root, Column::Accounts, missing, Some(b"held")));
        }

        // A proof of one entry used for another, or with a sibling changed, fails.
        let mut proof = tree.prove(Column::Accounts, &entries[3]).unwrap();
        let other = tree.prove(Column::Accounts, &entries[4]).unwrap();

        assert!(!other.verify(&root, Column::Accounts, &entries[3], Some(b"held")));
        proof.siblings[0] = Hash::zero(hash());
        assert!(!proof.verify(&root, Column::Accounts, &entries[3], Some(b"held")));

        // The empty tree proves nothing is there.
        let empty = StateTree::new(&fresh(), hash()).prove(Column::Accounts, b"k").unwrap();

        assert!(empty.verify(&Hash::zero(hash()), Column::Accounts, b"k", None));
    }

    #[test]
    fn the_stored_root_reads_the_entries_back() {
        let db = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());
        state.put(Column::Accounts, b"a".to_vec(), b"1".to_vec());
        state.put(Column::Namespaces, b"n".to_vec(), b"2".to_vec());
        state.put(Column::Blocks, b"b".to_vec(), b"3".to_vec());
        let root = StateTree::new(&state, hash()).follow().unwrap();
        db.commit(state.take_changes()).unwrap();

        assert_eq!(stored_root(&*db, hash()).unwrap(), root);
    }
}
