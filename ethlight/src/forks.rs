//! Beacon-chain networks: each one's own definition, its slot timing, fork schedule, genesis
//! validators root and genesis time, and the signing domains sync committees sign under.
//!
//! Mainnet, Sepolia and Hoodi are built in, each through the latest fork it has activated: Fulu
//! (Fusaka) on all three. Blob-parameter-only forks change no fork version, so they change nothing
//! here. Any other network running the beacon chain's light client protocol on the mainnet preset
//! (512-member sync committees, 8,192 block roots kept), such as Gnosis with its 5-second slots and
//! 16-slot epochs, is a [`Network`] of its own, made by hand or decoded from its SSZ.
//!
//! A fork past a schedule's last signs with a version the schedule lacks, so its updates fail the
//! signature check until the schedule names it: the client stops rather than follow it blindly.

use {
    crate::{Crypto, Error, Root, ssz},
    alloc::{borrow::Cow, string::String, vec::Vec},
};

/// How many block roots a state keeps: those of the last 8,192 slots, on the mainnet preset every
/// network here runs.
pub const SLOTS_PER_HISTORICAL_ROOT: u64 = 8192;
/// What sync committees sign under.
pub const DOMAIN_SYNC_COMMITTEE: [u8; 4] = [7, 0, 0, 0];

/// The execution payload's generalized index in a block body, Capella on.
pub const EXECUTION_PAYLOAD_GINDEX: u64 = 25;
/// The finalized checkpoint's root in a state, before Electra and from it.
pub const FINALIZED_ROOT_GINDEX: u64 = 105;
pub const FINALIZED_ROOT_GINDEX_ELECTRA: u64 = 169;
/// The current sync committee in a state, before Electra and from it.
pub const CURRENT_SYNC_COMMITTEE_GINDEX: u64 = 54;
pub const CURRENT_SYNC_COMMITTEE_GINDEX_ELECTRA: u64 = 86;
/// The next sync committee in a state, before Electra and from it.
pub const NEXT_SYNC_COMMITTEE_GINDEX: u64 = 55;
pub const NEXT_SYNC_COMMITTEE_GINDEX_ELECTRA: u64 = 87;
/// The block roots vector, the state's sixth field, before Electra and from it.
pub const BLOCK_ROOTS_GINDEX: u64 = 37;
pub const BLOCK_ROOTS_GINDEX_ELECTRA: u64 = 69;

/// A fork of the beacon chain, oldest first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Fork {
    Phase0 = 0,
    Altair = 1,
    Bellatrix = 2,
    Capella = 3,
    Deneb = 4,
    Electra = 5,
    Fulu = 6,
}

impl Fork {
    /// Every fork, oldest first.
    pub const ALL: [Fork; 7] = [Fork::Phase0, Fork::Altair, Fork::Bellatrix, Fork::Capella, Fork::Deneb, Fork::Electra, Fork::Fulu];

    /// The fork by its pinned tag, as a network's SSZ writes it.
    pub fn from_tag(tag: u8) -> Option<Fork> {
        Fork::ALL.get(usize::from(tag)).copied()
    }

    /// Its name, lowercase, as the beacon API's Eth-Consensus-Version names it.
    pub fn name(self) -> &'static str {
        match self {
            Fork::Phase0 => "phase0",
            Fork::Altair => "altair",
            Fork::Bellatrix => "bellatrix",
            Fork::Capella => "capella",
            Fork::Deneb => "deneb",
            Fork::Electra => "electra",
            Fork::Fulu => "fulu",
        }
    }

    /// Whether its execution payload header carries blob gas: Deneb on.
    pub fn has_blobs(self) -> bool {
        self >= Fork::Deneb
    }

    /// Whether its state is Electra's, of 64 leaves rather than 32, which moved every state field's
    /// generalized index.
    pub fn electra(self) -> bool {
        self >= Fork::Electra
    }

    /// The finalized checkpoint's root in its state.
    pub fn finalized_root_gindex(self) -> u64 {
        if self.electra() { FINALIZED_ROOT_GINDEX_ELECTRA } else { FINALIZED_ROOT_GINDEX }
    }

    /// The current sync committee in its state.
    pub fn current_sync_committee_gindex(self) -> u64 {
        if self.electra() { CURRENT_SYNC_COMMITTEE_GINDEX_ELECTRA } else { CURRENT_SYNC_COMMITTEE_GINDEX }
    }

    /// The next sync committee in its state.
    pub fn next_sync_committee_gindex(self) -> u64 {
        if self.electra() { NEXT_SYNC_COMMITTEE_GINDEX_ELECTRA } else { NEXT_SYNC_COMMITTEE_GINDEX }
    }

    /// The block roots vector in its state.
    pub fn block_roots_gindex(self) -> u64 {
        if self.electra() { BLOCK_ROOTS_GINDEX_ELECTRA } else { BLOCK_ROOTS_GINDEX }
    }
}

/// A fork as a network schedules it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ForkAt {
    pub fork: Fork,
    pub version: [u8; 4],
    pub epoch: u64,
}

/// A beacon-chain network the light client follows: everything about it the light client's
/// rules depend on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    /// Its name, such as mainnet: 1 to 64 bytes of UTF-8.
    pub name: Cow<'static, str>,
    /// The root of its genesis validators, which every signing domain mixes in.
    pub genesis_validators_root: Root,
    /// Its genesis, in seconds since 1970: slot 0's start.
    pub genesis_time: u64,
    /// 12 on Ethereum, 5 on Gnosis.
    pub seconds_per_slot: u64,
    /// 32 on Ethereum, 16 on Gnosis.
    pub slots_per_epoch: u64,
    /// How long a sync committee serves: 256 on Ethereum, 512 on Gnosis.
    pub epochs_per_sync_committee_period: u64,
    /// Its forks from genesis, Phase 0 first at epoch 0, each later fork at its epoch, in order.
    pub forks: Cow<'static, [ForkAt]>,
}

const fn at(fork: Fork, version: [u8; 4], epoch: u64) -> ForkAt {
    ForkAt { fork, version, epoch }
}

/// Ethereum's mainnet.
pub static MAINNET: Network = Network {
    name: Cow::Borrowed("mainnet"),
    genesis_validators_root: hex32(b"4b363db94e286120d76eb905340fdd4e54bfe9f06bf33ff6cf5ad27f511bfe95"),
    genesis_time: 1_606_824_023,
    seconds_per_slot: 12,
    slots_per_epoch: 32,
    epochs_per_sync_committee_period: 256,
    forks: Cow::Borrowed(&[
        at(Fork::Phase0, [0x00, 0, 0, 0], 0),
        at(Fork::Altair, [0x01, 0, 0, 0], 74_240),
        at(Fork::Bellatrix, [0x02, 0, 0, 0], 144_896),
        at(Fork::Capella, [0x03, 0, 0, 0], 194_048),
        at(Fork::Deneb, [0x04, 0, 0, 0], 269_568),
        at(Fork::Electra, [0x05, 0, 0, 0], 364_032),
        at(Fork::Fulu, [0x06, 0, 0, 0], 411_392),
    ]),
};

/// The Sepolia testnet.
pub static SEPOLIA: Network = Network {
    name: Cow::Borrowed("sepolia"),
    genesis_validators_root: hex32(b"d8ea171f3c94aea21ebc42a1ed61052acf3f9209c00e4efbaaddac09ed9b8078"),
    genesis_time: 1_655_733_600,
    seconds_per_slot: 12,
    slots_per_epoch: 32,
    epochs_per_sync_committee_period: 256,
    forks: Cow::Borrowed(&[
        at(Fork::Phase0, [0x90, 0x00, 0x00, 0x69], 0),
        at(Fork::Altair, [0x90, 0x00, 0x00, 0x70], 50),
        at(Fork::Bellatrix, [0x90, 0x00, 0x00, 0x71], 100),
        at(Fork::Capella, [0x90, 0x00, 0x00, 0x72], 56_832),
        at(Fork::Deneb, [0x90, 0x00, 0x00, 0x73], 132_608),
        at(Fork::Electra, [0x90, 0x00, 0x00, 0x74], 222_464),
        at(Fork::Fulu, [0x90, 0x00, 0x00, 0x75], 272_640),
    ]),
};

/// The Hoodi testnet, whose forks up to Deneb were all at genesis.
pub static HOODI: Network = Network {
    name: Cow::Borrowed("hoodi"),
    genesis_validators_root: hex32(b"212f13fc4df078b6cb7db228f1c8307566dcecf900867401a92023d7ba99cb5f"),
    genesis_time: 1_742_213_400,
    seconds_per_slot: 12,
    slots_per_epoch: 32,
    epochs_per_sync_committee_period: 256,
    forks: Cow::Borrowed(&[
        at(Fork::Phase0, [0x10, 0x00, 0x09, 0x10], 0),
        at(Fork::Altair, [0x20, 0x00, 0x09, 0x10], 0),
        at(Fork::Bellatrix, [0x30, 0x00, 0x09, 0x10], 0),
        at(Fork::Capella, [0x40, 0x00, 0x09, 0x10], 0),
        at(Fork::Deneb, [0x50, 0x00, 0x09, 0x10], 0),
        at(Fork::Electra, [0x60, 0x00, 0x09, 0x10], 2_048),
        at(Fork::Fulu, [0x70, 0x00, 0x09, 0x10], 50_688),
    ]),
};

/// Every network built in.
pub static NETWORKS: [&Network; 3] = [&MAINNET, &SEPOLIA, &HOODI];

/// The longest name a network may have, in bytes.
pub const MAX_NETWORK_NAME: usize = 64;
/// The most forks a network may list: room for forks to come.
pub const MAX_FORKS: usize = 32;
/// A fork's SSZ: its tag, its version and its epoch.
const FORK_SSZ_LEN: usize = 1 + 4 + 8;
/// A network's SSZ's fixed part: the name's offset, the root, four numbers and the forks' offset.
const NETWORK_FIXED_LEN: usize = 4 + 32 + 8 * 4 + 4;

impl Network {
    /// A built-in network by its name: mainnet, sepolia or hoodi.
    pub fn by_name(name: &str) -> Option<&'static Network> {
        NETWORKS.into_iter().find(|network| network.name == name)
    }

    /// The fork in force at an epoch: the last one scheduled at it or before.
    pub fn fork_at_epoch(&self, epoch: u64) -> ForkAt {
        *self.forks.iter().rev().find(|fork| fork.epoch <= epoch).unwrap_or(&self.forks[0])
    }

    /// The fork in force at a slot.
    pub fn fork_at_slot(&self, slot: u64) -> ForkAt {
        self.fork_at_epoch(self.epoch_at_slot(slot))
    }

    /// The epoch a slot is in.
    pub fn epoch_at_slot(&self, slot: u64) -> u64 {
        slot / self.slots_per_epoch.max(1)
    }

    /// Slots a sync committee serves: 8,192 on Ethereum.
    pub fn slots_per_period(&self) -> u64 {
        self.slots_per_epoch.saturating_mul(self.epochs_per_sync_committee_period).max(1)
    }

    /// The sync committee period a slot is in.
    pub fn period_at_slot(&self, slot: u64) -> u64 {
        slot / self.slots_per_period()
    }

    /// The slot at a time, in seconds since 1970: 0 before genesis.
    pub fn slot_at(&self, unix_seconds: u64) -> u64 {
        unix_seconds.saturating_sub(self.genesis_time) / self.seconds_per_slot.max(1)
    }

    /// The domain a signature of this network's under a fork version is made in.
    pub fn domain<C: Crypto + ?Sized>(&self, crypto: &C, domain_type: [u8; 4], fork_version: [u8; 4]) -> [u8; 32] {
        compute_domain(crypto, domain_type, fork_version, &self.genesis_validators_root)
    }

    /// Why it could not be followed:
    /// - a name empty or past 64 bytes;
    /// - a slot, epoch or period of 0;
    /// - no forks, or more than 32;
    /// - a first fork other than Phase 0 at epoch 0;
    /// - forks out of order, by fork or by epoch;
    /// - two forks with one version.
    pub fn problem(&self) -> Option<&'static str> {
        if self.name.is_empty() || self.name.len() > MAX_NETWORK_NAME {
            return Some("a network's name is 1 to 64 bytes");
        }

        if self.seconds_per_slot == 0 || self.slots_per_epoch == 0 || self.epochs_per_sync_committee_period == 0 {
            return Some("a network's slots, epochs and periods are at least 1");
        }

        let forks = &self.forks[..];

        match forks.first() {
            None => return Some("a network has at least one fork"),
            Some(first) if first.fork != Fork::Phase0 || first.epoch != 0 => return Some("a network's first fork is phase0 at epoch 0"),
            _ if forks.len() > MAX_FORKS => return Some("a network lists at most 32 forks"),
            _ => {}
        }

        if !forks.windows(2).all(|pair| pair[0].fork < pair[1].fork && pair[0].epoch <= pair[1].epoch) {
            return Some("a network's forks are in order, by fork and by epoch");
        }

        if forks.iter().enumerate().any(|(index, fork)| forks[..index].iter().any(|before| before.version == fork.version)) {
            return Some("each of a network's forks has a version of its own");
        }

        None
    }

    /// Its SSZ, as the container
    /// `{ name: ByteList[64], genesis_validators_root: Bytes32, genesis_time: uint64,
    /// seconds_per_slot: uint64, slots_per_epoch: uint64, epochs_per_sync_committee_period: uint64,
    /// forks: List[{ fork: uint8, version: Bytes4, epoch: uint64 }, 32] }`, each fork by its pinned
    /// tag.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(NETWORK_FIXED_LEN + self.name.len() + FORK_SSZ_LEN * self.forks.len());
        out.extend_from_slice(&(NETWORK_FIXED_LEN as u32).to_le_bytes());
        out.extend_from_slice(&self.genesis_validators_root);

        for number in [self.genesis_time, self.seconds_per_slot, self.slots_per_epoch, self.epochs_per_sync_committee_period] {
            out.extend_from_slice(&number.to_le_bytes());
        }

        out.extend_from_slice(&((NETWORK_FIXED_LEN + self.name.len()) as u32).to_le_bytes());
        out.extend_from_slice(self.name.as_bytes());

        for fork in self.forks.iter() {
            out.push(fork.fork as u8);
            out.extend_from_slice(&fork.version);
            out.extend_from_slice(&fork.epoch.to_le_bytes());
        }

        out
    }

    /// A network from its SSZ, checked as problem checks it, and its name as UTF-8: what a chain
    /// carrying a network of its own in its settings reads it back with.
    pub fn decode(bytes: &[u8]) -> Result<Network, Error> {
        let what = "a network does not read";
        let mut reader = ssz::Reader::new(bytes, what);
        let name_offset = reader.offset()?;
        let genesis_validators_root = reader.array()?;
        let (genesis_time, seconds_per_slot, slots_per_epoch, epochs_per_sync_committee_period) = (reader.u64()?, reader.u64()?, reader.u64()?, reader.u64()?);
        let forks_offset = reader.offset()?;

        if name_offset != NETWORK_FIXED_LEN || forks_offset < name_offset || forks_offset > bytes.len() || !(bytes.len() - forks_offset).is_multiple_of(FORK_SSZ_LEN) {
            return Err(Error::Malformed(what));
        }

        let name = String::from(core::str::from_utf8(&bytes[name_offset..forks_offset]).map_err(|_| Error::Malformed("a network's name is not UTF-8"))?);
        let forks = bytes[forks_offset..]
            .as_chunks::<FORK_SSZ_LEN>()
            .0
            .iter()
            .map(|fork| {
                Some(ForkAt {
                    fork: Fork::from_tag(fork[0])?,
                    version: fork[1..5].try_into().expect("4 bytes"),
                    epoch: u64::from_le_bytes(fork[5..].try_into().expect("8 bytes")),
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(Error::Malformed("a network names a fork this build lacks"))?;
        let network = Network {
            name: Cow::Owned(name),
            genesis_validators_root,
            genesis_time,
            seconds_per_slot,
            slots_per_epoch,
            epochs_per_sync_committee_period,
            forks: Cow::Owned(forks),
        };

        match network.problem() {
            Some(problem) => Err(Error::Malformed(problem)),
            None => Ok(network),
        }
    }

    /// Its SSZ root: what names the whole definition in 32 bytes.
    pub fn hash_tree_root<C: Crypto + ?Sized>(&self, crypto: &C) -> Root {
        let name = ssz::mix_in_length(crypto, &ssz::merkleize(crypto, &ssz::pack(self.name.as_bytes()), (MAX_NETWORK_NAME / 32) as u64), self.name.len() as u64);
        let forks: Vec<Root> = self
            .forks
            .iter()
            .map(|fork| {
                let (mut tag, mut version) = ([0u8; 32], [0u8; 32]);
                tag[0] = fork.fork as u8;
                version[..4].copy_from_slice(&fork.version);

                ssz::merkleize(crypto, &[tag, version, ssz::chunk_u64(fork.epoch)], 3)
            })
            .collect();
        let forks = ssz::mix_in_length(crypto, &ssz::merkleize(crypto, &forks, MAX_FORKS as u64), self.forks.len() as u64);
        let fields = [
            name,
            self.genesis_validators_root,
            ssz::chunk_u64(self.genesis_time),
            ssz::chunk_u64(self.seconds_per_slot),
            ssz::chunk_u64(self.slots_per_epoch),
            ssz::chunk_u64(self.epochs_per_sync_committee_period),
            forks,
        ];

        ssz::merkleize(crypto, &fields, 7)
    }
}

/// compute_fork_data_root: ForkData's root, the fork version beside the genesis validators root.
pub fn compute_fork_data_root<C: Crypto + ?Sized>(crypto: &C, fork_version: [u8; 4], genesis_validators_root: &Root) -> Root {
    let mut version = [0u8; 32];
    version[..4].copy_from_slice(&fork_version);

    ssz::hash_pair(crypto, &version, genesis_validators_root)
}

/// compute_domain: the domain type, then the first 28 bytes of the fork data root.
pub fn compute_domain<C: Crypto + ?Sized>(crypto: &C, domain_type: [u8; 4], fork_version: [u8; 4], genesis_validators_root: &Root) -> [u8; 32] {
    let mut domain = [0u8; 32];
    domain[..4].copy_from_slice(&domain_type);
    domain[4..].copy_from_slice(&compute_fork_data_root(crypto, fork_version, genesis_validators_root)[..28]);

    domain
}

/// compute_signing_root: SigningData's root, the object's beside the domain: what is signed.
pub fn compute_signing_root<C: Crypto + ?Sized>(crypto: &C, object_root: &Root, domain: &[u8; 32]) -> Root {
    ssz::hash_pair(crypto, object_root, domain)
}

/// 64 hex digits as 32 bytes, at compile time.
pub(crate) const fn hex32(text: &[u8; 64]) -> Root {
    const fn digit(byte: u8) -> u8 {
        match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => panic!("not a lowercase hex digit"),
        }
    }

    let mut out = [0u8; 32];
    let mut at = 0;

    while at < 32 {
        out[at] = digit(text[2 * at]) << 4 | digit(text[2 * at + 1]);
        at += 1;
    }

    out
}
