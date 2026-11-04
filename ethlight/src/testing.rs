//! Synthetic beacon chains for tests: sync committees of BLS keys made here, signing real-format
//! updates, and states and bodies made up around the roots a light client checks. Also a native
//! [`Crypto`], SHA-256 and zkcrypto's BLS. Small secret keys, made from a seed: for tests only.

use {
    crate::{
        Crypto, PublicKey, Root, Signature,
        forks::{DOMAIN_SYNC_COMMITTEE, Fork, Network, SLOTS_PER_HISTORICAL_ROOT, compute_domain, compute_signing_root},
        ssz::{self, chunk_u64, hash_pair, merkle_branch, merkleize},
        tx,
        types::{BeaconBlockHeader, ExecutionPayloadHeader, LightClientBootstrap, LightClientHeader, LightClientUpdate, SYNC_COMMITTEE_SIZE, SyncAggregate, SyncCommittee},
    },
    alloc::vec::Vec,
    bls12_381::{
        G1Affine, G1Projective, G2Affine, G2Prepared, G2Projective, Gt, Scalar,
        hash_to_curve::{ExpandMsgXmd, HashToCurve},
        multi_miller_loop,
    },
    sha2::{Digest, Sha256},
};

/// Ethereum's ciphersuite: proof-of-possession BLS, signatures on G2, hashed to it with SHA-256.
pub const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";

/// SHA-256 and BLS natively.
pub struct Native;

impl Crypto for Native {
    fn sha256(&self, data: &[u8]) -> [u8; 32] {
        Sha256::digest(data).into()
    }

    fn fast_aggregate_verify(&self, pubkeys: &[PublicKey], message: &[u8; 32], signature: &Signature) -> bool {
        fast_aggregate_verify(pubkeys, message, signature)
    }
}

/// A message hashed to G2, as Ethereum signs.
pub fn hash_to_g2(message: &[u8]) -> G2Projective {
    <G2Projective as HashToCurve<ExpandMsgXmd<Sha256>>>::hash_to_curve([message], DST)
}

/// FastAggregateVerify: no keys, a key or signature that is no point of its group, or one at
/// infinity, refused.
pub fn fast_aggregate_verify(pubkeys: &[PublicKey], message: &[u8], signature: &Signature) -> bool {
    let mut aggregate = G1Projective::identity();

    for key in pubkeys {
        match Option::<G1Affine>::from(G1Affine::from_compressed(key)) {
            Some(point) if !bool::from(point.is_identity()) => aggregate += point,
            _ => return false,
        }
    }

    let Some(signature) = Option::<G2Affine>::from(G2Affine::from_compressed(signature)).filter(|point| !bool::from(point.is_identity())) else {
        return false;
    };

    if pubkeys.is_empty() || bool::from(aggregate.is_identity()) {
        return false;
    }

    let hashed = G2Affine::from(hash_to_g2(message));
    let terms = [(&G1Affine::from(aggregate), &G2Prepared::from(hashed)), (&-G1Affine::generator(), &G2Prepared::from(signature))];

    multi_miller_loop(&terms).final_exponentiation() == Gt::identity()
}

/// A secret key's public key, compressed.
pub fn public_key(secret: &Scalar) -> PublicKey {
    G1Affine::from(G1Affine::generator() * secret).to_compressed()
}

/// A signature by a secret key, compressed.
pub fn sign(secret: &Scalar, message: &[u8]) -> Signature {
    G2Affine::from(hash_to_g2(message) * secret).to_compressed()
}

/// A sync committee whose secret keys are known.
pub struct Committee {
    pub secrets: Vec<Scalar>,
    pub committee: SyncCommittee,
}

impl Committee {
    /// 512 members, their keys made from a seed: two seeds share no member.
    pub fn new(seed: u32) -> Committee {
        let first = (u64::from(seed) << 32) + 1;
        let secrets: Vec<Scalar> = (0..SYNC_COMMITTEE_SIZE as u64).map(|index| Scalar::from(first + index)).collect();
        // Each key one generator on from the last, as each secret is one more: one multiplication.
        let mut points = Vec::with_capacity(SYNC_COMMITTEE_SIZE);
        points.push(G1Affine::generator() * secrets[0]);

        for index in 1..SYNC_COMMITTEE_SIZE {
            points.push(points[index - 1] + G1Affine::generator());
        }

        let aggregate = points.iter().fold(G1Projective::identity(), |total, point| total + point);
        let mut affine = alloc::vec![G1Affine::identity(); SYNC_COMMITTEE_SIZE];
        G1Projective::batch_normalize(&points, &mut affine);
        let pubkeys = affine.iter().map(G1Affine::to_compressed).collect();

        Committee { secrets, committee: SyncCommittee { pubkeys, aggregate_pubkey: G1Affine::from(aggregate).to_compressed() } }
    }

    /// The first so many members' aggregate signature over a message, and their bits.
    pub fn sign(&self, signers: usize, message: &[u8; 32]) -> SyncAggregate {
        let mut bits = [0u8; SYNC_COMMITTEE_SIZE / 8];
        let mut secret = Scalar::zero();

        for (index, member) in self.secrets.iter().enumerate().take(signers) {
            bits[index / 8] |= 1 << (index % 8);
            secret += member;
        }

        SyncAggregate { bits, signature: sign(&secret, message) }
    }
}

/// A made-up root, the same for the same tag, slot and place.
pub fn filler(tag: u8, slot: u64, index: u64) -> Root {
    let mut root = [tag; 32];
    root[..8].copy_from_slice(&slot.to_le_bytes());
    root[8..16].copy_from_slice(&index.to_le_bytes());

    root
}

/// A state's field index for the finalized checkpoint, the sync committees and the block roots.
pub const FINALIZED_CHECKPOINT: u64 = 20;
pub const CURRENT_SYNC_COMMITTEE: u64 = 22;
pub const NEXT_SYNC_COMMITTEE: u64 = 23;
pub const BLOCK_ROOTS: u64 = 5;

/// A beacon state, as its fields' roots: 32 leaves before Electra, 64 from it, all made up but
/// those set.
#[derive(Clone)]
pub struct State {
    pub fields: Vec<Root>,
}

impl State {
    pub fn new(fork: Fork, slot: u64) -> State {
        let count = if fork.electra() { 64 } else { 32 };

        State { fields: (0..count).map(|index| filler(0x5a, slot, index)).collect() }
    }

    fn depth(&self) -> u32 {
        ssz::ceil_log2(self.fields.len() as u64)
    }

    pub fn root<C: Crypto + ?Sized>(&self, crypto: &C) -> Root {
        merkleize(crypto, &self.fields, self.fields.len() as u64)
    }

    pub fn branch<C: Crypto + ?Sized>(&self, crypto: &C, field: u64) -> Vec<Root> {
        merkle_branch(crypto, &self.fields, self.depth(), field)
    }

    /// Sets the finalized checkpoint: an epoch and a block root.
    pub fn finalize<C: Crypto + ?Sized>(&mut self, crypto: &C, epoch: u64, root: &Root) {
        self.fields[FINALIZED_CHECKPOINT as usize] = hash_pair(crypto, &chunk_u64(epoch), root);
    }

    /// The branch from the finalized checkpoint's root.
    pub fn finality_branch<C: Crypto + ?Sized>(&self, crypto: &C, epoch: u64) -> Vec<Root> {
        [alloc::vec![chunk_u64(epoch)], self.branch(crypto, FINALIZED_CHECKPOINT)].concat()
    }
}

/// A state's block roots: the last 8192 slots'.
pub struct BlockRoots {
    pub roots: Vec<Root>,
}

impl BlockRoots {
    pub fn new(slot: u64) -> BlockRoots {
        BlockRoots { roots: (0..SLOTS_PER_HISTORICAL_ROOT).map(|index| filler(0xb1, slot, index)).collect() }
    }

    pub fn set(&mut self, slot: u64, root: Root) {
        self.roots[(slot % SLOTS_PER_HISTORICAL_ROOT) as usize] = root;
    }

    pub fn root<C: Crypto + ?Sized>(&self, crypto: &C) -> Root {
        merkleize(crypto, &self.roots, SLOTS_PER_HISTORICAL_ROOT)
    }

    /// The branch from the root at a slot up to the state's root, through its field.
    pub fn branch<C: Crypto + ?Sized>(&self, crypto: &C, state: &State, slot: u64) -> Vec<Root> {
        [merkle_branch(crypto, &self.roots, 13, slot % SLOTS_PER_HISTORICAL_ROOT), state.branch(crypto, BLOCK_ROOTS)].concat()
    }
}

/// An execution header made up around a block number and its transactions.
pub fn execution<C: Crypto + ?Sized, T: AsRef<[u8]>>(crypto: &C, block_number: u64, transactions: &[T]) -> ExecutionPayloadHeader {
    ExecutionPayloadHeader {
        parent_hash: filler(0xe0, block_number, 0),
        fee_recipient: [0xfe; 20],
        state_root: filler(0xe1, block_number, 0),
        receipts_root: filler(0xe2, block_number, 0),
        logs_bloom: [0x0b; 256],
        prev_randao: filler(0xe3, block_number, 0),
        block_number,
        gas_limit: 36_000_000,
        gas_used: 21_000 * transactions.len() as u64,
        timestamp: 1_765_000_000 + block_number,
        extra_data: b"ethlight".to_vec(),
        base_fee_per_gas: chunk_u64(7),
        block_hash: filler(0xe4, block_number, 0),
        transactions_root: tx::transactions_root(crypto, transactions),
        withdrawals_root: filler(0xe5, block_number, 0),
        blob_gas_used: 131_072,
        excess_blob_gas: 0,
    }
}

/// A light client header at a slot over a state root, its body made up around the execution
/// header; blob gas cleared before Deneb.
pub fn header<C: Crypto + ?Sized>(crypto: &C, network: &Network, slot: u64, state_root: Root, mut execution: ExecutionPayloadHeader) -> LightClientHeader {
    let fork = network.fork_at_slot(slot).fork;

    if !fork.has_blobs() {
        (execution.blob_gas_used, execution.excess_blob_gas) = (0, 0);
    }

    let mut body: Vec<Root> = (0..16).map(|index| filler(0xb0, slot, index)).collect();
    body[9] = execution.hash_tree_root(crypto, fork);
    let execution_branch = merkle_branch(crypto, &body, 4, 9).try_into().expect("4 nodes");
    let beacon = BeaconBlockHeader { slot, proposer_index: slot % 1_000_000, parent_root: filler(0xa0, slot, 0), state_root, body_root: merkleize(crypto, &body, 16) };

    LightClientHeader { beacon, execution, execution_branch }
}

/// A bootstrap at a slot for a committee, and the block root to trust it by.
pub fn bootstrap<C: Crypto + ?Sized>(crypto: &C, network: &Network, slot: u64, committee: &SyncCommittee) -> (LightClientBootstrap, Root) {
    let mut state = State::new(network.fork_at_slot(slot).fork, slot);
    state.fields[CURRENT_SYNC_COMMITTEE as usize] = committee.hash_tree_root(crypto);
    let header = header(crypto, network, slot, state.root(crypto), execution(crypto, slot, &[] as &[&[u8]]));
    let root = header.beacon.hash_tree_root(crypto);

    (LightClientBootstrap { header, current_sync_committee: committee.clone(), current_sync_committee_branch: state.branch(crypto, CURRENT_SYNC_COMMITTEE) }, root)
}

/// An update attested at a slot whose state finalizes a header and names a next committee, or
/// none; signed at a later slot by so many of a committee's first members, under the network's
/// domain for that slot.
#[allow(clippy::too_many_arguments)]
pub fn update<C: Crypto + ?Sized>(
    crypto: &C,
    network: &Network,
    signer: &Committee,
    signers: usize,
    attested_slot: u64,
    signature_slot: u64,
    finalized: &LightClientHeader,
    next: Option<&SyncCommittee>,
) -> LightClientUpdate {
    let fork = network.fork_at_slot(attested_slot).fork;
    let mut state = State::new(fork, attested_slot);
    let epoch = finalized.beacon.slot.div_ceil(network.slots_per_epoch);
    state.finalize(crypto, epoch, &finalized.beacon.hash_tree_root(crypto));

    if let Some(next) = next {
        state.fields[NEXT_SYNC_COMMITTEE as usize] = next.hash_tree_root(crypto);
    }

    let next_sync_committee_branch = match next {
        Some(_) => state.branch(crypto, NEXT_SYNC_COMMITTEE),
        None => alloc::vec![[0u8; 32]; ssz::floorlog2(fork.next_sync_committee_gindex()) as usize],
    };
    let attested_header = header(crypto, network, attested_slot, state.root(crypto), execution(crypto, attested_slot, &[] as &[&[u8]]));
    let mut update = LightClientUpdate {
        attested_header,
        next_sync_committee: next.cloned(),
        next_sync_committee_branch,
        finalized_header: finalized.clone(),
        finality_branch: state.finality_branch(crypto, epoch),
        sync_aggregate: SyncAggregate { bits: [0; SYNC_COMMITTEE_SIZE / 8], signature: [0; 96] },
        signature_slot,
    };
    let version = network.fork_at_slot(signature_slot.max(1) - 1).version;
    resign(crypto, signer, signers, &mut update, version, &network.genesis_validators_root);

    update
}

/// Signs an update again, by so many of a committee's first members, under any fork version and
/// genesis validators root.
pub fn resign<C: Crypto + ?Sized>(crypto: &C, signer: &Committee, signers: usize, update: &mut LightClientUpdate, version: [u8; 4], genesis_validators_root: &Root) {
    let domain = compute_domain(crypto, DOMAIN_SYNC_COMMITTEE, version, genesis_validators_root);
    let signing_root = compute_signing_root(crypto, &update.attested_header.beacon.hash_tree_root(crypto), &domain);
    update.sync_aggregate = signer.sign(signers, &signing_root);
}

/// RLP of a string.
pub fn rlp_string(bytes: &[u8]) -> Vec<u8> {
    match bytes {
        [byte] if *byte < 0x80 => alloc::vec![*byte],
        _ => [rlp_length(0x80, bytes.len()), bytes.to_vec()].concat(),
    }
}

/// RLP of a list of items already encoded.
pub fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let payload = items.concat();

    [rlp_length(0xc0, payload.len()), payload].concat()
}

/// A whole number's RLP: big-endian, no leading zeros, empty for 0.
pub fn number(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();

    rlp_string(&bytes[bytes.iter().position(|byte| *byte != 0).unwrap_or(8)..])
}

fn rlp_length(offset: u8, length: usize) -> Vec<u8> {
    match length < 56 {
        true => alloc::vec![offset + length as u8],
        false => {
            let bytes = (length as u64).to_be_bytes();
            let bytes = &bytes[bytes.iter().position(|byte| *byte != 0).expect("a length of 56 or more")..];

            [alloc::vec![offset + 55 + bytes.len() as u8], bytes.to_vec()].concat()
        }
    }
}

/// An EIP-1559 transaction to an address carrying calldata, with a made-up signature: as a
/// payload holds it, its type byte before its RLP.
pub fn eip1559(chain_id: u64, nonce: u64, to: &[u8; 20], data: &[u8]) -> Vec<u8> {
    let fields = [
        number(chain_id),
        number(nonce),
        number(1_000_000_000),
        number(30_000_000_000),
        number(30_000),
        rlp_string(to),
        number(0),
        rlp_string(data),
        rlp_list(&[]),
        number(1),
        rlp_string(&[0x11; 32]),
        rlp_string(&[0x22; 32]),
    ];

    [alloc::vec![2], rlp_list(&fields)].concat()
}
