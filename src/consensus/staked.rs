//! Production by stake: the bonded validators of FFG finality produce the blocks, in slots, each
//! with a share in proportion to its bond. The producers of a height are its epoch's validator
//! set, as finality fixed it `activation_delay` epochs ahead, so a validator produces from the
//! epoch its bond first counts in and stops with the epoch it leaves. The schedule picks among
//! them by weight, one of two ways genesis names: a draw by the chain's hash, or Tendermint's
//! priority rotation. As under a rotating authority, a block declares as its difficulty how many
//! slots passed before its producer's turn, an in-turn block weighs 2 and a later one 1, and the
//! rules take the epoch's set from whoever checks the block: a node from its state, a light
//! client from the voters it proved.

use {
    super::{Consensus, ConsensusError, Producer, seal_bytes},
    crate::crypto::hashable::Hashable,
    crate::crypto::hasher::Hasher,
    crate::crypto::keypair::{Keypair, PublicKey},
    crate::registry::validators::{Member, ValidatorSet},
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::hash::HashType,
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

/// Separates the schedule's draws from anything else the chain hashes.
const DRAW_DOMAIN: &[u8] = b"chainv4 draw";

#[document(
    summary = "How far ahead, in slots, a validator looks for its turn under the draw: a validator not drawn within this many slots of a height does not produce at it. A validator with a thousandth of the stake is drawn within it in all but a third of heights, and the chain takes the next height's draw long before.",
    category = "Consensus",
    see_also = "Schedule::Draw",
)]
pub const MAX_DRAW_WAIT: u64 = 1_024;

#[document(
    summary = "How production by stake picks a slot's producer among the epoch's validators. Part of genesis, as ConsensusConfig::Staked.",
    category = "Consensus",
    see_also = "ConsensusConfig::Staked, Staked",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Schedule {
    #[document(
        summary = "A weighted draw: the chain's hash over the set, the height and the slot picks a validator with a chance in proportion to its bond, and every slot that passes draws again. Stateless, so checking a block takes the set alone; a validator may be drawn for several slots of a height in a row, and one that misses its slot costs the chain a slot's wait."
    )]
    #[wincode(tag = 0)]
    Draw,
    #[document(
        summary = "Tendermint's proposer rotation: from the epoch's first height on, each validator's priority grows by its bond every height, the highest produces, and its priority falls by the set's whole weight. Exact shares over an epoch and an order known in advance, the next in line stepping in for a missed slot; checking a block replays the epoch's heights before it."
    )]
    #[wincode(tag = 1)]
    Priority,
}

impl Schedule {
    #[document(parent = "Schedule", summary = "The schedule's name, as genesis files spell it: draw or priority.")]
    pub fn name(&self) -> &'static str {
        match self {
            Schedule::Draw => "draw",
            Schedule::Priority => "priority",
        }
    }
}

#[document(
    summary = "The rules of a chain whose FFG validators produce its blocks by stake. Each epoch's validator set, as finality fixed it, is the epoch's producers; the schedule names, for each height, the validator in turn and the one each slot that passes hands the height to. A block's difficulty is how many slots its producer waited, 0 in turn, so it weighs 2 in turn and 1 otherwise. The seal is the producer's signature over the header's id, by its voting key, and the header's producer is the account its bond names.",
    category = "Consensus",
    see_also = "Consensus, StakedProducer, ConsensusConfig::Staked, Schedule, FfgConfig",
)]
#[derive(Clone, Copy)]
pub struct Staked {
    slot_ms: u64,
    schedule: Schedule,
    hash: HashType,
    interval: u64,
}

impl Staked {
    #[document(
        parent = "Staked",
        summary = "The rules for a slot length and a schedule, from genesis.",
        params = {
            slot_ms = "Milliseconds between blocks, and from a missed slot to the next in line's.",
            schedule = "How a slot's producer is picked.",
            hash = "The chain's hash, which the draw uses.",
            checkpoint_interval = "Blocks per finality epoch, where the priority rotation starts over.",
        },
    )]
    pub fn new(slot_ms: u64, schedule: Schedule, hash: HashType, checkpoint_interval: u64) -> Staked {
        Staked {
            slot_ms,
            schedule,
            hash,
            interval: checkpoint_interval.max(1),
        }
    }

    #[document(parent = "Staked", summary = "How a slot's producer is picked.")]
    pub fn schedule(&self) -> Schedule {
        self.schedule
    }

    /// The set in schedule order: by voting key, as finality fixes it, whatever order the caller
    /// held it in, and without members of no weight, which no schedule could pick.
    fn ordered(producers: &[Member]) -> Vec<Member> {
        let mut set: Vec<Member> = producers.iter().filter(|member| member.weight > u256::ZERO).copied().collect();
        set.sort_by_key(|member| member.voter);

        set
    }

    #[document(
        parent = "Staked",
        summary = "The validator the schedule names for a height after this many slots passed: the one in turn at 0, and the one each further slot hands the height to. None for an empty set, or one of no weight.",
    )]
    pub fn producer_at(&self, producers: &[Member], height: u64, offset: u64) -> Option<Member> {
        let set = Staked::ordered(producers);

        if set.is_empty() {
            return None;
        }

        Some(match self.schedule {
            Schedule::Draw => Drawer::new(self.hash, &set).pick(height, offset),
            Schedule::Priority => {
                let order = self.priority_order(&set, height);

                order[(offset % order.len() as u64) as usize]
            }
        })
    }

    #[document(
        parent = "Staked",
        summary = "How many slots a validator waits at a height before it may produce: the first offset the schedule names it at, 0 when in turn. None for one not in the set, or, under the draw, not drawn within MAX_DRAW_WAIT slots.",
    )]
    pub fn offset_of(&self, producers: &[Member], height: u64, voter: &PublicKey) -> Option<u64> {
        self.offset_in(&Staked::ordered(producers), height, voter)
    }

    /// offset_of for a set already in schedule order.
    fn offset_in(&self, set: &[Member], height: u64, voter: &PublicKey) -> Option<u64> {
        if !set.iter().any(|member| member.voter == *voter) {
            return None;
        }

        match self.schedule {
            Schedule::Draw => {
                let drawer = Drawer::new(self.hash, set);

                (0..MAX_DRAW_WAIT).find(|offset| drawer.pick(height, *offset).voter == *voter)
            }
            Schedule::Priority => self.priority_order(set, height).iter().position(|member| member.voter == *voter).map(|index| index as u64),
        }
    }

    /// The set in priority order at a height: Tendermint's rotation replayed from the epoch's
    /// first height. Every height, each priority grows by its member's weight; the order is by
    /// priority, the highest first, ties by key; and the one in turn then falls by the set's whole
    /// weight. Every other one rising by it instead keeps the same order without a negative number.
    fn priority_order(&self, set: &[Member], height: u64) -> Vec<Member> {
        let start = height - height % self.interval;
        let total = set.iter().fold(u256::ZERO, |total, member| total.saturating_add(member.weight));
        let mut priority = vec![u256::ZERO; set.len()];
        let mut order: Vec<usize> = (0..set.len()).collect();

        for at in start..=height {
            for (priority, member) in priority.iter_mut().zip(set) {
                *priority = priority.saturating_add(member.weight);
            }

            order.sort_by(|a, b| priority[*b].cmp(&priority[*a]).then_with(|| set[*a].voter.cmp(&set[*b].voter)));

            if at == height {
                break;
            }

            let in_turn = order[0];

            for (index, priority) in priority.iter_mut().enumerate() {
                if index != in_turn {
                    *priority = priority.saturating_add(total);
                }
            }
        }

        order.into_iter().map(|index| set[index]).collect()
    }
}

/// Draws from one set: the set's encoding hashed once, then a point per height and slot.
struct Drawer<'a> {
    hash: HashType,
    set: &'a [Member],
    prefix: Vec<u8>,
    total: u256,
}

impl<'a> Drawer<'a> {
    fn new(hash: HashType, set: &'a [Member]) -> Drawer<'a> {
        let mut prefix = DRAW_DOMAIN.to_vec();
        prefix.extend_from_slice(&ValidatorSet { members: set.to_vec() }.hashable_bytes());

        Drawer {
            hash,
            set,
            prefix,
            total: set.iter().fold(u256::ZERO, |total, member| total.saturating_add(member.weight)),
        }
    }

    /// The member whose share of the weight the hash of the height and slot lands in.
    fn pick(&self, height: u64, offset: u64) -> Member {
        let mut hasher = Hasher::new(self.hash);
        hasher.update(&self.prefix);
        hasher.update(&height.to_le_bytes());
        hasher.update(&offset.to_le_bytes());
        let digest = hasher.finalize().to_bytes();
        // Right-aligned, so a short digest still spreads over the weight.
        let mut bytes = [0; 32];
        let taken = digest.len().min(32);
        bytes[32 - taken..].copy_from_slice(&digest[..taken]);
        let mut point = u256::from_be_bytes(bytes) % self.total;

        for member in self.set {
            if point < member.weight {
                return *member;
            }

            point -= member.weight;
        }

        unreachable!("the point is below the total, which the weights add up to")
    }
}

impl Consensus for Staked {
    fn needs_producers(&self) -> bool {
        true
    }

    /// In turn weighs 2, stepping in 1: the schedule wins.
    fn work(&self, header: &BlockHeader) -> u256 {
        match header.difficulty {
            0 => u256::from(2u64),
            _ => u256::ONE,
        }
    }

    /// The slots that passed before this block, less the one in turn: what a producer that waited
    /// that long declares. The producer itself declares its own offset; this is the most it can be.
    fn difficulty(&self, ancestors: &[BlockHeader], timestamp: u64) -> u128 {
        let Some(parent) = ancestors.first() else {
            return 0;
        };

        u128::from((timestamp.saturating_sub(parent.timestamp) / self.slot_ms.max(1)).saturating_sub(1))
    }

    fn check(&self, header: &BlockHeader, seal: &Seal, ancestors: &[BlockHeader], producers: &[Member], now: u64) -> Result<(), ConsensusError> {
        let Some(parent) = ancestors.first() else {
            return Err(ConsensusError::NoAncestors);
        };
        let set = Staked::ordered(producers);

        if set.is_empty() {
            return Err(ConsensusError::NoProducers);
        }

        // The producer is the account's member whose turn the block declares: an account may bond
        // more than one voting key.
        let candidates: Vec<&Member> = set.iter().filter(|member| member.account == header.producer).collect();

        if candidates.is_empty() {
            return Err(ConsensusError::WrongProducer);
        }

        let offsets: Vec<Option<u64>> = candidates.iter().map(|member| self.offset_in(&set, header.height, &member.voter)).collect();
        let Some(member) = candidates.iter().zip(&offsets).find(|(_, offset)| offset.map(u128::from) == Some(header.difficulty)).map(|(member, _)| **member) else {
            return Err(ConsensusError::WrongDifficulty {
                expected: offsets.iter().flatten().min().map_or(u128::MAX, |offset| u128::from(*offset)),
                got: header.difficulty,
            });
        };
        let offset = u64::try_from(header.difficulty).expect("it matched one");

        if header.timestamp < parent.timestamp.saturating_add(self.slot_ms.saturating_mul(offset.saturating_add(1))) {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + super::drift_ms(self.slot_ms) {
            return Err(ConsensusError::FromTheFuture);
        }

        let Seal::Signature(signature) = seal else {
            return Err(ConsensusError::BadSeal);
        };

        member.voter.verify_signature(&seal_bytes(header), signature).map_err(|_| ConsensusError::BadSeal)
    }
}

#[document(
    summary = "Produces blocks as one of a staked chain's validators: at its slot, in turn or stepping in, signs the header's id at once with its voting key, for the account its bond names, which it learns from the epoch's producers.",
    category = "Consensus",
    see_also = "Staked, Producer",
)]
pub struct StakedProducer {
    keypair: Keypair,
    rules: Staked,
}

impl StakedProducer {
    #[document(
        parent = "StakedProducer",
        summary = "Produces with a validator's voting key, under the chain's rules.",
        params = {
            keypair = "The validator's voting key, which signs its blocks as it signs its votes.",
            rules = "The chain's rules, which say when its turn comes.",
        },
    )]
    pub fn new(keypair: Keypair, rules: Staked) -> StakedProducer {
        StakedProducer { keypair, rules }
    }

    /// Its member in the epoch's set, if it is one.
    fn member(&self, producers: &[Member]) -> Option<Member> {
        let key = self.keypair.public();

        producers.iter().find(|member| member.voter == key).copied()
    }
}

impl Producer for StakedProducer {
    /// The account its bond names; its key's own address while it is not among the producers,
    /// when it produces nothing anyway.
    fn address(&self, producers: &[Member]) -> Address {
        self.member(producers).map_or_else(|| self.keypair.public().address(), |member| member.account)
    }

    /// Its slot: the parent's time plus one slot when in turn, and one more for each slot the
    /// schedule has it wait. Never, when it is not among the epoch's validators.
    fn start_at(&self, parent: &BlockHeader, producers: &[Member]) -> u64 {
        match self.difficulty(parent, producers) {
            Some(offset) => parent.timestamp.saturating_add(self.rules.slot_ms.saturating_mul(offset as u64 + 1)),
            None => u64::MAX,
        }
    }

    /// Its offset in the schedule, which the block declares.
    fn difficulty(&self, parent: &BlockHeader, producers: &[Member]) -> Option<u128> {
        self.rules.offset_of(producers, parent.height + 1, &self.keypair.public()).map(u128::from)
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        Some(Seal::Signature(self.keypair.sign(&seal_bytes(header))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::Hash;

    const SLOT: u64 = 2_000;
    const INTERVAL: u64 = 6;

    fn key(seed: u8) -> Keypair {
        Keypair::from_seed([seed; 32])
    }

    fn rules(schedule: Schedule) -> Staked {
        Staked::new(SLOT, schedule, HashType::default_chain(), INTERVAL)
    }

    /// Three validators weighing 1, 2 and 3, the third bonded by another account.
    fn validators() -> Vec<Member> {
        vec![
            Member { voter: key(1).public(), account: key(1).public().address(), weight: u256::ONE, attester: None },
            Member { voter: key(2).public(), account: key(2).public().address(), weight: u256::from(2u64), attester: None },
            Member { voter: key(3).public(), account: key(9).public().address(), weight: u256::from(3u64), attester: None },
        ]
    }

    fn keypair_of(member: &Member) -> Keypair {
        [key(1), key(2), key(3)].into_iter().find(|keypair| keypair.public() == member.voter).unwrap()
    }

    fn parent(height: u64) -> BlockHeader {
        let chain = HashType::default_chain();

        BlockHeader {
            parent: Hash::zero(chain),
            height,
            timestamp: 10_000,
            producer: key(1).public().address(),
            difficulty: 0,
            tx_root: Hash::zero(chain),
            proof_root: Hash::zero(chain),
            pool_root: Hash::zero(chain),
            receipts_root: Hash::zero(chain),
            state_root: Hash::zero(chain),
            gas_used: 0,
            base_price: u256::ZERO,
        }
    }

    /// The block after a parent by a validator, at a time, declaring its offset, sealed.
    fn block(rules: Staked, keypair: &Keypair, parent: &BlockHeader, timestamp: u64, producers: &[Member]) -> (BlockHeader, Seal) {
        let mut producer = StakedProducer::new(keypair.clone(), rules);
        let header = BlockHeader {
            parent: parent.id(),
            height: parent.height + 1,
            timestamp,
            producer: producer.address(producers),
            difficulty: producer.difficulty(parent, producers).unwrap_or(0),
            ..parent.clone()
        };
        let seal = producer.seal(&header).unwrap();

        (header, seal)
    }

    /// How often each validator is in turn over the heights, by weight.
    fn shares(rules: Staked, heights: std::ops::Range<u64>) -> [u64; 3] {
        let set = validators();
        let mut counts = [0; 3];

        for height in heights {
            let in_turn = rules.producer_at(&set, height, 0).unwrap();
            counts[set.iter().position(|member| member.voter == in_turn.voter).unwrap()] += 1;
        }

        counts
    }

    #[test]
    fn the_draw_gives_each_validator_its_share_of_turns() {
        let [light, middle, heavy] = shares(rules(Schedule::Draw), 0..6_000);

        // A sixth, a third and a half, give or take.
        assert!((800..1_200).contains(&light), "{light}");
        assert!((1_700..2_300).contains(&middle), "{middle}");
        assert!((2_700..3_300).contains(&heavy), "{heavy}");

        // The same set, height and slot always draw the same validator, whatever order the set
        // came in, and a member of no weight is never drawn.
        let rules = rules(Schedule::Draw);
        let mut shuffled = validators();
        shuffled.reverse();
        shuffled.push(Member { voter: key(4).public(), account: key(4).public().address(), weight: u256::ZERO, attester: None });

        for height in 0..50 {
            assert_eq!(rules.producer_at(&validators(), height, 0), rules.producer_at(&shuffled, height, 0));
            assert_ne!(rules.producer_at(&shuffled, height, 3).unwrap().voter, key(4).public());
        }

        assert_eq!(rules.offset_of(&shuffled, 7, &key(4).public()), None, "no weight, no turn");
        assert_eq!(rules.producer_at(&[], 7, 0), None);
    }

    #[test]
    fn the_priority_rotation_gives_each_validator_exactly_its_share_each_epoch() {
        let rules = rules(Schedule::Priority);

        // The weights add up to the epoch's length, so each validator is in turn its weight's
        // worth of heights in every epoch.
        for epoch in 0..5 {
            assert_eq!(shares(rules, epoch * INTERVAL..(epoch + 1) * INTERVAL), [1, 2, 3]);
        }

        // An order per height, every validator in it once, the heaviest first at the epoch's
        // start and the lightest last: the offsets are its positions.
        let set = validators();
        let order: Vec<Member> = (0..3).map(|offset| rules.producer_at(&set, INTERVAL, offset).unwrap()).collect();

        assert_eq!(order[0].voter, key(3).public());
        assert_eq!(order[2].voter, key(1).public());
        assert_eq!(rules.producer_at(&set, INTERVAL, 3), Some(order[0]), "and round again");

        for (offset, member) in order.iter().enumerate() {
            assert_eq!(rules.offset_of(&set, INTERVAL, &member.voter), Some(offset as u64));
        }

        assert_eq!(rules.offset_of(&set, INTERVAL, &key(7).public()), None);
    }

    #[test]
    fn validators_produce_in_turn_and_step_in_when_one_misses() {
        for schedule in [Schedule::Draw, Schedule::Priority] {
            let rules = rules(schedule);
            let set = validators();
            let parent = parent(4);
            let in_turn = rules.producer_at(&set, 5, 0).unwrap();
            // The first slot that hands the height to someone else.
            let (wait, next) = (1..).map(|offset| (offset, rules.producer_at(&set, 5, offset).unwrap())).find(|(_, member)| member.voter != in_turn.voter).unwrap();
            let (header, seal) = block(rules, &keypair_of(&in_turn), &parent, 10_000 + SLOT, &set);

            assert_eq!(rules.check(&header, &seal, std::slice::from_ref(&parent), &set, 12_000), Ok(()));
            assert_eq!((header.difficulty, header.producer), (0, in_turn.account));
            assert_eq!(rules.work(&header), u256::from(2u64));
            assert!(rules.needs_producers());

            // The next in line is early at its slot less one, in time at its slot, and weighs one.
            let (early, seal) = block(rules, &keypair_of(&next), &parent, 10_000 + wait * SLOT, &set);

            assert_eq!(early.difficulty, u128::from(wait));
            assert_eq!(rules.check(&early, &seal, std::slice::from_ref(&parent), &set, 12_000 + wait * SLOT), Err(ConsensusError::Early));

            let (stepped, seal) = block(rules, &keypair_of(&next), &parent, 10_000 + (wait + 1) * SLOT, &set);

            assert_eq!(rules.check(&stepped, &seal, std::slice::from_ref(&parent), &set, 12_000 + wait * SLOT), Ok(()));
            assert_eq!(rules.work(&stepped), u256::ONE);
            assert_eq!(rules.difficulty(std::slice::from_ref(&parent), 10_000 + (wait + 1) * SLOT), u128::from(wait), "that many slots passed");

            // A producer's start is its slot, and nobody's when it is not a validator.
            let mut producer = StakedProducer::new(keypair_of(&next), rules);

            assert_eq!(producer.start_at(&parent, &set), 10_000 + (wait + 1) * SLOT);
            assert_eq!(producer.address(&set), next.account);
            assert!(producer.seal(&stepped).is_some());
            assert_eq!(StakedProducer::new(key(7), rules).start_at(&parent, &set), u64::MAX);
            assert_eq!(StakedProducer::new(key(7), rules).address(&set), key(7).public().address());
        }
    }

    #[test]
    fn bad_blocks_are_refused() {
        for schedule in [Schedule::Draw, Schedule::Priority] {
            let rules = rules(schedule);
            let set = validators();
            let parent = parent(4);
            let in_turn = rules.producer_at(&set, 5, 0).unwrap();
            let other = set.iter().find(|member| member.voter != in_turn.voter).unwrap();
            let (good, seal) = block(rules, &keypair_of(&in_turn), &parent, 10_000 + SLOT, &set);

            // A stranger, a wrong offset, the wrong key's signature, no seal, no validators, no
            // parent, and the future.
            let (stranger, stranger_seal) = block(rules, &key(7), &parent, 10_000 + SLOT, &set);
            let wrong_offset = BlockHeader { difficulty: 1, ..good.clone() };
            let forged = Seal::Signature(keypair_of(other).sign(&seal_bytes(&good)));
            let future = BlockHeader { timestamp: 10_000 + SLOT + crate::consensus::MAX_DRIFT_MS + 1, ..good.clone() };

            assert_eq!(rules.check(&stranger, &stranger_seal, std::slice::from_ref(&parent), &set, 12_000), Err(ConsensusError::WrongProducer));
            assert_eq!(
                rules.check(&wrong_offset, &seal, std::slice::from_ref(&parent), &set, 12_000),
                Err(ConsensusError::WrongDifficulty { expected: 0, got: 1 })
            );
            assert_eq!(rules.check(&good, &forged, std::slice::from_ref(&parent), &set, 12_000), Err(ConsensusError::BadSeal));
            assert_eq!(rules.check(&good, &Seal::None, std::slice::from_ref(&parent), &set, 12_000), Err(ConsensusError::BadSeal));
            assert_eq!(rules.check(&good, &seal, std::slice::from_ref(&parent), &[], 12_000), Err(ConsensusError::NoProducers));
            assert_eq!(rules.check(&good, &seal, &[], &set, 12_000), Err(ConsensusError::NoAncestors));
            assert_eq!(rules.check(&future, &seal, std::slice::from_ref(&parent), &set, 12_000), Err(ConsensusError::FromTheFuture));
        }
    }

    #[test]
    fn the_configuration_keeps_its_tags() {
        use crate::consensus::ConsensusConfig;

        let config = ConsensusConfig::Staked { slot_ms: SLOT, schedule: Schedule::Priority };
        let bytes = config.hashable_bytes_with_tag();

        assert_eq!(bytes[0], 3, "the staked variant");
        assert_eq!(bytes[9], 1, "the priority schedule, after the slot");
        assert_eq!(wincode::deserialize::<ConsensusConfig>(&bytes).unwrap(), config);
        assert_eq!((config.name(), Schedule::Draw.name(), Schedule::Priority.name()), ("staked", "draw", "priority"));
    }
}
