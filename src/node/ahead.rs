//! Blocks gossiped ahead of the tip: a peer's block whose parent this node lacks yet. Rather than
//! drop it and fetch it again once sync gets there, the node holds a few, each checked as far as it
//! can be without its parent: its roots, that it is no block this node has or knows to be bad, and
//! that it sits above finality and not far past the tip. When the parent lands, by gossip or sync,
//! the blocks held for it are imported at once, and theirs after them.
//!
//! What is held stays bounded: so many in all and from any one peer, so many bytes, for so long,
//! and each one counts in its peer's share of the backlog until it goes, so a peer gossiping blocks
//! that never connect fills its own share and no more.

use {
    crate::structs::block::Block,
    crate::structs::hash::Hash,
    libp2p::PeerId,
    r#macro::document,
    std::time::{Duration, Instant},
};

#[document(
    summary = "How many blocks gossiped ahead of the tip a node holds for their parent: in all, from any one peer, for how long each, and how far past its tip. Set with --tune's ahead. keys.",
    category = "Node",
    see_also = "Tuning::ahead",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AheadLimits {
    #[document(summary = "Blocks held in all: 0 holds none, as before.")]
    pub blocks: usize,
    #[document(summary = "Blocks held from any one peer.")]
    pub per_peer: usize,
    #[document(summary = "How long a block is held before it is let go, its parent never having come.")]
    pub keep: Duration,
    #[document(summary = "How many heights past the tip a block held may be.")]
    pub depth: u64,
}

impl Default for AheadLimits {
    fn default() -> AheadLimits {
        AheadLimits { blocks: 32, per_peer: 4, keep: Duration::from_secs(120), depth: 64 }
    }
}

/// The most bytes of blocks held in all, whatever AheadLimits::blocks says.
const MOST_BYTES: usize = 16 << 20;

/// One block held, who sent it, its size and when it came.
struct Held {
    block: Block,
    from: PeerId,
    bytes: usize,
    at: Instant,
}

/// The blocks held, oldest first.
pub(super) struct Ahead {
    limits: AheadLimits,
    held: Vec<Held>,
}

/// What hold made of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Holding {
    /// Held, until its parent lands or it is let go.
    Held,
    /// Held already, as another peer passed it on too.
    Already,
    /// Not held: no room in all, for its peer, or for its size.
    Full,
}

impl Ahead {
    pub(super) fn new(limits: AheadLimits) -> Ahead {
        Ahead { limits, held: Vec::new() }
    }

    pub(super) fn limits(&self) -> AheadLimits {
        self.limits
    }

    /// Holds a block from a peer, if there is room for it.
    pub(super) fn hold(&mut self, block: Block, from: PeerId, now: Instant) -> Holding {
        let id = block.id();

        if self.held.iter().any(|held| held.block.id() == id) {
            return Holding::Already;
        }

        let bytes = wincode::serialized_size(&block).map_or(usize::MAX, |size| size as usize);
        let total: usize = self.held.iter().map(|held| held.bytes).sum();
        let theirs = self.held.iter().filter(|held| held.from == from).count();

        if self.held.len() >= self.limits.blocks || theirs >= self.limits.per_peer || total.saturating_add(bytes) > MOST_BYTES {
            return Holding::Full;
        }

        self.held.push(Held { block, from, bytes, at: now });

        Holding::Held
    }

    /// Takes the blocks held whose parent this is, oldest first, with who sent each.
    pub(super) fn children(&mut self, parent: &Hash) -> Vec<(Block, PeerId)> {
        let (children, rest): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held).into_iter().partition(|held| held.block.header.parent == *parent);
        self.held = rest;

        children.into_iter().map(|held| (held.block, held.from)).collect()
    }

    /// Lets go of the blocks held longer than AheadLimits::keep, and those at or below height,
    /// which the chain now has or never will: who sent each, for the backlog to count it out.
    pub(super) fn let_go(&mut self, now: Instant, height: u64) -> Vec<PeerId> {
        let keep = self.limits.keep;
        let (gone, rest): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.held).into_iter().partition(|held| now.saturating_duration_since(held.at) > keep || held.block.header.height <= height);
        self.held = rest;

        gone.into_iter().map(|held| held.from).collect()
    }

    pub(super) fn len(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::structs::address::Address,
        crate::structs::block::{BlockHeader, Seal},
        crate::structs::hash::HashType,
        crate::structs::u256::u256,
    };

    /// A block at a height on a parent, told apart from its siblings by its time.
    fn block(height: u64, parent: u8, time: u8) -> Block {
        let hash = HashType::default_chain();
        let zero = Hash::zero(hash);

        Block {
            header: BlockHeader {
                parent: Hash::from_bytes(hash, &vec![parent; hash.digest_len()]).unwrap(),
                height,
                timestamp: u64::from(time),
                producer: Address::Ed25519([1; 32]),
                difficulty: 0,
                tx_root: zero,
                proof_root: zero,
                pool_root: zero,
                receipts_root: zero,
                state_root: zero,
                gas_used: 0,
                base_price: u256::ZERO,
            },
            txs: Vec::new(),
            proofs: Vec::new(),
            pool: None,
            seal: Seal::None,
        }
    }

    #[test]
    fn a_few_are_held_from_each_peer_and_in_all() {
        let mut ahead = Ahead::new(AheadLimits { blocks: 5, per_peer: 2, ..AheadLimits::default() });
        let (one, two, three) = (PeerId::random(), PeerId::random(), PeerId::random());
        let now = Instant::now();

        assert_eq!(ahead.hold(block(10, 1, 0), one, now), Holding::Held);
        assert_eq!(ahead.hold(block(10, 1, 0), two, now), Holding::Already, "the same block from another peer");
        assert_eq!(ahead.hold(block(11, 2, 0), one, now), Holding::Held);
        assert_eq!(ahead.hold(block(12, 3, 0), one, now), Holding::Full, "its peer holds its two");
        assert_eq!(ahead.hold(block(12, 3, 0), two, now), Holding::Held);
        assert_eq!(ahead.hold(block(13, 4, 0), two, now), Holding::Held);
        assert_eq!(ahead.hold(block(14, 5, 0), three, now), Holding::Held);
        assert_eq!(ahead.hold(block(15, 6, 0), three, now), Holding::Full, "five in all");
        assert_eq!(ahead.len(), 5);
    }

    #[test]
    fn the_children_of_a_parent_that_landed_are_taken_and_the_old_let_go() {
        let mut ahead = Ahead::new(AheadLimits::default());
        let (one, two) = (PeerId::random(), PeerId::random());
        let now = Instant::now();
        let parent = Hash::from_bytes(HashType::default_chain(), &vec![7; HashType::default_chain().digest_len()]).unwrap();

        ahead.hold(block(20, 7, 1), one, now);
        ahead.hold(block(20, 7, 2), two, now);
        ahead.hold(block(30, 8, 0), two, now);

        let children = ahead.children(&parent);
        assert_eq!(children.iter().map(|(_, from)| *from).collect::<Vec<_>>(), [one, two], "both of its children, oldest first");
        assert_eq!(ahead.len(), 1);

        // Its parent never came: let go once kept for long enough, or once the tip passed it.
        assert!(ahead.let_go(now + Duration::from_secs(60), 0).is_empty());
        assert_eq!(ahead.let_go(now + Duration::from_secs(60), 30), [two], "the chain is past it");

        ahead.hold(block(40, 9, 0), one, now);
        assert_eq!(ahead.let_go(now + Duration::from_secs(121), 0), [one]);
        assert_eq!(ahead.len(), 0);
    }
}
