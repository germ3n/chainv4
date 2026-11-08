//! Proofs of a withdrawal to Bitcoin, for BitVM's side of the Bitcoin bridge: that the chain of this
//! genesis queued a withdrawal through its Bitcoin gateway, as a range of blocks proven to end at a
//! checkpoint the header chain finalized from genesis shows. An operator who paid the withdrawal on
//! Bitcoin out of its own coins shows it to be reimbursed from the bridge's coins; a watcher who
//! finds no such withdrawal challenges it. The statement needs no word of the validators', and no
//! federation's: finality comes from the header chain, and what the blocks did from the range.
//!
//! In phase 1 watchers check it off Bitcoin, and the bridge's signers settle a challenge. In phase 2
//! Bitcoin's own script checks it, through BitVM's split verifier.

use {
    super::{ProofKind, ProvenHeaders, ProvenRange, verify_withdrawal},
    crate::structs::hash::{Hash, HashType},
    crate::structs::u256::u256,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A proof as another proof takes it inside: its kind, its public values and the proof.",
    category = "Proofs",
    see_also = "WithdrawalInput",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct InnerProof {
    #[document(summary = "The proof system it was made with.")]
    pub kind: ProofKind,
    #[document(summary = "Its public values.")]
    pub values: Vec<u8>,
    #[document(summary = "The proof.")]
    pub proof: Vec<u8>,
}

#[document(
    summary = "Everything a withdrawal proof runs on: the programs the header chain and the range were proven by, both proofs, and the withdrawal's leaf with its path in the range's btc_withdrawals tree.",
    category = "Proofs",
    see_also = "prove_withdrawal, ProvenWithdrawal",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct WithdrawalInput {
    #[document(summary = "The chain's genesis id, which both proofs must be of.")]
    pub genesis: Hash,
    #[document(summary = "The header-chain program.")]
    pub headers_program: [u8; 32],
    #[document(summary = "The range program.")]
    pub range_program: [u8; 32],
    #[document(summary = "The header-chain proof, finalizing the checkpoint the range ends at.")]
    pub headers: InnerProof,
    #[document(summary = "The range proof, ending at that checkpoint.")]
    pub range: InnerProof,
    #[document(summary = "The withdrawal's leaf, btc_withdrawal_digest's.")]
    pub digest: [u8; 32],
    #[document(summary = "Its path in the range's btc_withdrawals tree, withdrawal_proof's.")]
    pub path: Vec<[u8; 32]>,
}

#[document(
    summary = "What a withdrawal proof proves: that the chain of this genesis queued the withdrawal with this leaf through its Bitcoin gateway, in a range of blocks proven by the range program to end at the checkpoint the header-chain program finalized from genesis. Its public values are nine words, as abi.encode lays them out.",
    category = "Proofs",
    see_also = "prove_withdrawal, btc_withdrawal_digest",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ProvenWithdrawal {
    #[document(summary = "The chain's EVM chain id.")]
    pub chain_id: u64,
    #[document(summary = "The genesis id.")]
    pub genesis: Hash,
    #[document(summary = "The header-chain program that finalized the checkpoint.")]
    pub headers_program: [u8; 32],
    #[document(summary = "The range program that proved the blocks.")]
    pub range_program: [u8; 32],
    #[document(summary = "The finalized checkpoint the range ends at: its epoch.")]
    pub epoch: u64,
    #[document(summary = "Its block.")]
    pub block: Hash,
    #[document(summary = "Its height.")]
    pub height: u64,
    #[document(summary = "Its state root.")]
    pub state_root: Hash,
    #[document(summary = "The withdrawal's leaf.")]
    pub digest: [u8; 32],
}

impl ProvenWithdrawal {
    #[document(parent = "ProvenWithdrawal", summary = "How many 32-byte words the public values are.")]
    pub const WORDS: usize = 9;

    #[document(parent = "ProvenWithdrawal", summary = "The public values: abi.encode(chainId, genesis, headersProgram, rangeProgram, epoch, block, height, stateRoot, digest).")]
    pub fn public_values(&self) -> Vec<u8> {
        let number = |value: u64| u256::from(value).to_be_bytes();

        [
            number(self.chain_id),
            crate::structs::attestation::word(&self.genesis),
            self.headers_program,
            self.range_program,
            number(self.epoch),
            crate::structs::attestation::word(&self.block),
            number(self.height),
            crate::structs::attestation::word(&self.state_root),
            self.digest,
        ]
        .concat()
    }

    #[document(parent = "ProvenWithdrawal", summary = "A withdrawal from its public values, its hashes of the chain's kind; None for any that do not read.")]
    pub fn from_public_values(hash: HashType, bytes: &[u8]) -> Option<ProvenWithdrawal> {
        if bytes.len() != 32 * ProvenWithdrawal::WORDS {
            return None;
        }

        let words: Vec<[u8; 32]> = bytes.as_chunks::<32>().0.to_vec();
        let small = |at: usize| u64::try_from(u256::from_be_bytes(words[at])).ok();
        let hash_at = |at: usize| Hash::from_bytes(hash, &words[at]);

        Some(ProvenWithdrawal {
            chain_id: small(0)?,
            genesis: hash_at(1)?,
            headers_program: words[2],
            range_program: words[3],
            epoch: small(4)?,
            block: hash_at(5)?,
            height: small(6)?,
            state_root: hash_at(7)?,
            digest: words[8],
        })
    }
}

impl WithdrawalInput {
    #[document(parent = "WithdrawalInput", summary = "The input as bytes, as the zkVM reads it.")]
    pub fn to_bytes(&self) -> Vec<u8> {
        wincode::serialize(self).expect("a withdrawal input always encodes")
    }

    #[document(parent = "WithdrawalInput", summary = "An input from its bytes.")]
    pub fn from_bytes(bytes: &[u8]) -> Result<WithdrawalInput, String> {
        wincode::deserialize(bytes).map_err(|err| format!("a withdrawal input that does not decode: {err}"))
    }
}

#[document(
    summary = "Runs a withdrawal proof's input and says what it proves: what the withdrawal program is. The header-chain proof and the range proof are each checked by their kind's verifier under their programs; both must be of the genesis named, the range must end exactly at the checkpoint the header chain finalized, and the leaf must be in the range's btc_withdrawals tree by its path.",
    params = { input = "The input." },
    returns = "What it proves, or why it does not hold.",
    category = "Proofs",
    see_also = "WithdrawalInput, ProvenWithdrawal",
)]
pub fn prove_withdrawal(input: &WithdrawalInput) -> Result<ProvenWithdrawal, String> {
    let hash = input.genesis.algorithm();
    let check = |inner: &InnerProof, program: &[u8; 32], what: &str| -> Result<(), String> {
        super::verifier(inner.kind).ok_or_else(|| format!("this build checks no {} proofs", inner.kind.name()))?.verify(program, &inner.values, &inner.proof).map_err(|why| format!("the {what} proof: {why}"))
    };

    check(&input.headers, &input.headers_program, "header-chain")?;
    check(&input.range, &input.range_program, "range")?;
    let headers = ProvenHeaders::from_public_values(hash, &input.headers.values).ok_or("public values that are no header chain")?;
    let range = ProvenRange::from_public_values(hash, &input.range.values).ok_or("public values that are no range")?;

    if headers.genesis != input.genesis || range.genesis != input.genesis || headers.chain_id != range.chain_id {
        return Err("proofs of another chain".to_string());
    }

    if headers.program != input.headers_program {
        return Err("a header chain by another program".to_string());
    }

    let finalized = headers.finalized;

    if range.end.height != finalized.height || range.end.block != finalized.checkpoint.block || range.end.state_root != finalized.state_root {
        return Err(format!("a range ending at {}, not at the checkpoint the header chain finalized at {}", range.end.height, finalized.height));
    }

    if !verify_withdrawal(&range.btc_withdrawals, &input.digest, &input.path) {
        return Err("a withdrawal that is not in the range's Bitcoin withdrawals".to_string());
    }

    Ok(ProvenWithdrawal {
        chain_id: range.chain_id,
        genesis: input.genesis,
        headers_program: input.headers_program,
        range_program: input.range_program,
        epoch: finalized.checkpoint.epoch,
        block: finalized.checkpoint.block,
        height: finalized.height,
        state_root: finalized.state_root,
        digest: input.digest,
    })
}
