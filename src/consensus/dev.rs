use {
    super::{Consensus, ConsensusError},
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PrivateKey, PublicKey, Signature},
    crate::structs::block::BlockHeader,
    r#macro::document,
};

/// How far ahead of this node's clock a block's timestamp may be.
pub const MAX_DRIFT_MS: u64 = 15_000;

/// Separates block seals from anything else a key might sign.
const SEAL_DOMAIN: &[u8] = b"chainv4 block";

/// The development authority's seed. Public on purpose: anyone can run a dev network with it,
/// and nothing of value should ever depend on it.
const DEV_SEED: [u8; 32] = *b"chainv4 dev authority, not safe!";

#[document(
    summary = "One known key produces every block, one per slot. For development and tests: the loop, execution and networking can run end to end before a real consensus exists.",
    category = "Consensus",
    see_also = "Consensus, Genesis",
)]
pub struct DevAuthority {
    authority: PublicKey,
    slot_ms: u64,
    keypair: Option<Keypair>,
}

impl DevAuthority {
    #[document(
        parent = "DevAuthority",
        summary = "Follows the authority, and produces when given its keys.",
        params = {
            authority = "The only key that may produce, from genesis.",
            slot_ms = "Milliseconds between blocks, from genesis.",
            keypair = "The authority's keys on the node that produces; None on every other node.",
        },
        panics = "Panics when keypair is not the authority's.",
    )]
    pub fn new(authority: PublicKey, slot_ms: u64, keypair: Option<Keypair>) -> DevAuthority {
        if let Some(keypair) = &keypair {
            assert_eq!(keypair.public(), authority, "the keypair must be the authority's");
        }

        DevAuthority {
            authority,
            slot_ms,
            keypair,
        }
    }

    #[document(
        parent = "DevAuthority",
        summary = "The well-known development authority. Never use it for anything of value: its seed is in the source.",
    )]
    pub fn dev_keypair() -> Keypair {
        #[cfg(feature = "ed25519")]
        let private = PrivateKey::Ed25519(DEV_SEED);
        #[cfg(not(feature = "ed25519"))]
        let private = PrivateKey::EdBlake(DEV_SEED);

        Keypair::from_private(private)
    }
}

fn seal_bytes(header: &BlockHeader) -> Vec<u8> {
    let mut bytes = SEAL_DOMAIN.to_vec();
    bytes.extend_from_slice(&header.id().hashable_bytes_with_tag());

    bytes
}

impl Consensus for DevAuthority {
    fn next_slot(&self, parent: &BlockHeader) -> Option<u64> {
        self.keypair.as_ref()?;

        Some(parent.timestamp + self.slot_ms)
    }

    fn producer(&self) -> Option<PublicKey> {
        self.keypair.as_ref().map(Keypair::public)
    }

    fn seal(&self, header: &BlockHeader) -> Option<Signature> {
        Some(self.keypair.as_ref()?.sign(&seal_bytes(header)))
    }

    fn check(
        &self,
        header: &BlockHeader,
        seal: Option<&Signature>,
        parent: &BlockHeader,
        now: u64,
    ) -> Result<(), ConsensusError> {
        if header.producer != self.authority {
            return Err(ConsensusError::WrongProducer);
        }

        if header.timestamp < parent.timestamp + self.slot_ms {
            return Err(ConsensusError::Early);
        }

        if header.timestamp > now + MAX_DRIFT_MS {
            return Err(ConsensusError::FromTheFuture);
        }

        let seal = seal.ok_or(ConsensusError::BadSeal)?;

        self.authority
            .verify_signature(&seal_bytes(header), seal)
            .map_err(|_| ConsensusError::BadSeal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::structs::hash::Hash;

    const SLOT: u64 = 2_000;

    fn parent() -> BlockHeader {
        BlockHeader {
            parent: Hash::Blake3_256([0; 32]),
            height: 0,
            timestamp: 10_000,
            producer: DevAuthority::dev_keypair().public(),
            tx_root: Hash::Blake3_256([0; 32]),
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

    fn producing() -> DevAuthority {
        let keypair = DevAuthority::dev_keypair();

        DevAuthority::new(keypair.public(), SLOT, Some(keypair))
    }

    #[test]
    fn the_authority_produces_once_per_slot() {
        let authority = producing();
        let header = child(12_000);
        let seal = authority.seal(&header);

        assert_eq!(authority.next_slot(&parent()), Some(12_000));
        assert_eq!(authority.check(&header, seal.as_ref(), &parent(), 12_000), Ok(()));
    }

    #[test]
    fn followers_never_produce_but_still_check() {
        let follower = DevAuthority::new(DevAuthority::dev_keypair().public(), SLOT, None);
        let header = child(12_000);
        let seal = producing().seal(&header);

        assert_eq!(follower.next_slot(&parent()), None);
        assert_eq!(follower.seal(&header), None);
        assert_eq!(follower.check(&header, seal.as_ref(), &parent(), 12_000), Ok(()));
    }

    #[test]
    fn bad_blocks_are_rejected() {
        let authority = producing();
        let early = child(11_999);
        let future = child(12_000 + MAX_DRIFT_MS + 1);
        let header = child(12_000);
        let other_seal = producing().seal(&child(13_000));

        assert_eq!(
            authority.check(&early, authority.seal(&early).as_ref(), &parent(), 12_000),
            Err(ConsensusError::Early)
        );
        assert_eq!(
            authority.check(&future, authority.seal(&future).as_ref(), &parent(), 12_000),
            Err(ConsensusError::FromTheFuture)
        );
        assert_eq!(
            authority.check(&header, None, &parent(), 12_000),
            Err(ConsensusError::BadSeal)
        );
        assert_eq!(
            authority.check(&header, other_seal.as_ref(), &parent(), 12_000),
            Err(ConsensusError::BadSeal)
        );

        let stranger = Keypair::generate(authority.authority.key_type()).unwrap();
        let forged = BlockHeader {
            producer: stranger.public(),
            ..child(12_000)
        };

        assert_eq!(
            authority.check(&forged, Some(&stranger.sign(&seal_bytes(&forged))), &parent(), 12_000),
            Err(ConsensusError::WrongProducer)
        );
    }
}
