//! A rotating authority: the signers of authority finality produce blocks in turn, one per slot.
//! The producer for a height is the signer at height mod n in the epoch's set, sorted by key; when
//! it misses its slot, each further slot lets the next in line step in. A block carries the offset
//! of its producer in the schedule as its difficulty, so an in-turn block weighs 2 and a stepped-in
//! one 1, and the chain prefers the schedule. The rules take the epoch's signers from whoever
//! checks the block: a node from its state, a light client from the voters it proved.

use {
    super::{Consensus, ConsensusError, MAX_DRIFT_MS, Producer, seal_bytes},
    crate::crypto::keypair::Keypair,
    crate::registry::validators::Member,
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::u256::u256,
    r#macro::document,
};

#[document(
    summary = "The rules of a chain whose authority finality signers produce its blocks in turn. Each epoch's signers, as finality fixed them, sorted by key, take the heights of that epoch round robin: the signer at height mod n is in turn at parent's time plus slot_ms, and each further slot lets the next in line produce too. A block's difficulty is its producer's offset from the one in turn, 0 for the one in turn, so it weighs 2 in turn and 1 otherwise. The seal is the producer's signature over the header's id, and the header's producer is the signer's account.",
    category = "Consensus",
    see_also = "Consensus, RotatingProducer, ConsensusConfig::Rotating, AuthorityConfig",
)]
pub struct Rotating {
    slot_ms: u64,
}

impl Rotating {
    #[document(parent = "Rotating", summary = "The rules for a slot length, from genesis.")]
    pub fn new(slot_ms: u64) -> Rotating {
        Rotating { slot_ms }
    }
}

/// The epoch's signers in schedule order: by key.
fn schedule(producers: &[Member]) -> Vec<Member> {
    let mut sorted = producers.to_vec();
    sorted.sort_by_key(|signer| signer.voter);

    sorted
}

/// Where a signer stands in the schedule for a height: 0 when in turn, else how many slots it
/// must wait. None for one not in the schedule.
fn offset(schedule: &[Member], height: u64, is: impl Fn(&Member) -> bool) -> Option<u64> {
    let n = schedule.len() as u64;
    let index = schedule.iter().position(is)? as u64;
    let in_turn = height % n;

    Some((index + n - in_turn) % n)
}

impl Consensus for Rotating {
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

        if producers.is_empty() {
            return Err(ConsensusError::NoProducers);
        }

        let schedule = schedule(producers);
        let Some(offset) = offset(&schedule, header.height, |signer| signer.account == header.producer) else {
            return Err(ConsensusError::WrongProducer);
        };

        if header.difficulty != u128::from(offset) {
            return Err(ConsensusError::WrongDifficulty {
                expected: u128::from(offset),
                got: header.difficulty,
            });
        }

        if header.timestamp < parent.timestamp.saturating_add(self.slot_ms.saturating_mul(offset + 1)) {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + MAX_DRIFT_MS {
            return Err(ConsensusError::FromTheFuture);
        }

        let Seal::Signature(signature) = seal else {
            return Err(ConsensusError::BadSeal);
        };
        let key = schedule.iter().find(|signer| signer.account == header.producer).expect("found above").voter;

        key.verify_signature(&seal_bytes(header), signature).map_err(|_| ConsensusError::BadSeal)
    }
}

#[document(
    summary = "Produces blocks as one of a rotating authority's signers: at its slot, in turn or stepping in, signs the header's id at once, as the signer's account.",
    category = "Consensus",
    see_also = "Rotating, Producer",
)]
pub struct RotatingProducer {
    keypair: Keypair,
    account: Address,
    slot_ms: u64,
}

impl RotatingProducer {
    #[document(
        parent = "RotatingProducer",
        summary = "Produces with a signer's voting key, for its account.",
        params = {
            keypair = "The signer's voting key, which signs its blocks as it signs its votes.",
            account = "The signer's account, which the blocks name as their producer and which their rewards go to.",
            slot_ms = "Milliseconds between blocks, from genesis.",
        },
    )]
    pub fn new(keypair: Keypair, account: Address, slot_ms: u64) -> RotatingProducer {
        RotatingProducer { keypair, account, slot_ms }
    }
}

impl Producer for RotatingProducer {
    fn address(&self, _producers: &[Member]) -> Address {
        self.account
    }

    /// Its slot: the parent's time plus one slot when in turn, and one more for each signer
    /// before it in line. Never, when it is not among the epoch's signers.
    fn start_at(&self, parent: &BlockHeader, producers: &[Member]) -> u64 {
        match self.difficulty(parent, producers) {
            Some(offset) => parent.timestamp.saturating_add(self.slot_ms.saturating_mul(offset as u64 + 1)),
            None => u64::MAX,
        }
    }

    /// Its offset in the schedule, which the block declares.
    fn difficulty(&self, parent: &BlockHeader, producers: &[Member]) -> Option<u128> {
        let key = self.keypair.public();

        offset(&schedule(producers), parent.height + 1, |signer| signer.voter == key).map(u128::from)
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        Some(Seal::Signature(self.keypair.sign(&seal_bytes(header))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::{Hash, HashType};

    const SLOT: u64 = 2_000;

    fn key(seed: u8) -> Keypair {
        Keypair::from_seed([seed; 32])
    }

    /// Three signers, the third with an account of its own, each weighing one as a signer set's
    /// validators do.
    fn signers() -> Vec<Member> {
        use crate::registry::signers::{Signer, SignerSet};

        SignerSet::new(vec![Signer::own(key(1).public()), Signer::own(key(2).public()), Signer { key: key(3).public(), account: key(9).public().address() }], 2, 2)
            .validators()
            .members
    }

    fn parent() -> BlockHeader {
        let chain = HashType::default_chain();

        BlockHeader {
            parent: Hash::zero(chain),
            height: 4,
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

    /// The block after the parent by a signer, at a time, declaring its offset, sealed.
    fn block(keypair: &Keypair, timestamp: u64, producers: &[Member]) -> (BlockHeader, Seal) {
        let account = producers.iter().find(|signer| signer.voter == keypair.public()).map_or(keypair.public().address(), |signer| signer.account);
        let mut producer = RotatingProducer::new(keypair.clone(), account, SLOT);
        let header = BlockHeader {
            parent: parent().id(),
            height: 5,
            timestamp,
            producer: account,
            difficulty: producer.difficulty(&parent(), producers).unwrap_or(0),
            ..parent()
        };
        let seal = producer.seal(&header).unwrap();

        (header, seal)
    }

    #[test]
    fn signers_take_turns_and_step_in_when_one_misses() {
        let rules = Rotating::new(SLOT);
        let signers = signers();
        let sorted = schedule(&signers);
        // Height 5 of 3 signers: the signer at index 2 of the sorted schedule is in turn.
        let in_turn = sorted[2];
        let next = sorted[0];
        let in_turn_key = [key(1), key(2), key(3)].into_iter().find(|keypair| keypair.public() == in_turn.voter).unwrap();
        let next_key = [key(1), key(2), key(3)].into_iter().find(|keypair| keypair.public() == next.voter).unwrap();
        let (header, seal) = block(&in_turn_key, 10_000 + SLOT, &signers);

        assert_eq!(rules.check(&header, &seal, &[parent()], &signers, 12_000), Ok(()));
        assert_eq!(header.difficulty, 0);
        assert_eq!(rules.work(&header), u256::from(2u64));
        assert_eq!(header.producer, in_turn.account);
        assert!(rules.needs_producers());

        // The next in line is early at one slot, and in time at two, weighing one.
        let (early, seal) = block(&next_key, 10_000 + SLOT, &signers);

        assert_eq!(early.difficulty, 1);
        assert_eq!(rules.check(&early, &seal, &[parent()], &signers, 12_000), Err(ConsensusError::Early));

        let (stepped, seal) = block(&next_key, 10_000 + 2 * SLOT, &signers);

        assert_eq!(rules.check(&stepped, &seal, &[parent()], &signers, 14_000), Ok(()));
        assert_eq!(rules.work(&stepped), u256::ONE);
        assert_eq!(rules.difficulty(&[parent()], 10_000 + 2 * SLOT), 1, "two slots passed");

        // A producer's start is its slot, and nobody's when it is not a signer.
        let mut producer = RotatingProducer::new(next_key.clone(), next.account, SLOT);

        assert_eq!(producer.start_at(&parent(), &signers), 10_000 + 2 * SLOT);
        assert_eq!(RotatingProducer::new(key(7), key(7).public().address(), SLOT).start_at(&parent(), &signers), u64::MAX);
        assert_eq!(producer.address(&signers), next.account);
        assert!(producer.seal(&stepped).is_some());
    }

    #[test]
    fn bad_blocks_are_refused() {
        let rules = Rotating::new(SLOT);
        let signers = signers();
        let sorted = schedule(&signers);
        let in_turn = [key(1), key(2), key(3)].into_iter().find(|keypair| keypair.public() == sorted[2].voter).unwrap();
        let (good, seal) = block(&in_turn, 10_000 + SLOT, &signers);

        // A stranger, a wrong offset, the wrong key's signature, no seal, no signers, no parent,
        // and the future.
        let (stranger, stranger_seal) = block(&key(7), 10_000 + SLOT, &signers);
        let wrong_offset = BlockHeader { difficulty: 1, ..good.clone() };
        let other = [key(1), key(2), key(3)].into_iter().find(|keypair| keypair.public() == sorted[0].voter).unwrap();
        let forged = Seal::Signature(other.sign(&seal_bytes(&good)));
        let future = BlockHeader { timestamp: 10_000 + SLOT + MAX_DRIFT_MS + 1, ..good.clone() };

        assert_eq!(rules.check(&stranger, &stranger_seal, &[parent()], &signers, 12_000), Err(ConsensusError::WrongProducer));
        assert!(matches!(rules.check(&wrong_offset, &seal, &[parent()], &signers, 12_000), Err(ConsensusError::WrongDifficulty { .. })));
        assert_eq!(rules.check(&good, &forged, &[parent()], &signers, 12_000), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&good, &Seal::None, &[parent()], &signers, 12_000), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&good, &seal, &[parent()], &[], 12_000), Err(ConsensusError::NoProducers));
        assert_eq!(rules.check(&good, &seal, &[], &signers, 12_000), Err(ConsensusError::NoAncestors));
        assert_eq!(rules.check(&future, &seal, &[parent()], &signers, 12_000), Err(ConsensusError::FromTheFuture));
    }
}
