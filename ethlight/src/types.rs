//! The light client's containers, read from and written to SSZ as beacon nodes serve them
//! (`/eth/v1/beacon/light_client/*` with `Accept: application/octet-stream`), with their
//! hash_tree_roots.
//!
//! A light client container's layout depends on its fork: the execution payload header gained blob
//! gas in Deneb, and Electra's larger state lengthened the branches into it. An update's fork is
//! its attested header's, a bootstrap's its header's, as the specs and the beacon API pick them;
//! every header in a container takes the container's layout.

use {
    crate::{
        Crypto, Error, PublicKey, Root, Signature,
        forks::{EXECUTION_PAYLOAD_GINDEX, Fork, Network},
        ssz::{self, Reader, chunk_u64, floorlog2, hash_pair, merkleize, mix_in_length, pack},
    },
    alloc::vec::Vec,
};

/// Members of a sync committee.
pub const SYNC_COMMITTEE_SIZE: usize = 512;

/// A beacon block's header: what sync committees sign and roots chain to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BeaconBlockHeader {
    pub slot: u64,
    pub proposer_index: u64,
    pub parent_root: Root,
    pub state_root: Root,
    pub body_root: Root,
}

impl BeaconBlockHeader {
    /// Its SSZ, fixed.
    pub const SSZ_LEN: usize = 112;

    pub fn decode(bytes: &[u8]) -> Result<BeaconBlockHeader, Error> {
        let what = "a beacon block header is 112 bytes";

        if bytes.len() != Self::SSZ_LEN {
            return Err(Error::Malformed(what));
        }

        let mut reader = Reader::new(bytes, what);

        Ok(BeaconBlockHeader { slot: reader.u64()?, proposer_index: reader.u64()?, parent_root: reader.array()?, state_root: reader.array()?, body_root: reader.array()? })
    }

    pub fn encode(&self) -> [u8; Self::SSZ_LEN] {
        let mut out = [0u8; Self::SSZ_LEN];
        out[..8].copy_from_slice(&self.slot.to_le_bytes());
        out[8..16].copy_from_slice(&self.proposer_index.to_le_bytes());
        out[16..48].copy_from_slice(&self.parent_root);
        out[48..80].copy_from_slice(&self.state_root);
        out[80..112].copy_from_slice(&self.body_root);

        out
    }

    /// Its root: the block root a beacon node names the block by.
    pub fn hash_tree_root<C: Crypto + ?Sized>(&self, crypto: &C) -> Root {
        merkleize(crypto, &[chunk_u64(self.slot), chunk_u64(self.proposer_index), self.parent_root, self.state_root, self.body_root], 5)
    }
}

/// An execution block's header as the beacon chain holds it, Capella's, or Deneb's on with blob
/// gas: zero before Deneb.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutionPayloadHeader {
    pub parent_hash: Root,
    pub fee_recipient: [u8; 20],
    pub state_root: Root,
    pub receipts_root: Root,
    pub logs_bloom: [u8; 256],
    pub prev_randao: Root,
    pub block_number: u64,
    pub gas_limit: u64,
    pub gas_used: u64,
    pub timestamp: u64,
    /// At most 32 bytes.
    pub extra_data: Vec<u8>,
    /// A u256, little-endian.
    pub base_fee_per_gas: [u8; 32],
    pub block_hash: Root,
    pub transactions_root: Root,
    pub withdrawals_root: Root,
    pub blob_gas_used: u64,
    pub excess_blob_gas: u64,
}

impl Default for ExecutionPayloadHeader {
    fn default() -> ExecutionPayloadHeader {
        ExecutionPayloadHeader {
            parent_hash: [0; 32],
            fee_recipient: [0; 20],
            state_root: [0; 32],
            receipts_root: [0; 32],
            logs_bloom: [0; 256],
            prev_randao: [0; 32],
            block_number: 0,
            gas_limit: 0,
            gas_used: 0,
            timestamp: 0,
            extra_data: Vec::new(),
            base_fee_per_gas: [0; 32],
            block_hash: [0; 32],
            transactions_root: [0; 32],
            withdrawals_root: [0; 32],
            blob_gas_used: 0,
            excess_blob_gas: 0,
        }
    }
}

/// The most bytes of extra data an execution block carries.
pub const MAX_EXTRA_DATA_BYTES: usize = 32;

impl ExecutionPayloadHeader {
    /// Its SSZ's fixed part in a fork's layout, extra data's offset included.
    pub fn fixed_len(fork: Fork) -> usize {
        if fork.has_blobs() { 584 } else { 568 }
    }

    pub fn decode(bytes: &[u8], fork: Fork) -> Result<ExecutionPayloadHeader, Error> {
        let what = "an execution payload header does not read";

        if fork < Fork::Capella {
            return Err(Error::BeforeCapella);
        }

        let mut reader = Reader::new(bytes, what);
        let mut header = ExecutionPayloadHeader {
            parent_hash: reader.array()?,
            fee_recipient: reader.array()?,
            state_root: reader.array()?,
            receipts_root: reader.array()?,
            logs_bloom: reader.array()?,
            prev_randao: reader.array()?,
            block_number: reader.u64()?,
            gas_limit: reader.u64()?,
            gas_used: reader.u64()?,
            timestamp: reader.u64()?,
            ..ExecutionPayloadHeader::default()
        };
        let extra_offset = reader.offset()?;
        header.base_fee_per_gas = reader.array()?;
        header.block_hash = reader.array()?;
        header.transactions_root = reader.array()?;
        header.withdrawals_root = reader.array()?;

        if fork.has_blobs() {
            header.blob_gas_used = reader.u64()?;
            header.excess_blob_gas = reader.u64()?;
        }

        if extra_offset != reader.at() || bytes.len() - extra_offset > MAX_EXTRA_DATA_BYTES {
            return Err(Error::Malformed(what));
        }

        header.extra_data = bytes[extra_offset..].to_vec();

        Ok(header)
    }

    pub fn encode(&self, fork: Fork) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::fixed_len(fork) + self.extra_data.len());
        out.extend_from_slice(&self.parent_hash);
        out.extend_from_slice(&self.fee_recipient);
        out.extend_from_slice(&self.state_root);
        out.extend_from_slice(&self.receipts_root);
        out.extend_from_slice(&self.logs_bloom);
        out.extend_from_slice(&self.prev_randao);

        for number in [self.block_number, self.gas_limit, self.gas_used, self.timestamp] {
            out.extend_from_slice(&number.to_le_bytes());
        }

        out.extend_from_slice(&(Self::fixed_len(fork) as u32).to_le_bytes());
        out.extend_from_slice(&self.base_fee_per_gas);
        out.extend_from_slice(&self.block_hash);
        out.extend_from_slice(&self.transactions_root);
        out.extend_from_slice(&self.withdrawals_root);

        if fork.has_blobs() {
            out.extend_from_slice(&self.blob_gas_used.to_le_bytes());
            out.extend_from_slice(&self.excess_blob_gas.to_le_bytes());
        }

        out.extend_from_slice(&self.extra_data);

        out
    }

    /// Its root in a fork's layout: 15 fields under Capella, 17 from Deneb.
    pub fn hash_tree_root<C: Crypto + ?Sized>(&self, crypto: &C, fork: Fork) -> Root {
        let mut fee_recipient = [0u8; 32];
        fee_recipient[..20].copy_from_slice(&self.fee_recipient);
        let mut fields = Vec::with_capacity(17);
        fields.extend_from_slice(&[
            self.parent_hash,
            fee_recipient,
            self.state_root,
            self.receipts_root,
            merkleize(crypto, &pack(&self.logs_bloom), 8),
            self.prev_randao,
            chunk_u64(self.block_number),
            chunk_u64(self.gas_limit),
            chunk_u64(self.gas_used),
            chunk_u64(self.timestamp),
            mix_in_length(crypto, &merkleize(crypto, &pack(&self.extra_data), 1), self.extra_data.len() as u64),
            self.base_fee_per_gas,
            self.block_hash,
            self.transactions_root,
            self.withdrawals_root,
        ]);

        if fork.has_blobs() {
            fields.extend_from_slice(&[chunk_u64(self.blob_gas_used), chunk_u64(self.excess_blob_gas)]);
        }

        merkleize(crypto, &fields, fields.len() as u64)
    }
}

/// A light client's header, Capella on: the beacon block's header, its execution payload's header,
/// and the branch from that to the block's body.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LightClientHeader {
    pub beacon: BeaconBlockHeader,
    pub execution: ExecutionPayloadHeader,
    pub execution_branch: [Root; 4],
}

impl LightClientHeader {
    /// Its SSZ's fixed part: the beacon header, the execution header's offset and the branch.
    pub const FIXED_LEN: usize = BeaconBlockHeader::SSZ_LEN + 4 + 4 * 32;

    /// It, in a container fork's layout.
    pub fn decode(bytes: &[u8], fork: Fork) -> Result<LightClientHeader, Error> {
        let what = "a light client header does not read";
        let mut reader = Reader::new(bytes, what);
        let beacon = BeaconBlockHeader::decode(reader.bytes(BeaconBlockHeader::SSZ_LEN)?)?;
        let offset = reader.offset()?;
        let execution_branch = [reader.array()?, reader.array()?, reader.array()?, reader.array()?];

        if offset != Self::FIXED_LEN || bytes.len() < offset {
            return Err(Error::Malformed(what));
        }

        Ok(LightClientHeader { beacon, execution: ExecutionPayloadHeader::decode(&bytes[offset..], fork)?, execution_branch })
    }

    /// It alone, as a bootstrap or an ancestor proof carries one: in its own slot's fork's layout.
    pub fn decode_for(bytes: &[u8], network: &Network) -> Result<LightClientHeader, Error> {
        let slot = bytes.get(..8).ok_or(Error::Malformed("a light client header does not read"))?;
        let fork = network.fork_at_slot(u64::from_le_bytes(slot.try_into().expect("8 bytes"))).fork;

        LightClientHeader::decode(bytes, fork)
    }

    pub fn encode(&self, fork: Fork) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::FIXED_LEN + ExecutionPayloadHeader::fixed_len(fork));
        out.extend_from_slice(&self.beacon.encode());
        out.extend_from_slice(&(Self::FIXED_LEN as u32).to_le_bytes());

        for node in &self.execution_branch {
            out.extend_from_slice(node);
        }

        out.extend_from_slice(&self.execution.encode(fork));

        out
    }

    /// get_lc_execution_root: its execution header's root, by its own slot's fork: Capella's
    /// fields before Deneb, Deneb's from it.
    pub fn execution_root<C: Crypto + ?Sized>(&self, crypto: &C, network: &Network) -> Root {
        self.execution.hash_tree_root(crypto, network.fork_at_slot(self.beacon.slot).fork)
    }

    /// is_valid_light_client_header, Capella on: its execution header is in its body, and carries
    /// no blob gas before Deneb. Headers from before Capella are refused, as they hold no
    /// execution payload to prove anything with.
    pub fn check<C: Crypto + ?Sized>(&self, crypto: &C, network: &Network) -> Result<(), Error> {
        let fork = network.fork_at_slot(self.beacon.slot).fork;

        if fork < Fork::Capella {
            return Err(Error::BeforeCapella);
        }

        if !fork.has_blobs() && (self.execution.blob_gas_used != 0 || self.execution.excess_blob_gas != 0) {
            return Err(Error::BadExecutionBranch);
        }

        let valid = ssz::is_valid_merkle_branch(
            crypto,
            &self.execution_root(crypto, network),
            &self.execution_branch,
            floorlog2(EXECUTION_PAYLOAD_GINDEX),
            ssz::subtree_index(EXECUTION_PAYLOAD_GINDEX),
            &self.beacon.body_root,
        );

        valid.then_some(()).ok_or(Error::BadExecutionBranch)
    }
}

/// A sync committee: 512 keys, and their aggregate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncCommittee {
    /// Exactly SYNC_COMMITTEE_SIZE.
    pub pubkeys: Vec<PublicKey>,
    pub aggregate_pubkey: PublicKey,
}

impl SyncCommittee {
    /// Its SSZ, fixed: 513 keys of 48 bytes.
    pub const SSZ_LEN: usize = (SYNC_COMMITTEE_SIZE + 1) * 48;

    pub fn decode(bytes: &[u8]) -> Result<SyncCommittee, Error> {
        if bytes.len() != Self::SSZ_LEN {
            return Err(Error::Malformed("a sync committee is 24,624 bytes"));
        }

        let mut keys = bytes.as_chunks::<48>().0.iter().copied();
        let pubkeys = keys.by_ref().take(SYNC_COMMITTEE_SIZE).collect();

        Ok(SyncCommittee { pubkeys, aggregate_pubkey: keys.next().expect("513 keys") })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::SSZ_LEN);

        for key in self.pubkeys.iter().chain([&self.aggregate_pubkey]) {
            out.extend_from_slice(key);
        }

        out
    }

    /// Whether it is the same committee as another, compared by words.
    pub fn same_as(&self, other: &SyncCommittee) -> bool {
        ssz::same_bytes(self.pubkeys.as_flattened(), other.pubkeys.as_flattened()) && ssz::same_bytes(&self.aggregate_pubkey, &other.aggregate_pubkey)
    }

    /// Its root: each key's root, a 48-byte vector's, merkleized, beside the aggregate's.
    pub fn hash_tree_root<C: Crypto + ?Sized>(&self, crypto: &C) -> Root {
        let leaves: Vec<Root> = self.pubkeys.iter().map(|key| pubkey_root(crypto, key)).collect();

        hash_pair(crypto, &merkleize(crypto, &leaves, SYNC_COMMITTEE_SIZE as u64), &pubkey_root(crypto, &self.aggregate_pubkey))
    }
}

/// A BLS key's root: its 48 bytes as two chunks.
pub fn pubkey_root<C: Crypto + ?Sized>(crypto: &C, key: &PublicKey) -> Root {
    let mut chunks = [0u8; 64];
    chunks[..48].copy_from_slice(key);

    crypto.sha256(&chunks)
}

/// Which members of a sync committee signed, and their aggregate signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncAggregate {
    /// A bitvector of 512, the first member's bit the lowest of the first byte.
    pub bits: [u8; SYNC_COMMITTEE_SIZE / 8],
    pub signature: Signature,
}

impl SyncAggregate {
    /// Its SSZ, fixed.
    pub const SSZ_LEN: usize = SYNC_COMMITTEE_SIZE / 8 + 96;

    /// How many members signed.
    pub fn participants(&self) -> usize {
        self.bits.iter().map(|byte| byte.count_ones() as usize).sum()
    }

    /// Whether the member at an index signed.
    pub fn signed(&self, index: usize) -> bool {
        self.bits[index / 8] >> (index % 8) & 1 == 1
    }
}

/// An update: a header the sync committee signed, the header its state finalized, and the next
/// committee its state names, each with its branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightClientUpdate {
    pub attested_header: LightClientHeader,
    /// None for an update that brings none, whose committee and branch are all zeros.
    pub next_sync_committee: Option<SyncCommittee>,
    pub next_sync_committee_branch: Vec<Root>,
    pub finalized_header: LightClientHeader,
    pub finality_branch: Vec<Root>,
    pub sync_aggregate: SyncAggregate,
    pub signature_slot: u64,
}

impl LightClientUpdate {
    fn fixed_len(fork: Fork) -> usize {
        let (next, finality) = (floorlog2(fork.next_sync_committee_gindex()) as usize, floorlog2(fork.finalized_root_gindex()) as usize);

        4 + SyncCommittee::SSZ_LEN + 32 * next + 4 + 32 * finality + SyncAggregate::SSZ_LEN + 8
    }

    /// An update, in its attested header's fork's layout. One without a finalized header, a
    /// finality update's zeros, is refused: this client follows finality alone.
    pub fn decode(bytes: &[u8], network: &Network) -> Result<LightClientUpdate, Error> {
        let what = "a light client update does not read";
        let fork = network.fork_at_slot(ssz::slot_at_offset(bytes, what)?).fork;

        if fork < Fork::Capella {
            return Err(Error::BeforeCapella);
        }

        let mut reader = Reader::new(bytes, what);
        let attested_offset = reader.offset()?;
        let committee = reader.bytes(SyncCommittee::SSZ_LEN)?;
        let next_sync_committee_branch = reader.roots(floorlog2(fork.next_sync_committee_gindex()) as usize)?;
        let finalized_offset = reader.offset()?;
        let finality_branch = reader.roots(floorlog2(fork.finalized_root_gindex()) as usize)?;
        let bits = reader.array()?;
        let signature = reader.array()?;
        let signature_slot = reader.u64()?;

        if attested_offset != Self::fixed_len(fork) || finalized_offset < attested_offset || finalized_offset > bytes.len() {
            return Err(Error::Malformed(what));
        }

        let next_sync_committee = match ssz::all_zero(&next_sync_committee_branch) {
            true if ssz::zero_bytes(committee) => None,
            true => return Err(Error::Malformed("a next sync committee without its branch")),
            false => Some(SyncCommittee::decode(committee)?),
        };

        if ssz::all_zero(&finality_branch) {
            return Err(Error::NotFinality);
        }

        Ok(LightClientUpdate {
            attested_header: LightClientHeader::decode(&bytes[attested_offset..finalized_offset], fork)?,
            next_sync_committee,
            next_sync_committee_branch,
            finalized_header: LightClientHeader::decode(&bytes[finalized_offset..], fork)?,
            finality_branch,
            sync_aggregate: SyncAggregate { bits, signature },
            signature_slot,
        })
    }

    pub fn encode(&self, network: &Network) -> Vec<u8> {
        let fork = network.fork_at_slot(self.attested_header.beacon.slot).fork;
        let attested = self.attested_header.encode(fork);
        let mut out = Vec::with_capacity(Self::fixed_len(fork) + 2 * attested.len());
        out.extend_from_slice(&(Self::fixed_len(fork) as u32).to_le_bytes());

        match &self.next_sync_committee {
            Some(committee) => out.extend_from_slice(&committee.encode()),
            None => out.resize(out.len() + SyncCommittee::SSZ_LEN, 0),
        }

        for node in &self.next_sync_committee_branch {
            out.extend_from_slice(node);
        }

        out.extend_from_slice(&((Self::fixed_len(fork) + attested.len()) as u32).to_le_bytes());

        for node in &self.finality_branch {
            out.extend_from_slice(node);
        }

        out.extend_from_slice(&self.sync_aggregate.bits);
        out.extend_from_slice(&self.sync_aggregate.signature);
        out.extend_from_slice(&self.signature_slot.to_le_bytes());
        out.extend_from_slice(&attested);
        out.extend_from_slice(&self.finalized_header.encode(fork));

        out
    }
}

impl LightClientUpdate {
    fn finality_fixed_len(fork: Fork) -> usize {
        4 + 4 + 32 * floorlog2(fork.finalized_root_gindex()) as usize + SyncAggregate::SSZ_LEN + 8
    }

    /// A finality update, as a beacon node serves it at /eth/v1/beacon/light_client/finality_update:
    /// an update's fields without the next committee, read as an update that brings none, which is
    /// how the relay takes one. In its attested header's fork's layout.
    pub fn decode_finality(bytes: &[u8], network: &Network) -> Result<LightClientUpdate, Error> {
        let what = "a light client finality update does not read";
        let fork = network.fork_at_slot(ssz::slot_at_offset(bytes, what)?).fork;

        if fork < Fork::Capella {
            return Err(Error::BeforeCapella);
        }

        let mut reader = Reader::new(bytes, what);
        let attested_offset = reader.offset()?;
        let finalized_offset = reader.offset()?;
        let finality_branch = reader.roots(floorlog2(fork.finalized_root_gindex()) as usize)?;
        let bits = reader.array()?;
        let signature = reader.array()?;
        let signature_slot = reader.u64()?;

        if attested_offset != Self::finality_fixed_len(fork) || finalized_offset < attested_offset || finalized_offset > bytes.len() {
            return Err(Error::Malformed(what));
        }

        if ssz::all_zero(&finality_branch) {
            return Err(Error::NotFinality);
        }

        Ok(LightClientUpdate {
            attested_header: LightClientHeader::decode(&bytes[attested_offset..finalized_offset], fork)?,
            next_sync_committee: None,
            next_sync_committee_branch: alloc::vec![[0u8; 32]; floorlog2(fork.next_sync_committee_gindex()) as usize],
            finalized_header: LightClientHeader::decode(&bytes[finalized_offset..], fork)?,
            finality_branch,
            sync_aggregate: SyncAggregate { bits, signature },
            signature_slot,
        })
    }

    /// Its finality update, as a beacon node serves one: its next committee left out.
    pub fn encode_finality(&self, network: &Network) -> Vec<u8> {
        let fork = network.fork_at_slot(self.attested_header.beacon.slot).fork;
        let attested = self.attested_header.encode(fork);
        let fixed = Self::finality_fixed_len(fork);
        let mut out = Vec::with_capacity(fixed + 2 * attested.len());
        out.extend_from_slice(&(fixed as u32).to_le_bytes());
        out.extend_from_slice(&((fixed + attested.len()) as u32).to_le_bytes());

        for node in &self.finality_branch {
            out.extend_from_slice(node);
        }

        out.extend_from_slice(&self.sync_aggregate.bits);
        out.extend_from_slice(&self.sync_aggregate.signature);
        out.extend_from_slice(&self.signature_slot.to_le_bytes());
        out.extend_from_slice(&attested);
        out.extend_from_slice(&self.finalized_header.encode(fork));

        out
    }
}

/// What a light client starts from: a header, its state's current sync committee, and the branch
/// between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightClientBootstrap {
    pub header: LightClientHeader,
    pub current_sync_committee: SyncCommittee,
    pub current_sync_committee_branch: Vec<Root>,
}

impl LightClientBootstrap {
    fn fixed_len(fork: Fork) -> usize {
        4 + SyncCommittee::SSZ_LEN + 32 * floorlog2(fork.current_sync_committee_gindex()) as usize
    }

    /// A bootstrap, in its header's fork's layout.
    pub fn decode(bytes: &[u8], network: &Network) -> Result<LightClientBootstrap, Error> {
        let what = "a light client bootstrap does not read";
        let fork = network.fork_at_slot(ssz::slot_at_offset(bytes, what)?).fork;

        if fork < Fork::Capella {
            return Err(Error::BeforeCapella);
        }

        let mut reader = Reader::new(bytes, what);
        let offset = reader.offset()?;
        let current_sync_committee = SyncCommittee::decode(reader.bytes(SyncCommittee::SSZ_LEN)?)?;
        let current_sync_committee_branch = reader.roots(floorlog2(fork.current_sync_committee_gindex()) as usize)?;

        if offset != Self::fixed_len(fork) {
            return Err(Error::Malformed(what));
        }

        Ok(LightClientBootstrap { header: LightClientHeader::decode(&bytes[offset..], fork)?, current_sync_committee, current_sync_committee_branch })
    }

    pub fn encode(&self, network: &Network) -> Vec<u8> {
        let fork = network.fork_at_slot(self.header.beacon.slot).fork;
        let mut out = Vec::with_capacity(Self::fixed_len(fork) + 1024);
        out.extend_from_slice(&(Self::fixed_len(fork) as u32).to_le_bytes());
        out.extend_from_slice(&self.current_sync_committee.encode());

        for node in &self.current_sync_committee_branch {
            out.extend_from_slice(node);
        }

        out.extend_from_slice(&self.header.encode(fork));

        out
    }
}

/// What a light client keeps of a finalized header, as the eth_relay contract stores it: the
/// beacon header and its root, and the execution block's number, hash, time and roots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Finalized {
    pub root: Root,
    pub beacon: BeaconBlockHeader,
    pub block_number: u64,
    pub block_hash: Root,
    pub timestamp: u64,
    pub state_root: Root,
    pub receipts_root: Root,
    pub transactions_root: Root,
}

impl Finalized {
    /// Its record, fixed:
    ///
    /// | Bytes | What |
    /// | --- | --- |
    /// | 0..32 | the beacon block root |
    /// | 32..144 | the beacon block header, as SSZ: slot, proposer index, parent, state and body roots |
    /// | 144..152 | the execution block's number, little-endian |
    /// | 152..184 | its hash |
    /// | 184..192 | its timestamp, little-endian |
    /// | 192..224 | its state root |
    /// | 224..256 | its receipts root |
    /// | 256..288 | its transactions root |
    pub const RECORD_LEN: usize = 288;

    /// What is kept of a header, its root worked out.
    pub fn from_header<C: Crypto + ?Sized>(crypto: &C, header: &LightClientHeader) -> Finalized {
        Finalized {
            root: header.beacon.hash_tree_root(crypto),
            beacon: header.beacon.clone(),
            block_number: header.execution.block_number,
            block_hash: header.execution.block_hash,
            timestamp: header.execution.timestamp,
            state_root: header.execution.state_root,
            receipts_root: header.execution.receipts_root,
            transactions_root: header.execution.transactions_root,
        }
    }

    pub fn encode(&self) -> [u8; Self::RECORD_LEN] {
        let mut out = [0u8; Self::RECORD_LEN];
        out[..32].copy_from_slice(&self.root);
        out[32..144].copy_from_slice(&self.beacon.encode());
        out[144..152].copy_from_slice(&self.block_number.to_le_bytes());
        out[152..184].copy_from_slice(&self.block_hash);
        out[184..192].copy_from_slice(&self.timestamp.to_le_bytes());
        out[192..224].copy_from_slice(&self.state_root);
        out[224..256].copy_from_slice(&self.receipts_root);
        out[256..288].copy_from_slice(&self.transactions_root);

        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Finalized> {
        if bytes.len() != Self::RECORD_LEN {
            return None;
        }

        let root = |at: usize| -> Root { bytes[at..at + 32].try_into().expect("32 bytes") };
        let number = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().expect("8 bytes"));

        Some(Finalized {
            root: root(0),
            beacon: BeaconBlockHeader::decode(&bytes[32..144]).ok()?,
            block_number: number(144),
            block_hash: root(152),
            timestamp: number(184),
            state_root: root(192),
            receipts_root: root(224),
            transactions_root: root(256),
        })
    }
}
