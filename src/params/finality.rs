//! Which finality a chain runs, and every parameter of each kind. How a node finalizes by them is
//! chain::finality's. In a node each kind is there only with its feature; every build without the
//! node has them all, so it decodes whatever a chain or a change names.

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
    #[document(summary = "The inactivity leak, or None for none: once no checkpoint has been proven in a block for after_epochs epochs, each checkpoint takes permille thousandths of the bond of every validator that has not voted since, until those that do vote hold two thirds again. Added after chains launched: None adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "UnsetLeak")]
    pub leak: Option<Leak>,
}

impl Hashable for FfgConfig {}

/// Encodes FfgConfig::leak as nothing while None, and as a 1 and the leak once set, so a genesis
/// from before the field hashes as it did. Only as a last field; see Unset.
pub struct UnsetLeak;

// SAFETY: size_of is exactly what write writes: nothing for None, seventeen bytes for Some.
unsafe impl<C: wincode::config::ConfigCore> SchemaWrite<C> for UnsetLeak {
    type Src = Option<Leak>;

    fn size_of(src: &Option<Leak>) -> wincode::WriteResult<usize> {
        Ok(if src.is_some() { 17 } else { 0 })
    }

    fn write(writer: impl wincode::io::Writer, src: &Option<Leak>) -> wincode::WriteResult<()> {
        let Some(leak) = src else {
            return Ok(());
        };
        let mut bytes = [1; 17];
        bytes[1..9].copy_from_slice(&leak.after_epochs.to_le_bytes());
        bytes[9..].copy_from_slice(&leak.permille.to_le_bytes());

        <[u8; 17] as SchemaWrite<C>>::write(writer, &bytes)
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is None, which is how an
// encoding from before the field reads; so this only ever ends a message.
unsafe impl<'de, C: wincode::config::ConfigCore> SchemaRead<'de, C> for UnsetLeak {
    type Dst = Option<Leak>;

    fn read(mut reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<Option<Leak>>) -> wincode::ReadResult<()> {
        let value = match reader.take_byte() {
            Ok(1) => {
                let rest = reader.take_array::<16>()?;

                Some(Leak {
                    after_epochs: u64::from_le_bytes(rest[..8].try_into().expect("eight bytes")),
                    permille: u64::from_le_bytes(rest[8..].try_into().expect("eight bytes")),
                })
            }
            _ => None,
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
