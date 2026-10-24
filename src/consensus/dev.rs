use {
    super::{Consensus, ConsensusError, MAX_DRIFT_MS, Producer},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey},
    crate::structs::address::Address,
    crate::structs::block::{BlockHeader, Seal},
    crate::structs::u256::u256,
    r#macro::document,
};

/// Separates block seals from anything else a key might sign.
const SEAL_DOMAIN: &[u8] = b"chainv4 block";

#[document(
    summary = "The rules of a chain where one known key produces every block, one per slot. For development and tests: blocks come at a steady pace, and forks never happen.",
    category = "Consensus",
    see_also = "Consensus, AuthorityProducer, ConsensusConfig::Authority",
)]
pub struct DevAuthority {
    authority: PublicKey,
    slot_ms: u64,
}

impl DevAuthority {
    #[document(
        parent = "DevAuthority",
        summary = "The rules for an authority, from genesis.",
        params = {
            authority = "The only key that may produce.",
            slot_ms = "Milliseconds between blocks.",
        },
    )]
    pub fn new(authority: PublicKey, slot_ms: u64) -> DevAuthority {
        DevAuthority { authority, slot_ms }
    }

    #[document(
        parent = "DevAuthority",
        summary = "The well-known development key. Never use it for anything of value: its seed is in the source.",
    )]
    pub fn dev_keypair() -> Keypair {
        Keypair::development()
    }
}

fn seal_bytes(header: &BlockHeader) -> Vec<u8> {
    let mut bytes = SEAL_DOMAIN.to_vec();
    bytes.extend_from_slice(&header.id().hashable_bytes_with_tag());

    bytes
}

impl Consensus for DevAuthority {
    fn work(&self, _header: &BlockHeader) -> u256 {
        u256::ONE
    }

    fn difficulty(&self, _parent: &BlockHeader, _timestamp: u64) -> u128 {
        0
    }

    fn check(&self, header: &BlockHeader, seal: &Seal, parent: &BlockHeader, now: u64) -> Result<(), ConsensusError> {
        if header.producer != self.authority.address() {
            return Err(ConsensusError::WrongProducer);
        }

        if header.timestamp < parent.timestamp + self.slot_ms {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + MAX_DRIFT_MS {
            return Err(ConsensusError::FromTheFuture);
        }

        if header.difficulty != 0 {
            return Err(ConsensusError::WrongDifficulty {
                expected: 0,
                got: header.difficulty,
            });
        }

        let Seal::Signature(signature) = seal else {
            return Err(ConsensusError::BadSeal);
        };

        self.authority
            .verify_signature(&seal_bytes(header), signature)
            .map_err(|_| ConsensusError::BadSeal)
    }
}

#[document(
    summary = "Produces blocks as the authority: at each slot, signs the header's id at once.",
    category = "Consensus",
    see_also = "DevAuthority, Producer",
)]
pub struct AuthorityProducer {
    keypair: Keypair,
    slot_ms: u64,
}

impl AuthorityProducer {
    #[document(
        parent = "AuthorityProducer",
        summary = "Produces with the authority's keys.",
        params = {
            keypair = "The authority's keys.",
            slot_ms = "Milliseconds between blocks, from genesis.",
        },
    )]
    pub fn new(keypair: Keypair, slot_ms: u64) -> AuthorityProducer {
        AuthorityProducer { keypair, slot_ms }
    }
}

impl Producer for AuthorityProducer {
    fn address(&self) -> Address {
        self.keypair.public().address()
    }

    fn start_at(&self, parent: &BlockHeader) -> u64 {
        parent.timestamp + self.slot_ms
    }

    fn seal(&mut self, header: &BlockHeader) -> Option<Seal> {
        Some(Seal::Signature(self.keypair.sign(&seal_bytes(header))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::HashType;
    use crate::structs::u256::u256;
    use crate::structs::hash::Hash;

    const SLOT: u64 = 2_000;

    fn parent() -> BlockHeader {
        BlockHeader {
            parent: Hash::from_bytes(HashType::default_chain(), &[0; 32]).unwrap(),
            height: 0,
            timestamp: 10_000,
            producer: DevAuthority::dev_keypair().public().address(),
            difficulty: 0,
            tx_root: Hash::from_bytes(HashType::default_chain(), &[0; 32]).unwrap(),
            proof_root: Hash::from_bytes(HashType::default_chain(), &[0; 32]).unwrap(),
            gas_used: 0,
            base_price: u256::ZERO,
        }
    }

    fn child(timestamp: u64) -> BlockHeader {
        BlockHeader {
            parent: parent().id(),
            height: 1,
            timestamp,
            ..parent()
        }
    }

    fn rules() -> DevAuthority {
        DevAuthority::new(DevAuthority::dev_keypair().public(), SLOT)
    }

    fn producer() -> AuthorityProducer {
        AuthorityProducer::new(DevAuthority::dev_keypair(), SLOT)
    }

    fn sealed(header: &BlockHeader) -> Seal {
        producer().seal(header).unwrap()
    }

    #[test]
    fn the_authority_produces_once_per_slot() {
        let header = child(12_000);

        assert_eq!(producer().start_at(&parent()), 12_000);
        assert_eq!(producer().address(), parent().producer);
        assert_eq!(rules().check(&header, &sealed(&header), &parent(), 12_000), Ok(()));
        assert_eq!(rules().work(&header), u256::ONE);
    }

    #[test]
    fn bad_blocks_are_rejected() {
        let rules = rules();
        let early = child(11_999);
        let future = child(12_000 + MAX_DRIFT_MS + 1);
        let header = child(12_000);
        let hard = BlockHeader {
            difficulty: 5,
            ..child(12_000)
        };

        assert_eq!(rules.check(&early, &sealed(&early), &parent(), 12_000), Err(ConsensusError::Early));
        assert_eq!(rules.check(&future, &sealed(&future), &parent(), 12_000), Err(ConsensusError::FromTheFuture));
        assert_eq!(rules.check(&header, &Seal::None, &parent(), 12_000), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&header, &Seal::Work { nonce: 1 }, &parent(), 12_000), Err(ConsensusError::BadSeal));
        assert_eq!(rules.check(&header, &sealed(&child(13_000)), &parent(), 12_000), Err(ConsensusError::BadSeal));
        assert!(matches!(
            rules.check(&hard, &sealed(&hard), &parent(), 12_000),
            Err(ConsensusError::WrongDifficulty { .. })
        ));

        let stranger = Keypair::generate(DevAuthority::dev_keypair().public().key_type()).unwrap();
        let forged = BlockHeader {
            producer: stranger.public().address(),
            ..child(12_000)
        };
        let forged_seal = Seal::Signature(stranger.sign(&seal_bytes(&forged)));

        assert_eq!(rules.check(&forged, &forged_seal, &parent(), 12_000), Err(ConsensusError::WrongProducer));
    }
}
