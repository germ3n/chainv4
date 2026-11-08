//! Which finality a chain runs, and every parameter of each kind. How a node finalizes by them is
//! chain::finality's. In a node each kind is there only with its feature; every build without the
//! node has them all, so it decodes whatever a chain or a change names.


// A node built without any kind has an empty FinalityConfig, whose encoding nothing reaches.
#![cfg_attr(
    all(feature = "node", not(any(feature = "finality-ffg", feature = "finality-authority", feature = "finality-depth"))),
    allow(unreachable_code)
)]
use {
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::registry::signers::Signer,
    crate::registry::validators::Leak,
    crate::structs::address::Address,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Which finality a chain runs, and every parameter of it. Part of genesis, so every node agrees on it; nothing about finality is fixed in code.",
    category = "Finality",
    see_also = "Genesis, Finality, FfgConfig, AuthorityConfig, DepthConfig",
    note = "In a node, each kind sits behind its own cargo feature, finality-ffg, finality-authority and finality-depth, all on by default; a build without the node, such as the wallet's, reads every kind. Tags are pinned, so encodings match across builds.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum FinalityConfig {
    #[document(summary = "Casper FFG-style: validators bond on chain and vote on checkpoints. Two thirds by bond justify, a justified direct child finalizes its parent, and cheaters lose their bond.")]
    #[cfg(any(feature = "finality-ffg", not(feature = "node")))]
    #[wincode(tag = 0)]
    Ffg(FfgConfig),
    #[document(summary = "The same votes, from signers genesis names, a threshold of them justifying, who change by their own approval. No bonds: a signer that signs conflicting votes is thrown out instead of slashed.")]
    #[cfg(any(feature = "finality-authority", not(feature = "node")))]
    #[wincode(tag = 1)]
    Authority(AuthorityConfig),
    #[document(summary = "No votes: a block is final once enough blocks are on top of it on the main chain.")]
    #[cfg(any(feature = "finality-depth", not(feature = "node")))]
    #[wincode(tag = 2)]
    Depth(DepthConfig),
}

impl Hashable for FinalityConfig {
    const TAG_LEN: usize = 1;
}

impl FinalityConfig {
    #[document(parent = "FinalityConfig", summary = "The names of the kinds this build has, as genesis files spell them.")]
    pub const KINDS: &'static [&'static str] = &[
        #[cfg(any(feature = "finality-ffg", not(feature = "node")))]
        "ffg",
        #[cfg(any(feature = "finality-authority", not(feature = "node")))]
        "authority",
        #[cfg(any(feature = "finality-depth", not(feature = "node")))]
        "depth",
    ];

    #[document(parent = "FinalityConfig", summary = "The kind's name: ffg, authority or depth.")]
    pub fn name(&self) -> &'static str {
        match *self {
            #[cfg(any(feature = "finality-ffg", not(feature = "node")))]
            FinalityConfig::Ffg(_) => "ffg",
            #[cfg(any(feature = "finality-authority", not(feature = "node")))]
            FinalityConfig::Authority(_) => "authority",
            #[cfg(any(feature = "finality-depth", not(feature = "node")))]
            FinalityConfig::Depth(_) => "depth",
        }
    }
}

#[document(
    summary = "A validator bonded from genesis.",
    category = "Finality",
    see_also = "FfgConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct GenesisValidator {
    #[document(summary = "The account that owns the bond.")]
    pub account: Address,
    #[document(summary = "The key that signs its votes.")]
    pub voter: PublicKey,
    #[document(summary = "Its bond, minted at genesis.")]
    #[wincode(with = "U256Le")]
    pub bond: u256,
    #[document(summary = "Its attester, the Ethereum address it attests links with for Ethereum, or None.")]
    pub attester: Option<[u8; 20]>,
}

impl Hashable for GenesisValidator {}

#[document(
    summary = "Every parameter of FFG finality, set in genesis.",
    category = "Finality",
    see_also = "FinalityConfig, Staking, Vote",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FfgConfig {
    #[document(summary = "Blocks per epoch. Each epoch's first block is its checkpoint.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many blocks on top of a checkpoint before validators vote for it, so they rarely vote for a block that is about to lose a fork.")]
    pub vote_depth: u64,
    #[document(summary = "The smallest bond.")]
    #[wincode(with = "U256Le")]
    pub min_bond: u256,
    #[document(summary = "Epochs before a bond, unbond or slash changes who votes. At least 1; larger keeps validator sets agreeing across deeper forks.")]
    pub activation_delay: u64,
    #[document(summary = "Epochs a bond stays locked, and slashable, after its validator stops voting.")]
    pub withdrawal_delay: u64,
    #[document(summary = "A reporter of cheating gets the bond divided by this; the rest is burned.")]
    pub reporter_divisor: u64,
    #[document(summary = "The most validators registered at once.")]
    pub max_validators: u32,
    #[document(summary = "What every voter in a justification gets when a block carries it. With none, blocks carry no proofs.")]
    #[wincode(with = "U256Le")]
    pub vote_reward: u256,
    #[document(summary = "The most justifications one block may carry.")]
    pub max_proofs_per_block: u32,
    #[document(summary = "Validators bonded from the start. With none, nothing finalizes until someone bonds.")]
    pub validators: Vec<GenesisValidator>,
    #[document(summary = "How much of a cheater's bond a slash takes, in thousandths: None, as genesis files without it mean, takes it all, 1000. The reporter gets what is taken divided by reporter_divisor and the rest is burned; what is left stays the validator's, to withdraw once its delay is over. Governance retunes it by Change::Finality.")]
    pub slash_permille: Option<u32>,
    #[document(summary = "What FFG took on after chains launched, the inactivity leak and the justification threshold, each None while unset. Encoded as nothing while both are None, so chains from before keep their ids.")]
    #[wincode(with = "UnsetLater")]
    pub later: FfgLater,
}

impl Hashable for FfgConfig {}

impl FfgConfig {
    #[document(parent = "FfgConfig", summary = "The inactivity leak, if it has one.")]
    pub fn leak(&self) -> Option<Leak> {
        self.later.leak
    }

    #[document(
        parent = "FfgConfig",
        summary = "The weight a link's votes must reach, out of a set: justify_permille thousandths of it, rounded up, or two thirds while unset; never less than two thirds, never nothing.",
        params = { set = "The epoch's validator set." },
    )]
    pub fn needed(&self, set: &crate::registry::validators::ValidatorSet) -> u256 {
        let two_thirds = set.two_thirds();

        match self.later.justify_permille {
            None => two_thirds,
            Some(permille) => {
                let thousand = u256::from(1_000u64);
                let share = (set.total().saturating_mul(u256::from(permille)).saturating_add(thousand - u256::ONE)) / thousand;

                share.max(two_thirds)
            }
        }
    }
}

#[document(
    summary = "The least and most FfgConfig's justify_permille may be: two thirds, which keeps FFG's safety, up to all of the weight.",
    category = "Finality",
    see_also = "FfgLater",
)]
pub const JUSTIFY_PERMILLE: std::ops::RangeInclusive<u32> = 667..=1_000;

#[document(
    summary = "What FFG took on after chains launched, kept together at the end of its settings so they encode as nothing while unset.",
    category = "Finality",
    see_also = "FfgConfig",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FfgLater {
    #[document(summary = "The inactivity leak, or None for none: once no checkpoint has been proven in a block for after_epochs epochs, each checkpoint takes permille thousandths of the bond of every validator that has not voted since, until those that do vote hold the threshold again.")]
    pub leak: Option<Leak>,
    #[document(summary = "How much of the bonded weight, in thousandths, votes for a link must reach to justify it: JUSTIFY_PERMILLE, 667 to 1000; None for two thirds exactly, as chains from before it. Governance retunes it by Change::Finality, from the proofs fork on, and it holds from the epoch the change is fixed for, as validator sets do.")]
    pub justify_permille: Option<u32>,
}

/// Encodes FfgConfig::later as nothing while both are None, so a genesis from before them hashes
/// as it did; a leak alone as a 1 and the leak, as it was before the threshold; the threshold alone
/// as a 2 and its four bytes; both as a 3, the leak, then the threshold. Only as a last field: an
/// exhausted reader is both None.
pub struct UnsetLater;

fn leak_bytes(leak: &Leak) -> [u8; 16] {
    let mut bytes = [0; 16];
    bytes[..8].copy_from_slice(&leak.after_epochs.to_le_bytes());
    bytes[8..].copy_from_slice(&leak.permille.to_le_bytes());

    bytes
}

fn later_bytes(later: &FfgLater) -> Vec<u8> {
    match (later.leak, later.justify_permille) {
        (None, None) => Vec::new(),
        (Some(leak), None) => [[1].as_slice(), &leak_bytes(&leak)].concat(),
        (None, Some(permille)) => [[2].as_slice(), &permille.to_le_bytes()].concat(),
        (Some(leak), Some(permille)) => [[3].as_slice(), &leak_bytes(&leak), &permille.to_le_bytes()].concat(),
    }
}

// SAFETY: size_of is exactly what write writes, later_bytes's length.
unsafe impl<C: wincode::config::ConfigCore> SchemaWrite<C> for UnsetLater {
    type Src = FfgLater;

    fn size_of(src: &FfgLater) -> wincode::WriteResult<usize> {
        Ok(later_bytes(src).len())
    }

    fn write(mut writer: impl wincode::io::Writer, src: &FfgLater) -> wincode::WriteResult<()> {
        for byte in later_bytes(src) {
            <u8 as SchemaWrite<C>>::write(&mut writer, &byte)?;
        }

        Ok(())
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is both None, which is how
// an encoding from before them reads; so this only ever ends a message.
unsafe impl<'de, C: wincode::config::ConfigCore> SchemaRead<'de, C> for UnsetLater {
    type Dst = FfgLater;

    fn read(mut reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<FfgLater>) -> wincode::ReadResult<()> {
        let leak = |bytes: [u8; 16]| Leak { after_epochs: u64::from_le_bytes(bytes[..8].try_into().expect("eight bytes")), permille: u64::from_le_bytes(bytes[8..].try_into().expect("eight bytes")) };
        let value = match reader.take_byte() {
            Ok(1) => FfgLater { leak: Some(leak(reader.take_array::<16>()?)), justify_permille: None },
            Ok(2) => FfgLater { leak: None, justify_permille: Some(u32::from_le_bytes(reader.take_array::<4>()?)) },
            Ok(3) => FfgLater { leak: Some(leak(reader.take_array::<16>()?)), justify_permille: Some(u32::from_le_bytes(reader.take_array::<4>()?)) },
            _ => FfgLater::default(),
        };
        dst.write(value);

        Ok(())
    }
}

#[document(
    summary = "Every parameter of authority finality, set in genesis. The signers are where the chain starts; they change by their own approval.",
    category = "Finality",
    see_also = "FinalityConfig, SignerSet, Tx::ApproveSigners",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct AuthorityConfig {
    #[document(summary = "Blocks per epoch. Each epoch's first block is its checkpoint.")]
    pub checkpoint_interval: u64,
    #[document(summary = "How many blocks on top of a checkpoint before signers vote for it.")]
    pub vote_depth: u64,
    #[document(summary = "Epochs before a decided change of signers takes effect. At least 1; larger keeps the signers agreeing across deeper forks.")]
    pub activation_delay: u64,
    #[document(summary = "The most signers a set may have.")]
    pub max_signers: u32,
    #[document(summary = "The signers from genesis, each counting once: its voting key and the account that speaks for it.")]
    pub signers: Vec<Signer>,
    #[document(summary = "How many signers voting for one link justify it: more than half of them, and at most all.")]
    pub threshold: u32,
    #[document(summary = "How many signers must approve a change to the set: more than half of them, and at most all.")]
    pub change_threshold: u32,
    #[document(summary = "How many epochs an approval of a change counts for, the epoch it was made in included. At least 1. A change is only decided by approvals that still count, so it always reflects what the signers want now.")]
    pub approval_epochs: u64,
    #[document(summary = "What every signer in a justification gets when a block carries it, paid to its key's address. With none, blocks carry no proofs.")]
    #[wincode(with = "U256Le")]
    pub vote_reward: u256,
    #[document(summary = "The most justifications one block may carry.")]
    pub max_proofs_per_block: u32,
}

impl Hashable for AuthorityConfig {}

#[document(
    summary = "The one parameter of depth finality, set in genesis.",
    category = "Finality",
    see_also = "FinalityConfig",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct DepthConfig {
    #[document(summary = "How many blocks must be on top of a block for it to be final. 0 makes every block final as soon as it is the tip, so the chain never reorganizes.")]
    pub depth: u64,
}

impl Hashable for DepthConfig {}
