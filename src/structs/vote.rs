use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::{Keypair, PublicKey, Signature},
    crate::crypto::verifiable::VerifyError,
    crate::structs::hash::Hash,
    r#macro::document,
    wincode::SchemaRead,
    wincode::SchemaWrite,
};

/// Separates votes from anything else a key might sign.
const VOTE_DOMAIN: &[u8] = b"chainv4 vote";

#[document(
    summary = "A block finality votes on: the block at the first height of an epoch, every checkpoint_interval blocks.",
    category = "Finality",
    see_also = "Vote, FinalityConfig",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, SchemaWrite, SchemaRead)]
pub struct Checkpoint {
    #[document(summary = "Which epoch's checkpoint it is. Its block is at height epoch × checkpoint_interval.")]
    pub epoch: u64,
    #[document(summary = "The block's id.")]
    pub block: Hash,
}

impl Hashable for Checkpoint {}

#[document(
    summary = "A validator's vote, in the style of Casper FFG: a link from a checkpoint it holds justified, the source, to a later one on the same branch, the target. When validators holding two thirds of the bonded weight vote for the same link, the target is justified; when the target is the source's direct successor, the source is final.",
    category = "Finality",
    see_also = "SignedVote, Justification",
    note = "An honest validator never signs two votes with the same target epoch, and never one whose source and target both lie strictly between another's. Either is proof of cheating, and costs the validator its bond.",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, std::hash::Hash, SchemaWrite, SchemaRead)]
pub struct Vote {
    #[document(summary = "A justified checkpoint the target builds on.")]
    pub source: Checkpoint,
    #[document(summary = "The checkpoint voted for.")]
    pub target: Checkpoint,
    #[document(summary = "The voting key named in the validator's bond.")]
    pub voter: PublicKey,
}

impl Hashable for Vote {}

impl Vote {
    #[document(
        parent = "Vote",
        summary = "Whether two votes by the same voter break the rules: a different vote for the same target epoch, or one surrounding the other.",
    )]
    pub fn conflicts(&self, other: &Vote) -> bool {
        if self.voter != other.voter || self == other {
            return false;
        }

        let surrounds = |outer: &Vote, inner: &Vote| {
            outer.source.epoch < inner.source.epoch && inner.target.epoch < outer.target.epoch
        };

        self.target.epoch == other.target.epoch || surrounds(self, other) || surrounds(other, self)
    }
}

#[document(
    summary = "A vote and its voter's signature, bound to one chain by its genesis id so it cannot be replayed on another.",
    category = "Finality",
    see_also = "Vote",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, std::hash::Hash, SchemaWrite, SchemaRead)]
pub struct SignedVote {
    #[document(summary = "The vote.")]
    pub vote: Vote,
    #[document(summary = "The voter's signature over the vote and the chain's genesis id.")]
    pub signature: Signature,
    #[document(summary = "Its validator's attestation of the link, for Ethereum: r, s and v over the link's AttestedLink digest, by the attester its validator registered, or None. One that is there must recover to that attester, or the vote is invalid; the vote's own signature does not cover it.")]
    pub attestation: Option<[u8; 65]>,
}

impl Hashable for SignedVote {}

fn signing_bytes(vote: &Vote, chain: &Hash) -> Vec<u8> {
    let mut bytes = VOTE_DOMAIN.to_vec();
    bytes.extend_from_slice(&chain.hashable_bytes_with_tag());
    bytes.extend_from_slice(&vote.hashable_bytes_with_tag());

    bytes
}

impl SignedVote {
    #[document(
        parent = "SignedVote",
        summary = "Signs a vote with the voting key.",
        params = {
            vote = "The vote; its voter must be keypair's public key.",
            keypair = "The voting key.",
            chain = "The chain's genesis id.",
        },
    )]
    pub fn sign(vote: Vote, keypair: &Keypair, chain: &Hash) -> SignedVote {
        SignedVote {
            signature: keypair.sign(&signing_bytes(&vote, chain)),
            vote,
            attestation: None,
        }
    }

    #[document(
        parent = "SignedVote",
        summary = "Checks the signature, and that the target comes after the source.",
        params = { chain = "The chain's genesis id." },
    )]
    pub fn verify(&self, chain: &Hash) -> Result<(), VerifyError> {
        if self.vote.target.epoch <= self.vote.source.epoch {
            return Err(VerifyError::BadVote);
        }

        self.vote
            .voter
            .verify_signature(&signing_bytes(&self.vote, chain), &self.signature)
    }
}

#[document(
    summary = "Proof that a checkpoint is justified: signed votes for one link, from validators holding at least two thirds of the bonded weight of the target's epoch. Nodes keep one per justified checkpoint and hand them to peers, and producers may put them in blocks, which pays every voter in them.",
    category = "Finality",
    see_also = "Vote, Block",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Justification {
    #[document(summary = "The link's source.")]
    pub source: Checkpoint,
    #[document(summary = "The checkpoint it justifies.")]
    pub target: Checkpoint,
    #[document(summary = "The votes for the link, one per voter, sorted by voter.")]
    pub votes: Vec<SignedVote>,
}

impl Hashable for Justification {}
