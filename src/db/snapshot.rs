//! Taking chain state from peers instead of running every block: a walk down the state tree of a
//! pivot block, whose state root the node trusts from its header. It asks for one position at a
//! time, checks every part it gets against the root before it keeps it, and skips any subtree
//! whose hash matches what it holds already. So a walk that has to start again at a newer pivot,
//! once its peers no longer serve the old one, only fetches what changed in between.

use {
    super::{Column, DbError, Path, PartContents, StatePart, State, StateTree},
    crate::structs::hash::{Hash, HashType},
    r#macro::document,
    std::collections::{BTreeMap, BTreeSet, VecDeque},
    std::fmt,
};

#[document(
    summary = "A walk down a pivot block's state tree, bringing a node's own state in line with it: sans-IO, so the node asks peers for the positions it names, in whatever way it talks to them.",
    category = "Storage",
    see_also = "StatePart, StateTree::part, StateTree::subtree_hash",
    example = "let mut walk = SnapshotWalk::new(pivot.state_root);\nwhile let Some((depth, prefix)) = walk.next(&StateTree::new(&state, hash))? {\n    let part = ask_a_peer(depth, prefix);\n    walk.take(&layer, hash, depth, prefix, part)?;\n}",
)]
pub struct SnapshotWalk {
    root: Hash,
    /// Positions still to bring in line, with the hash the pivot's tree has there.
    waiting: VecDeque<(u16, Path, Hash)>,
    /// The positions asked for and not answered yet, with the hash the pivot's tree has at each:
    /// several at once, each of another peer, as positions out at once never overlap.
    asked: BTreeMap<(u16, Path), Hash>,
    /// Entries taken so far, for what the walk reports.
    taken: u64,
}

#[document(summary = "Why a part was not taken.", category = "Storage", see_also = "SnapshotWalk::take")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalkError {
    #[document(summary = "Nothing was asked at that position: an answer nobody waits for.")]
    Unasked,
    #[document(summary = "The part does not check against the pivot's root: the peer that sent it cannot be trusted with the rest.")]
    Invalid,
    #[document(summary = "The database failed.")]
    Db(DbError),
}

impl fmt::Display for WalkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalkError::Unasked => write!(f, "a part nobody asked for"),
            WalkError::Invalid => write!(f, "a part that does not check against the pivot's state root"),
            WalkError::Db(err) => write!(f, "{err}"),
        }
    }
}

impl From<DbError> for WalkError {
    fn from(err: DbError) -> WalkError {
        WalkError::Db(err)
    }
}

impl SnapshotWalk {
    #[document(parent = "SnapshotWalk", summary = "A walk to the state under this root, from its top.")]
    pub fn new(root: Hash) -> SnapshotWalk {
        SnapshotWalk {
            root,
            waiting: VecDeque::from([(0, [0; 32], root)]),
            asked: BTreeMap::new(),
            taken: 0,
        }
    }

    #[document(parent = "SnapshotWalk", summary = "The root the walk is bringing state in line with.")]
    pub fn root(&self) -> Hash {
        self.root
    }

    #[document(parent = "SnapshotWalk", summary = "How many entries it has taken from peers so far.")]
    pub fn taken(&self) -> u64 {
        self.taken
    }

    #[document(
        parent = "SnapshotWalk",
        summary = "The next position to ask a peer for, skipping every one the node's own tree holds the same already; None once nothing is left to ask for now, though answers may still be awaited. Asked again before an answer, it gives another position, for another peer: positions out at once never overlap.",
        params = { local = "The node's own state tree, as everything taken so far left it." },
    )]
    pub fn next(&mut self, local: &StateTree) -> Result<Option<(u16, Path)>, DbError> {
        while let Some((depth, prefix, expected)) = self.waiting.pop_front() {
            if local.subtree_hash(depth, &prefix)? != expected {
                self.asked.insert((depth, prefix), expected);

                return Ok(Some((depth, prefix)));
            }
        }

        Ok(None)
    }

    #[document(parent = "SnapshotWalk", summary = "How many positions are asked and not answered yet.")]
    pub fn out(&self) -> usize {
        self.asked.len()
    }

    #[document(
        parent = "SnapshotWalk",
        summary = "Takes a peer's answer for the position asked: checks it against the root, then either replaces what the node holds there with its entries, in layer, or waits on its two halves.",
        params = {
            layer = "Where to write: commit it, and follow the tree, before asking next.",
            hash = "The chain's hash.",
            depth = "The position it answers.",
            prefix = "Its path bits.",
            part = "The answer.",
        },
        returns = "Unasked for an answer to another position, Invalid for one that does not check: ask another peer.",
    )]
    pub fn take(&mut self, layer: &State, hash: HashType, depth: u16, prefix: Path, part: StatePart) -> Result<(), WalkError> {
        if !self.asked.contains_key(&(depth, prefix)) {
            return Err(WalkError::Unasked);
        }

        if !part.verify(&self.root, depth, &prefix) {
            return Err(WalkError::Invalid);
        }

        self.asked.remove(&(depth, prefix));

        match part.contents {
            PartContents::Branch { left, right } => {
                let zero = Hash::zero(hash);
                let right_prefix = with_bit(prefix, depth);

                for (half, expected) in [(prefix, left), (right_prefix, right)] {
                    match expected == zero {
                        // Nothing there in the pivot's tree: whatever the node holds there goes.
                        true => clear(layer, hash, depth + 1, &half)?,
                        false => self.waiting.push_back((depth + 1, half, expected)),
                    }
                }
            }
            PartContents::Entries(entries) => {
                let keep: BTreeSet<(Column, &[u8])> = entries.iter().map(|entry| (entry.column, entry.key.as_slice())).collect();

                for (column, key) in StateTree::new(layer, hash).entries_under(depth, &prefix)? {
                    if !keep.contains(&(column, key.as_slice())) {
                        layer.delete(column, key);
                    }
                }

                self.taken += entries.len() as u64;

                for entry in entries {
                    layer.put(entry.column, entry.key, entry.value);
                }
            }
        }

        Ok(())
    }

    #[document(
        parent = "SnapshotWalk",
        summary = "Gives back every position asked, to ask them again: the pivot moved.",
    )]
    pub fn retry(&mut self) {
        for ((depth, prefix), expected) in std::mem::take(&mut self.asked).into_iter().rev() {
            self.waiting.push_front((depth, prefix, expected));
        }
    }

    #[document(
        parent = "SnapshotWalk",
        summary = "Gives back one position asked, to ask it of another peer: the one asked failed, left, or sent what does not check.",
    )]
    pub fn give_back(&mut self, depth: u16, prefix: Path) {
        if let Some(expected) = self.asked.remove(&(depth, prefix)) {
            self.waiting.push_front((depth, prefix, expected));
        }
    }

    #[document(parent = "SnapshotWalk", summary = "Whether nothing is left to ask, nor awaited.")]
    pub fn done(&self) -> bool {
        self.asked.is_empty() && self.waiting.is_empty()
    }
}

/// Deletes every entry the node holds under a position.
fn clear(layer: &State, hash: HashType, depth: u16, prefix: &Path) -> Result<(), DbError> {
    for (column, key) in StateTree::new(layer, hash).entries_under(depth, prefix)? {
        layer.delete(column, key);
    }

    Ok(())
}

fn with_bit(mut prefix: Path, depth: u16) -> Path {
    let depth = usize::from(depth);
    prefix[depth / 8] |= 1 << (7 - depth % 8);

    prefix
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Database, MemoryDb, STATE_COLUMNS, StateEntry, stored_root};
    use std::sync::Arc;

    fn hash() -> HashType {
        HashType::default_chain()
    }

    /// A database holding entries, with its tree followed.
    fn holding(entries: &[(Column, Vec<u8>, Vec<u8>)]) -> Arc<MemoryDb> {
        let db = Arc::new(MemoryDb::default());
        let state = State::new(db.clone());

        for (column, key, value) in entries {
            state.put(*column, key.clone(), value.clone());
        }

        StateTree::new(&state, hash()).follow().unwrap();
        db.commit(state.take_changes()).unwrap();

        db
    }

    fn entries(range: std::ops::Range<u32>, value: u8) -> Vec<(Column, Vec<u8>, Vec<u8>)> {
        range
            .map(|n| (STATE_COLUMNS[n as usize % STATE_COLUMNS.len()], n.to_be_bytes().to_vec(), vec![value; 1 + n as usize % 5]))
            .collect()
    }

    /// Walks a node's database to a peer's state, a part at a time, the peer sending at most limit
    /// entries in each, and says how many parts it asked for.
    fn walk(node: &Arc<MemoryDb>, peer: &Arc<MemoryDb>, limit: usize) -> usize {
        let root = StateTree::new(&State::new(peer.clone()), hash()).root().unwrap();
        let mut walk = SnapshotWalk::new(root);
        let mut asked = 0;

        while let Some((depth, prefix)) = walk.next(&StateTree::new(&State::new(node.clone()), hash())).unwrap() {
            let part = StateTree::new(&State::new(peer.clone()), hash()).part(depth, &prefix, limit).unwrap().unwrap();
            let layer = State::new(node.clone());
            walk.take(&layer, hash(), depth, prefix, part).unwrap();
            StateTree::new(&layer, hash()).follow().unwrap();
            node.commit(layer.take_changes()).unwrap();
            asked += 1;
        }

        assert!(walk.done());
        assert_eq!(stored_root(&**node, hash()).unwrap(), root, "the node holds what the peer does");

        asked
    }

    #[test]
    fn a_walk_brings_a_node_to_a_peers_state() {
        let peer = holding(&entries(0..300, 1));
        let empty = Arc::new(MemoryDb::default());

        // From nothing, and from a state that overlaps, differs and holds what the peer does not.
        let from_nothing = walk(&empty, &peer, 16);
        let mut other = entries(100..400, 2);
        other.extend(entries(0..50, 1));
        let overlapping = holding(&other);
        walk(&overlapping, &peer, 16);

        assert!(from_nothing > 300 / 16, "{from_nothing} parts, at most 16 entries each");
        assert_eq!(walk(&holding(&entries(0..300, 1)), &peer, 16), 0, "nothing to ask once it holds the same");
        assert_eq!(walk(&Arc::new(MemoryDb::default()), &peer, 1_000), 1, "all in one part when it fits");
    }

    #[test]
    fn a_walk_that_starts_again_fetches_only_what_changed() {
        let mut state = entries(0..500, 1);
        let peer = holding(&state);
        let node = Arc::new(MemoryDb::default());
        walk(&node, &peer, 32);

        // The peer moves on: a few entries change, one goes, one comes.
        state[7].2 = vec![9];
        state[250].2 = vec![9, 9];
        state.remove(400);
        state.push((Column::Tokens, b"new".to_vec(), b"token".to_vec()));
        let moved = holding(&state);
        let again = walk(&node, &moved, 32);

        assert!(again < 40, "{again} parts for 4 changes");
    }

    #[test]
    fn parts_that_do_not_check_are_refused() {
        let peer = holding(&entries(0..100, 1));
        let root = StateTree::new(&State::new(peer.clone()), hash()).root().unwrap();
        let node = Arc::new(MemoryDb::default());
        let mut walk = SnapshotWalk::new(root);
        let (depth, prefix) = walk.next(&StateTree::new(&State::new(node.clone()), hash())).unwrap().unwrap();
        let mut part = StateTree::new(&State::new(peer.clone()), hash()).part(depth, &prefix, 1_000).unwrap().unwrap();
        let layer = State::new(node.clone());

        if let PartContents::Entries(entries) = &mut part.contents {
            entries.push(StateEntry {
                column: Column::Accounts,
                key: b"forged".to_vec(),
                value: b"rich".to_vec(),
            });
        }

        assert_eq!(walk.take(&layer, hash(), depth, prefix, part), Err(WalkError::Invalid));
        assert_eq!(walk.take(&layer, hash(), 3, [0; 32], StatePart {
            siblings: Vec::new(),
            contents: PartContents::Entries(Vec::new()),
        }), Err(WalkError::Unasked));
        assert!(layer.take_changes().is_empty(), "nothing refused is written");

        walk.retry();

        assert_eq!(walk.next(&StateTree::new(&State::new(node), hash())).unwrap(), Some((depth, prefix)), "asked again");
    }

    #[test]
    fn a_walk_keeps_several_positions_out_answered_in_any_order() {
        let peer = holding(&entries(0..500, 1));
        let root = StateTree::new(&State::new(peer.clone()), hash()).root().unwrap();
        let node = Arc::new(MemoryDb::default());
        let mut walk = SnapshotWalk::new(root);
        let (mut most, mut round) = (0, 0);

        loop {
            // Up to three out at once, as from three peers, distinct each.
            let mut out = Vec::new();

            while out.len() < 3
                && let Some(position) = walk.next(&StateTree::new(&State::new(node.clone()), hash())).unwrap()
            {
                assert!(!out.contains(&position));
                out.push(position);
            }

            if out.is_empty() {
                break;
            }

            most = most.max(walk.out());
            round += 1;

            // One peer fails every third round: its position goes back, and comes out again later.
            if round % 3 == 0 && out.len() > 1 {
                let (depth, prefix) = out.remove(0);
                walk.give_back(depth, prefix);
            }

            for (depth, prefix) in out.into_iter().rev() {
                let part = StateTree::new(&State::new(peer.clone()), hash()).part(depth, &prefix, 32).unwrap().unwrap();
                let layer = State::new(node.clone());
                walk.take(&layer, hash(), depth, prefix, part).unwrap();
                StateTree::new(&layer, hash()).follow().unwrap();
                node.commit(layer.take_changes()).unwrap();
            }
        }

        assert_eq!(most, 3);
        assert!(walk.done());
        assert_eq!(stored_root(&*node, hash()).unwrap(), root, "the node holds what the peer does");
    }
}
