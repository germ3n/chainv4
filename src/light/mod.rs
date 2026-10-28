//! A light client: it follows a chain from its headers alone, checking every one by the chain's
//! consensus, and its finality from justifications, checking each against voters it proved from
//! the state of a checkpoint it trusts already. Then it checks any value, receipt or transaction a
//! node sends, with its proof, against a header it holds. It trusts nothing but the genesis id it starts
//! from.
//!
//! It is sans-IO: next says what to fetch, and the deliver and take methods check what came back,
//! so the CLI drives it over HTTP with one library and the explorer with another.

use {
    crate::consensus::{Consensus, ConsensusConfig, ConsensusError},
    crate::governance::GovernanceKind,
    crate::structs::unset::Unset,
    crate::registry::changes::Changes,
    crate::crypto::hashable::Hashable,
    crate::crypto::keypair::PublicKey,
    crate::crypto::merkle,
    crate::db::{Column, DbError, State, StateProof, StateTree, namespace_key},
    crate::structs::address::Address,
    crate::registry::signers::{SignerSet, Signers},
    crate::registry::validators::{Member, ValidatorSet, Validators},
    crate::structs::block::{BlockHeader, SealedHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::receipt::CommittedReceipt,
    crate::structs::tx::SignedTx,
    crate::structs::u256::u256,
    crate::structs::vote::{Checkpoint, Justification},
    r#macro::document,
    std::collections::{BTreeMap, BTreeSet, HashMap},
    std::fmt,
    std::ops::RangeInclusive,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(summary = "The most headers a light client asks for at once.", category = "Light client")]
pub const MAX_LIGHT_HEADERS: u32 = 1_000;

#[document(summary = "The most epochs of finality a node sends a light client at once.", category = "Light client", see_also = "FinalityStep")]
pub const MAX_FINALITY_STEPS: u64 = 64;

#[document(
    summary = "Where a token keeps a holder's balance, by the token standard: in Column::Namespaces, the token's balances namespace, by the holder's address text. What a light client proves a token balance by.",
    params = {
        token = "The token contract.",
        holder = "Whose balance.",
    },
    category = "Light client",
    see_also = "token_balance",
)]
pub fn token_balance_key(token: &Address, holder: &Address) -> Vec<u8> {
    namespace_key(token, TOKEN_BALANCES, holder.to_string().as_bytes())
}

#[document(
    summary = "A token balance as a token keeps it: decimal text, as Lua tokens write it, or 32 bytes little-endian, as the SDK's U256 writes it.",
    returns = "The balance, or None for a value that is neither.",
    category = "Light client",
    see_also = "token_balance_key",
)]
pub fn token_balance(value: &[u8]) -> Option<u256> {
    let decimal = !value.is_empty() && value.iter().all(u8::is_ascii_digit);

    match (decimal, <[u8; 32]>::try_from(value)) {
        (true, _) => u256::from_str_radix(std::str::from_utf8(value).ok()?, 10).ok(),
        (false, Ok(bytes)) => Some(u256::from_le_bytes(bytes)),
        (false, Err(_)) => None,
    }
}

/// The namespace the token standard keeps balances in.
const TOKEN_BALANCES: &[u8] = b"balances";

#[document(
    summary = "Where an NFT collection keeps a token's owner, by the NFT standard: in Column::Namespaces, the collection's owners namespace, by the token id in decimal. What a light client proves who owns an NFT by.",
    params = {
        collection = "The collection contract.",
        id = "The token.",
    },
    category = "Light client",
    see_also = "nft_owner",
)]
pub fn nft_owner_key(collection: &Address, id: u256) -> Vec<u8> {
    namespace_key(collection, NFT_OWNERS, id.to_string().as_bytes())
}

#[document(
    summary = "An NFT's owner as a collection keeps it: the owner's address text, as Lua and the SDK both write it.",
    returns = "The owner, or None for a value that is not an address.",
    category = "Light client",
    see_also = "nft_owner_key",
)]
pub fn nft_owner(value: &[u8]) -> Option<Address> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

#[document(
    summary = "Where a multi-token collection keeps a holder's amount of an id, by the multi-token standard: in Column::Namespaces, the collection's balances namespace, by the id in decimal, a colon and the holder's address text. The amount reads as a token balance does, with token_balance.",
    params = {
        collection = "The collection contract.",
        id = "The id.",
        holder = "Whose amount.",
    },
    category = "Light client",
    see_also = "token_balance",
)]
pub fn multi_balance_key(collection: &Address, id: u256, holder: &Address) -> Vec<u8> {
    namespace_key(collection, TOKEN_BALANCES, format!("{id}:{holder}").as_bytes())
}

/// The namespace the NFT standard keeps owners in.
const NFT_OWNERS: &[u8] = b"owners";

#[document(
    summary = "What a light client needs to know about a chain to follow it: its hash, its consensus, how its finality works, and whether governance may change the consensus. Genesis's proof_root is the hash of these, so a client that trusts a genesis id can check what a node tells it they are.",
    category = "Light client",
    see_also = "LightClient, Genesis::light_params",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct LightParams {
    #[document(summary = "The chain hash.")]
    pub hash: HashType,
    #[document(summary = "The chain's consensus from genesis, which every header is checked by, until governance changes it.")]
    pub consensus: ConsensusConfig,
    #[document(summary = "How finality works on the chain, or None for a chain without.")]
    pub finality: Option<LightFinality>,
    #[document(summary = "The proof-of-work window, as Genesis::pow_window. Hashed as nothing while None, so chains from before it keep the proof_root their genesis commits to; on the wire and in saved state it is a plain Option.")]
    pub pow_window: Option<u64>,
    #[document(summary = "The kind of governance the chain runs, or None for one where nothing changes. With one, each epoch's consensus comes from a light update, as its voters do, and the client waits for it before checking the epoch's headers. Hashed as nothing while None, as the window is.")]
    pub governance: Option<GovernanceKind>,
}

/// The parameters as genesis commits to them: the window and the governance kind encode as
/// nothing while unset, so a chain from before either hashes as it always did. Only hashed, never
/// decoded, so the two trailing fields need no telling apart.
#[derive(SchemaWrite)]
struct CommittedLightParams {
    hash: HashType,
    consensus: ConsensusConfig,
    finality: Option<LightFinality>,
    #[wincode(with = "Unset")]
    pow_window: Option<u64>,
    #[wincode(with = "UnsetKind")]
    governance: Option<GovernanceKind>,
}

impl Hashable for CommittedLightParams {}

/// Encodes the governance kind as nothing while None, and as a 2 and its tag once set, for the
/// committed parameters alone.
struct UnsetKind;

// SAFETY: size_of is exactly what write writes: nothing for None, two bytes for Some.
unsafe impl<C: wincode::config::ConfigCore> SchemaWrite<C> for UnsetKind {
    type Src = Option<GovernanceKind>;

    fn size_of(src: &Option<GovernanceKind>) -> wincode::WriteResult<usize> {
        Ok(if src.is_some() { 2 } else { 0 })
    }

    fn write(writer: impl wincode::io::Writer, src: &Option<GovernanceKind>) -> wincode::WriteResult<()> {
        let Some(kind) = src else {
            return Ok(());
        };
        let tag = match kind {
            GovernanceKind::Electorate => 0,
            GovernanceKind::Council => 1,
        };

        <[u8; 2] as SchemaWrite<C>>::write(writer, &[2, tag])
    }
}

impl LightParams {
    fn committed(&self) -> CommittedLightParams {
        CommittedLightParams {
            hash: self.hash,
            consensus: self.consensus.clone(),
            finality: self.finality,
            pow_window: self.pow_window,
            governance: self.governance,
        }
    }
}

// Hashed as genesis commits to them, with the window and the governance kind encoding as nothing
// while unset; the derived encoding, which decodes, is what goes on the wire.
impl Hashable for LightParams {
    fn hash(&self, algorithm: HashType) -> Hash {
        self.committed().hash(algorithm)
    }

    fn hash_with_tag(&self, algorithm: HashType) -> Hash {
        self.committed().hash_with_tag(algorithm)
    }

    fn hashable_bytes(&self) -> Vec<u8> {
        self.committed().hashable_bytes()
    }

    fn hashable_bytes_with_tag(&self) -> Vec<u8> {
        self.committed().hashable_bytes_with_tag()
    }
}

impl LightParams {
    #[document(parent = "LightParams", summary = "The consensus rules headers are checked by from genesis, the window and the finality's epochs included.")]
    pub fn rules(&self) -> Box<dyn Consensus> {
        self.rules_of(&self.consensus)
    }

    #[document(parent = "LightParams", summary = "The rules of a consensus governance fixed for an epoch, built as genesis's are: with the chain's hash, its window and its finality's epochs.")]
    pub fn rules_of(&self, consensus: &ConsensusConfig) -> Box<dyn Consensus> {
        consensus.rules(self.hash, self.pow_window, self.finality.and_then(|finality| finality.checkpoint_interval()))
    }

    #[document(parent = "LightParams", summary = "How many ancestors genesis's consensus looks at when checking a header: what a client must hold below any header it checks.")]
    pub fn lookback(&self) -> usize {
        self.rules().lookback()
    }
}

#[document(
    summary = "How finality works on a chain, as much as a light client needs: when checkpoints come, how far ahead their voters are fixed, and how deep a block is final under depth finality.",
    category = "Light client",
    see_also = "LightParams",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum LightFinality {
    #[document(summary = "Bonded validators vote, two thirds of the weight justifying.")]
    #[wincode(tag = 0)]
    Ffg { checkpoint_interval: u64, fixed_ahead: u64 },
    #[document(summary = "A set of signers votes, a threshold of them justifying.")]
    #[wincode(tag = 1)]
    Authority { checkpoint_interval: u64, fixed_ahead: u64 },
    #[document(summary = "A block is final once this many are on top of it.")]
    #[wincode(tag = 2)]
    Depth { depth: u64 },
}

impl LightFinality {
    #[document(parent = "LightFinality", summary = "Blocks between checkpoints, for the kinds that vote.")]
    pub fn checkpoint_interval(&self) -> Option<u64> {
        match *self {
            LightFinality::Ffg { checkpoint_interval, .. } | LightFinality::Authority { checkpoint_interval, .. } => Some(checkpoint_interval),
            LightFinality::Depth { .. } => None,
        }
    }

    #[document(
        parent = "LightFinality",
        summary = "The epochs whose voters a block's light update carries: epoch 0 through fixed_ahead for genesis, so a client knows who produces under a rotating authority from the first block; for a checkpoint, the epochs after its own through the one it fixes, fixed_ahead ahead, since a slash or an ejection takes a voter out of sets fixed earlier, and the next checkpoint re-states them; none for any other block or kind.",
        params = { height = "The block's height." },
    )]
    pub fn fixed_at(&self, height: u64) -> Option<RangeInclusive<u64>> {
        let (LightFinality::Ffg { checkpoint_interval, fixed_ahead } | LightFinality::Authority { checkpoint_interval, fixed_ahead }) = *self else {
            return None;
        };

        match height {
            0 => Some(0..=fixed_ahead),
            _ if height.is_multiple_of(checkpoint_interval) => {
                let epoch = height / checkpoint_interval;

                Some(epoch + 1..=epoch + fixed_ahead)
            }
            _ => None,
        }
    }

    #[document(parent = "LightFinality", summary = "How many epochs ahead a checkpoint fixes the voters, for the kinds that vote.")]
    pub fn fixed_ahead(&self) -> Option<u64> {
        match *self {
            LightFinality::Ffg { fixed_ahead, .. } | LightFinality::Authority { fixed_ahead, .. } => Some(fixed_ahead),
            LightFinality::Depth { .. } => None,
        }
    }

    #[document(parent = "LightFinality", summary = "The kind's name, as genesis files spell it: ffg, authority or depth.")]
    pub fn name(&self) -> &'static str {
        match self {
            LightFinality::Ffg { .. } => "ffg",
            LightFinality::Authority { .. } => "authority",
            LightFinality::Depth { .. } => "depth",
        }
    }

    #[document(
        parent = "LightFinality",
        summary = "The other kind that votes, with the same epochs: what a chain whose governance switched finality's kind runs in the epochs a light update names as switched. None for depth finality, which no chain switches to or from.",
    )]
    pub fn other(&self) -> Option<LightFinality> {
        match *self {
            LightFinality::Ffg { checkpoint_interval, fixed_ahead } => Some(LightFinality::Authority { checkpoint_interval, fixed_ahead }),
            LightFinality::Authority { checkpoint_interval, fixed_ahead } => Some(LightFinality::Ffg { checkpoint_interval, fixed_ahead }),
            LightFinality::Depth { .. } => None,
        }
    }

    #[document(parent = "LightFinality", summary = "Where an epoch's voters are kept, in Column::Epochs.")]
    pub fn voters_key(&self, epoch: u64) -> Option<Vec<u8>> {
        match self {
            LightFinality::Ffg { .. } => Some(Validators::set_key(epoch)),
            LightFinality::Authority { .. } => Some(Signers::set_key(epoch)),
            LightFinality::Depth { .. } => None,
        }
    }

    #[document(parent = "LightFinality", summary = "An epoch's voters, and what they must add up to, from the value kept at voters_key.")]
    pub fn voters(&self, value: &[u8]) -> Option<Voters> {
        match self {
            LightFinality::Ffg { .. } => wincode::deserialize::<ValidatorSet>(value).ok().map(|set| Voters {
                needed: set.two_thirds(),
                set,
            }),
            LightFinality::Authority { .. } => wincode::deserialize::<SignerSet>(value).ok().map(|set| Voters {
                needed: u256::from(set.threshold),
                set: set.validators(),
            }),
            LightFinality::Depth { .. } => None,
        }
    }
}

#[document(summary = "An epoch's voters, with their weights, and the weight a link's votes must add up to.", category = "Light client")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Voters {
    #[document(summary = "Who votes, and with what weight.")]
    pub set: ValidatorSet,
    #[document(summary = "What a justification's votes must add up to.")]
    pub needed: u256,
}

#[document(
    summary = "What a node records for light clients when it runs a block that fixes voters, genesis or a checkpoint: proofs of those voters against the block's state root. A client that trusts the block's state, because it is at or below a justified checkpoint, learns the voters from them.",
    category = "Light client",
    see_also = "LightClient::take_update",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct LightUpdate {
    #[document(summary = "The block whose state root the proofs are against.")]
    pub block: Hash,
    #[document(summary = "The voters it fixed, each with its proof.")]
    pub voters: Vec<VoterProof>,
    #[document(summary = "Under governance, the consensus it fixed for the same epochs, each with its proof; empty on a chain without. Added after chains launched: an update stored before it reads as one with none, through stored_light_update.")]
    pub eras: Vec<EraProof>,
    #[document(summary = "The epochs among them whose voters are the other voting kind's, FFG's validators on a chain that started under authority or authority's signers on one that started under FFG, since governance switched finality's kind; their proofs are at that kind's keys. A state only ever fixes one kind's voters for an epoch, so a node can neither hide a switch nor claim one that did not happen. Added after chains launched: an update stored before it reads as one with none, through stored_light_update.")]
    pub switched: Vec<u64>,
}

impl Hashable for LightUpdate {}

#[document(
    summary = "A light update as nodes stored them before eras: what stored_light_update falls back to reading.",
    category = "Light client",
    see_also = "LightUpdate",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct LegacyLightUpdate {
    #[document(summary = "The block whose state root the proofs are against.")]
    pub block: Hash,
    #[document(summary = "The voters it fixed, each with its proof.")]
    pub voters: Vec<VoterProof>,
}

#[document(
    summary = "A light update as nodes stored them before switches of finality's kind: with eras, without switched epochs. What stored_light_update reads next.",
    category = "Light client",
    see_also = "LightUpdate",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ErasLightUpdate {
    #[document(summary = "The block whose state root the proofs are against.")]
    pub block: Hash,
    #[document(summary = "The voters it fixed, each with its proof.")]
    pub voters: Vec<VoterProof>,
    #[document(summary = "The consensus it fixed, each with its proof.")]
    pub eras: Vec<EraProof>,
}

impl From<ErasLightUpdate> for LightUpdate {
    fn from(update: ErasLightUpdate) -> LightUpdate {
        LightUpdate {
            block: update.block,
            voters: update.voters,
            eras: update.eras,
            switched: Vec::new(),
        }
    }
}

impl From<LegacyLightUpdate> for LightUpdate {
    fn from(legacy: LegacyLightUpdate) -> LightUpdate {
        LightUpdate {
            block: legacy.block,
            voters: legacy.voters,
            eras: Vec::new(),
            switched: Vec::new(),
        }
    }
}

#[document(summary = "One epoch's consensus, as kept in state under governance, with the proof it is there.", category = "Light client", see_also = "LightUpdate, Changes::consensus_key")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct EraProof {
    #[document(summary = "The epoch it is fixed for.")]
    pub epoch: u64,
    #[document(summary = "The consensus, encoded as kept at Changes::consensus_key.")]
    pub value: Vec<u8>,
    #[document(summary = "The proof of it, against the block's state root.")]
    pub proof: StateProof,
}

/// Encodes a saved client's eras as nothing while empty, as UnsetEras does its proofs, so a state
/// saved before the field restores. Only as a last field.
pub struct UnsetEraList;

// SAFETY: size_of is exactly what write writes: nothing when empty, the list's encoding otherwise.
unsafe impl<C: wincode::config::Config> wincode::SchemaWrite<C> for UnsetEraList {
    type Src = Vec<(u64, ConsensusConfig)>;

    fn size_of(src: &Vec<(u64, ConsensusConfig)>) -> wincode::WriteResult<usize> {
        match src.is_empty() {
            true => Ok(0),
            false => <Vec<(u64, ConsensusConfig)> as wincode::SchemaWrite<C>>::size_of(src),
        }
    }

    fn write(writer: impl wincode::io::Writer, src: &Vec<(u64, ConsensusConfig)>) -> wincode::WriteResult<()> {
        match src.is_empty() {
            true => Ok(()),
            false => <Vec<(u64, ConsensusConfig)> as wincode::SchemaWrite<C>>::write(writer, src),
        }
    }
}

// SAFETY: read initializes dst before it returns Ok. An exhausted reader is the empty list.
unsafe impl<'de, C: wincode::config::Config> wincode::SchemaRead<'de, C> for UnsetEraList {
    type Dst = Vec<(u64, ConsensusConfig)>;

    fn read(reader: impl wincode::io::Reader<'de>, dst: &mut std::mem::MaybeUninit<Vec<(u64, ConsensusConfig)>>) -> wincode::ReadResult<()> {
        dst.write(<Vec<(u64, ConsensusConfig)> as wincode::SchemaRead<'de, C>>::get(reader).unwrap_or_default());

        Ok(())
    }
}

#[document(
    summary = "One epoch of finality, as a node sends it to light clients: the justification of the epoch's checkpoint, and the light update the checkpoint block keeps. Sent many epochs at once, in order, since each update a justification makes trusted names the voters of justifications further on.",
    category = "Light client",
    see_also = "LightClient::deliver_finality, MAX_FINALITY_STEPS",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FinalityStep {
    #[document(summary = "The justification of the epoch's checkpoint; None for epoch 0, genesis, which needs none.")]
    pub justification: Option<Justification>,
    #[document(summary = "The light update the checkpoint block keeps; None when the node has none, as one that synced from a snapshot lacks those before it.")]
    pub update: Option<LightUpdate>,
}

impl Hashable for FinalityStep {}

#[document(summary = "One epoch's voters, as kept in state, with the proof they are there.", category = "Light client", see_also = "LightUpdate")]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct VoterProof {
    #[document(summary = "The epoch they vote in.")]
    pub epoch: u64,
    #[document(summary = "Their record, as kept at LightFinality::voters_key.")]
    pub value: Vec<u8>,
    #[document(summary = "The proof of it, against the block's state root.")]
    pub proof: StateProof,
}

impl LightUpdate {
    #[document(
        parent = "LightUpdate",
        summary = "The update for a block that fixes voters, made from the state it leaves; None for a block that fixes none.",
        params = {
            state = "The state once the block ran, its state tree followed.",
            hash = "The chain's hash.",
            finality = "How finality works on the chain.",
            governed = "Whether governance may change the consensus, so the update carries each epoch's consensus too.",
            block = "The block's id.",
            height = "Its height.",
        },
    )]
    pub fn of(state: &State, hash: HashType, finality: &LightFinality, governed: bool, block: Hash, height: u64) -> Result<Option<LightUpdate>, DbError> {
        let Some(epochs) = finality.fixed_at(height) else {
            return Ok(None);
        };
        let tree = StateTree::new(state, hash);
        let mut voters = Vec::new();
        let mut eras = Vec::new();
        let mut switched = Vec::new();

        for epoch in epochs.clone() {
            let key = Changes::consensus_key(epoch);

            if governed && let Some(value) = state.get(Column::Epochs, &key)? {
                eras.push(EraProof {
                    epoch,
                    value,
                    proof: tree.prove(Column::Epochs, &key)?,
                });
            }
        }

        for epoch in epochs {
            let kind = match governed && Changes::new(state).finality_at(epoch)?.is_some_and(|config| config.name() != finality.name()) {
                true => {
                    switched.push(epoch);

                    finality.other().unwrap_or(*finality)
                }
                false => *finality,
            };
            let key = kind.voters_key(epoch).expect("kinds that fix voters keep them");

            if let Some(value) = state.get(Column::Epochs, &key)? {
                voters.push(VoterProof {
                    epoch,
                    value,
                    proof: tree.prove(Column::Epochs, &key)?,
                });
            }
        }

        Ok(Some(LightUpdate { block, voters, eras, switched }))
    }

    #[document(
        parent = "LightUpdate",
        summary = "Checks an update a node sent for a block at a height, whose state root is trusted: it names that block, every voter and era proof holds against the root for an epoch the block fixes, and it holds the voters of every one of those epochs, each once, so none was left out.",
        params = {
            block = "The block it should be of.",
            finality = "The chain's finality, which says which epochs a block fixes and where voters are kept.",
            height = "The block's height.",
            root = "Its state root, from a header the caller trusts.",
        },
        returns = "The voters of each epoch, and the consensus of each epoch it names under governance. BadProof for one that does not hold or leaves voters out, Undecodable for a value that does not decode.",
    )]
    pub fn check(&self, block: &Hash, finality: &LightFinality, height: u64, root: &Hash) -> Result<Learned, LightError> {
        let fixed = finality.fixed_at(height).ok_or(LightError::BadProof)?;

        if self.block != *block {
            return Err(LightError::BadProof);
        }

        let epochs: BTreeSet<u64> = self.voters.iter().map(|voter| voter.epoch).collect();
        let switched: BTreeSet<u64> = self.switched.iter().copied().collect();

        if epochs.len() != self.voters.len() || !fixed.clone().all(|epoch| epochs.contains(&epoch)) {
            return Err(LightError::BadProof);
        }

        if switched.len() != self.switched.len() || !switched.iter().all(|epoch| fixed.contains(epoch)) {
            return Err(LightError::BadProof);
        }

        let mut learned = Vec::new();

        for voter in &self.voters {
            // A switched epoch's voters are the other kind's, at its key: a state holds only one
            // kind's for an epoch, so the proof fails for whichever it does not.
            let kind = match switched.contains(&voter.epoch) {
                true => finality.other().ok_or(LightError::BadProof)?,
                false => *finality,
            };
            let key = kind.voters_key(voter.epoch).ok_or(LightError::BadProof)?;

            if !fixed.contains(&voter.epoch) || !voter.proof.verify(root, Column::Epochs, &key, Some(&voter.value)) {
                return Err(LightError::BadProof);
            }

            learned.push((voter.epoch, kind.voters(&voter.value).ok_or(LightError::Undecodable)?));
        }

        let mut eras = Vec::new();

        for era in &self.eras {
            let key = Changes::consensus_key(era.epoch);

            if !fixed.contains(&era.epoch) || !era.proof.verify(root, Column::Epochs, &key, Some(&era.value)) {
                return Err(LightError::BadProof);
            }

            eras.push((era.epoch, wincode::deserialize::<ConsensusConfig>(&era.value).map_err(|_| LightError::Undecodable)?));
        }

        Ok((learned, eras))
    }
}

/// What a checked update teaches: each epoch's voters, and each epoch's consensus it names.
type Learned = (Vec<(u64, Voters)>, Vec<(u64, ConsensusConfig)>);

#[document(
    summary = "What a light client keeps between runs, from LightClient::save, to restore it from. Only checked data: headers from the finalized block on, and what they prove.",
    category = "Light client",
    see_also = "LightClient::save, LightClient::restore",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct LightState {
    #[document(summary = "The chain's light parameters.")]
    pub params: LightParams,
    #[document(summary = "Its genesis id.")]
    pub genesis: Hash,
    #[document(summary = "The headers kept, from the finalized block, or genesis, on.")]
    pub headers: Vec<SealedHeader>,
    #[document(summary = "Each one's total work, 32 bytes little-endian.")]
    pub work: Vec<[u8; 32]>,
    #[document(summary = "The voters it learned, by epoch, with the weight their votes must add up to, little-endian.")]
    pub voters: Vec<(u64, ValidatorSet, [u8; 32])>,
    #[document(summary = "The justified checkpoints it kept, by epoch.")]
    pub justified: Vec<(u64, Hash)>,
    #[document(summary = "The finalized checkpoint.")]
    pub finalized: Checkpoint,
    #[document(summary = "The heights of the blocks whose updates it took.")]
    pub updated: Vec<u64>,
    #[document(summary = "The headers just below the first kept, newest first, as many as consensus looks at when checking a header, so the first header the restored client checks is checked as any other.")]
    pub ancestors: Vec<SealedHeader>,
    #[document(summary = "Under governance, the consensus it learned for each epoch, from the finalized one on. Added after chains launched: empty, it adds nothing to the encoding.")]
    #[wincode(with = "UnsetEraList")]
    pub eras: Vec<(u64, ConsensusConfig)>,
}

impl Hashable for LightState {}

#[document(
    summary = "What a light client starts from at a checkpoint it trusts, rather than at genesis, as a node serves it: the checkpoint's header and those below it, back to the earliest checkpoint whose light update names the voters of an epoch after it, and those updates, oldest first.",
    category = "Light client",
    see_also = "LightClient::from_checkpoint",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct CheckpointStart {
    #[document(summary = "Consecutive main-chain headers with their seals, ending at the checkpoint.")]
    pub headers: Vec<SealedHeader>,
    #[document(summary = "The light update of each checkpoint among the headers, oldest first; None where the node has none. Empty under kinds of finality that do not vote.")]
    pub updates: Vec<Option<LightUpdate>>,
}

impl Hashable for CheckpointStart {}

#[document(
    summary = "Where a light client starting at the block of this height begins: its epoch, and the height of the first header it needs. Under voting finality the block must open an epoch, and the client needs the headers back to the checkpoint fixed_ahead epochs before, whose updates name the voters of the epochs after it, or genesis for the first few; under any other, the block alone, its height its epoch.",
    category = "Light client",
    returns = "The epoch and the first height, or None for a block that opens no epoch under voting finality.",
    see_also = "CheckpointStart",
)]
pub fn checkpoint_start(finality: Option<LightFinality>, height: u64) -> Option<(u64, u64)> {
    match finality.and_then(|finality| finality.checkpoint_interval().zip(finality.fixed_ahead())) {
        Some((interval, ahead)) => height.is_multiple_of(interval).then(|| (height / interval, (height / interval + 1).saturating_sub(ahead) * interval)),
        None => Some((height, height)),
    }
}

#[document(
    summary = "The first header a checkpoint start carries: the one checkpoint_start names, or lower when consensus looks at more ancestors than that leaves, so the client checks the header after the checkpoint as a node does.",
    category = "Light client",
    params = {
        from = "The first height checkpoint_start gave.",
        height = "The checkpoint's height.",
        lookback = "How many ancestors consensus looks at: LightParams::lookback.",
    },
    see_also = "checkpoint_start, CheckpointStart",
)]
pub fn checkpoint_first_header(from: u64, height: u64, lookback: usize) -> u64 {
    from.min((height + 1).saturating_sub(lookback as u64))
}

#[document(summary = "The most headers a saved light client keeps: without finality, or with finality stalled, those far below the tip go.", category = "Light client")]
pub const MAX_KEPT_HEADERS: u64 = 4_096;

#[document(summary = "Something for the light client's driver to fetch next.", category = "Light client", see_also = "LightClient::next")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LightRequest {
    #[document(summary = "The node's main-chain headers from a height on, at most count.")]
    Headers { from: u64, count: u32 },
    #[document(summary = "The node's finality from an epoch on, at most MAX_FINALITY_STEPS epochs: each checkpoint's justification and light update, for LightClient::deliver_finality.")]
    Finality { from: u64 },
}

/// What came of a justification delivered.
enum Took {
    Taken,
    /// At or below the justified checkpoint already.
    Old,
    /// Past the headers followed, or for voters not known yet.
    Waits,
}

#[document(summary = "How far the client trusts a value or receipt it checked.", category = "Light client")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assurance {
    #[document(summary = "The block it was proven in.")]
    pub block: Hash,
    #[document(summary = "Its height.")]
    pub height: u64,
    #[document(summary = "Whether the block is final: at or below the finalized checkpoint, or deep enough under depth finality.")]
    pub final_: bool,
}

#[document(summary = "Why the light client refused something a node sent.", category = "Light client")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LightError {
    #[document(summary = "The genesis it was given is not the one it trusts.")]
    WrongGenesis,
    #[document(summary = "The parameters do not hash to the genesis's proof_root.")]
    WrongParams,
    #[document(summary = "A header does not follow its parent, or consensus refused it.")]
    BadHeader { height: u64, why: String },
    #[document(summary = "The headers do not start where asked.")]
    Misplaced,
    #[document(summary = "Following a branch would drop a justified checkpoint.")]
    ConflictsWithFinality,
    #[document(summary = "The block is not on the client's chain.")]
    NotOnChain,
    #[document(summary = "The block's state is not trusted yet: it is above the latest justified checkpoint.")]
    Untrusted,
    #[document(summary = "A proof does not check.")]
    BadProof,
    #[document(summary = "The epoch's voters are not known yet.")]
    UnknownVoters(u64),
    #[document(summary = "A justification does not check, and why.")]
    BadJustification(&'static str),
    #[document(summary = "Something does not decode.")]
    Undecodable,
    #[document(summary = "What a node sent to start from a checkpoint does not end at it, or does not reach far enough back.")]
    WrongCheckpoint,
}

impl fmt::Display for LightError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LightError::WrongGenesis => write!(f, "the node is on another chain than the genesis trusted"),
            LightError::WrongParams => write!(f, "the chain's parameters do not match its genesis"),
            LightError::BadHeader { height, why } => write!(f, "header {height}: {why}"),
            LightError::Misplaced => write!(f, "headers that do not start where they were asked from"),
            LightError::ConflictsWithFinality => write!(f, "a branch that leaves a justified checkpoint"),
            LightError::NotOnChain => write!(f, "a block not on the chain followed"),
            LightError::Untrusted => write!(f, "a block above the latest justified checkpoint, whose state is not trusted yet"),
            LightError::BadProof => write!(f, "a proof that does not check"),
            LightError::UnknownVoters(epoch) => write!(f, "epoch {epoch}'s voters are not known yet"),
            LightError::BadJustification(why) => write!(f, "a justification that does not check: {why}"),
            LightError::Undecodable => write!(f, "something that does not decode"),
            LightError::WrongCheckpoint => write!(f, "headers that do not lead to the checkpoint trusted, or not from far enough back"),
        }
    }
}

impl std::error::Error for LightError {}

#[document(summary = "What delivering headers did.", category = "Light client", see_also = "LightClient::deliver_headers")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extended {
    #[document(summary = "They extended the chain by this many.")]
    Added(usize),
    #[document(summary = "They were a heavier branch, followed from this height on.")]
    Switched(u64),
    #[document(summary = "A branch no heavier than the chain followed: kept the chain.")]
    Lighter,
    #[document(summary = "They do not link to the chain where asked: the node's chain split off lower, so ask from further back.")]
    Unlinked,
    #[document(summary = "None: the client has every header the node has.")]
    UpToDate,
    #[document(summary = "None yet: under a consensus whose producers finality names, or governance that may change the consensus, the client cannot check them until it has proved their epoch's voters, or learned their epoch's consensus; it asks for finality first, and for them again after.")]
    Waiting,
}

#[document(
    summary = "A light client of one chain: the headers it checked, the voters it proved, and the checkpoints it saw justified and finalized.",
    category = "Light client",
    see_also = "LightParams, LightRequest, Assurance",
    example = "let mut client = LightClient::new(params, genesis, trusted_id)?;\nwhile let Some(request) = client.next() {\n    // fetch it from a node, and deliver what comes back\n}\nlet assurance = client.check_state(&block, Column::Accounts, &key, Some(&value), &proof)?;",
)]
pub struct LightClient {
    params: LightParams,
    /// Genesis's consensus rules: every header's, on a chain without governance.
    consensus: Box<dyn Consensus>,
    /// Under governance, the consensus learned for each epoch, from light updates, and the rules
    /// built for each consensus seen.
    eras: BTreeMap<u64, ConsensusConfig>,
    rules: Vec<(ConsensusConfig, Box<dyn Consensus>)>,
    genesis: Hash,
    /// The chain followed, by height, from base: genesis, or the finalized block a saved client
    /// kept headers from.
    headers: Vec<SealedHeader>,
    base: u64,
    /// Headers below base, newest first, each the parent of the one above: what lets a restored
    /// client check what is in blocks older than it kept. Never saved.
    ancestors: Vec<SealedHeader>,
    /// Each header's total work.
    work: Vec<u256>,
    heights: HashMap<Hash, u64>,
    voters: BTreeMap<u64, Voters>,
    /// The justified checkpoints on the chain, by epoch: the block's id.
    justified: BTreeMap<u64, Hash>,
    finalized: Checkpoint,
    /// The blocks whose updates were taken, or that had none to take, by height.
    updated: BTreeSet<u64>,
    /// Whether the node had no more headers, or justifications, when last asked.
    headers_done: bool,
    justifications_done: bool,
    /// Where to ask for headers from after a branch that did not link.
    back: Option<u64>,
    /// The height of the first header it could not check yet, under a consensus whose producers it
    /// learns from finality: it asks again once their epoch's voters are known.
    stalled: Option<u64>,
    /// What it checked of the chain's finality, kept for a node syncing from a snapshot to store
    /// and serve: None for a client that keeps nothing of it, as most do.
    kept: Option<Kept>,
}

#[document(
    summary = "What a light client checked of a chain's finality and kept, once asked to: each justification it took and each light update it trusted, for a node syncing from a snapshot to serve light clients from genesis.",
    category = "Light client",
    see_also = "LightClient::keep_checked, LightClient::take_checked",
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    #[document(summary = "Justifications, in the order taken.")]
    pub justifications: Vec<Justification>,
    #[document(summary = "Light updates, in the order trusted.")]
    pub updates: Vec<LightUpdate>,
}

impl LightClient {
    #[document(
        parent = "LightClient",
        summary = "A client of the chain with this genesis, once it checked the genesis is the one it trusts and the parameters are the ones it commits to.",
        params = {
            params = "The chain's light parameters, from any node.",
            genesis = "The genesis header, from any node.",
            trusted = "The genesis id the client trusts.",
        },
    )]
    pub fn new(params: LightParams, genesis: SealedHeader, trusted: Hash) -> Result<LightClient, LightError> {
        if genesis.id() != trusted || genesis.header.height != 0 {
            return Err(LightError::WrongGenesis);
        }

        if params.hash != trusted.algorithm() || genesis.header.proof_root != params.hash(params.hash) {
            return Err(LightError::WrongParams);
        }

        let consensus = params.rules();

        Ok(LightClient {
            consensus,
            eras: BTreeMap::new(),
            rules: Vec::new(),
            genesis: trusted,
            heights: HashMap::from([(trusted, 0)]),
            work: vec![u256::ZERO],
            headers: vec![genesis],
            base: 0,
            ancestors: Vec::new(),
            voters: BTreeMap::new(),
            justified: BTreeMap::from([(0, trusted)]),
            finalized: Checkpoint { epoch: 0, block: trusted },
            updated: BTreeSet::new(),
            headers_done: false,
            justifications_done: false,
            back: None,
            stalled: None,
            kept: None,
            params,
        })
    }

    #[document(parent = "LightClient", summary = "The chain's parameters.")]
    pub fn params(&self) -> &LightParams {
        &self.params
    }

    #[document(parent = "LightClient", summary = "The newest header followed.")]
    pub fn tip(&self) -> &BlockHeader {
        &self.headers.last().expect("genesis is always there").header
    }

    #[document(parent = "LightClient", summary = "A header it holds, by height: one it followed, or an ancestor it took.")]
    pub fn header_at(&self, height: u64) -> Option<&BlockHeader> {
        self.sealed(height).map(|sealed| &sealed.header)
    }

    #[document(
        parent = "LightClient",
        summary = "The lowest height it holds a header at: 0 for a client that followed from genesis; for a restored one, the block it kept headers from, less the ancestors it took since.",
        see_also = "LightClient::deliver_ancestors",
    )]
    pub fn lowest(&self) -> u64 {
        self.base - self.ancestors.len() as u64
    }

    #[document(
        parent = "LightClient",
        summary = "Takes the headers just below the lowest it holds, as a restored client needs to check anything in an older block. Each must be the parent of the one above it, the last the parent of the lowest it holds: the hash chain proves them, since every block above was checked already, so no consensus or finality check is needed. They are not saved.",
        params = {
            from = "The first one's height.",
            headers = "The headers from there on, in order, up to the lowest it holds; at most MAX_LIGHT_HEADERS come from a node at once.",
        },
        returns = "How many it took; Misplaced for headers that do not end just below the lowest it holds, BadHeader for one that does not link to the header above it.",
        see_also = "LightClient::lowest",
    )]
    pub fn deliver_ancestors(&mut self, from: u64, headers: Vec<SealedHeader>) -> Result<usize, LightError> {
        let lowest = self.lowest();

        if headers.is_empty() || from.checked_add(headers.len() as u64) != Some(lowest) {
            return Err(LightError::Misplaced);
        }

        // From the top down: each must hash to what the header above names as its parent.
        let mut parent = self.sealed(lowest).expect("it holds its lowest").header.parent;

        for (offset, sealed) in headers.iter().enumerate().rev() {
            let height = from + offset as u64;

            if sealed.header.height != height || sealed.id() != parent {
                return Err(LightError::BadHeader {
                    height,
                    why: "it is not the parent of the header above it".to_string(),
                });
            }

            parent = sealed.header.parent;
        }

        let taken = headers.len();

        for sealed in headers.into_iter().rev() {
            self.heights.insert(sealed.id(), sealed.header.height);
            self.ancestors.push(sealed);
        }

        Ok(taken)
    }

    #[document(parent = "LightClient", summary = "The latest justified checkpoint it checked; genesis's before any.")]
    pub fn justified(&self) -> Checkpoint {
        let (epoch, block) = self.justified.last_key_value().expect("genesis is justified");

        Checkpoint { epoch: *epoch, block: *block }
    }

    #[document(
        parent = "LightClient",
        summary = "The height up to which blocks are final: the finalized checkpoint's under the kinds that vote, the tip's less the depth under depth finality, 0 without finality.",
    )]
    pub fn final_height(&self) -> u64 {
        match self.params.finality {
            Some(LightFinality::Depth { depth }) => self.tip().height.saturating_sub(depth),
            Some(finality) => self.finalized.epoch * finality.checkpoint_interval().unwrap_or(1),
            None => 0,
        }
    }

    #[document(parent = "LightClient", summary = "The finalized checkpoint it checked, under the kinds that vote.")]
    pub fn finalized(&self) -> Checkpoint {
        self.finalized
    }

    #[document(
        parent = "LightClient",
        summary = "What to fetch next: headers until the node has no more, then, under voting finality, the updates of trusted checkpoints and the justifications after the latest; None once caught up. Call refresh to follow the chain again.",
    )]
    pub fn next(&self) -> Option<LightRequest> {
        // Headers that waited for their epoch's producers go first, once those are known.
        if let Some(height) = self.stalled
            && self.producers_for(height).is_some()
        {
            return Some(LightRequest::Headers { from: height, count: MAX_LIGHT_HEADERS });
        }

        if !self.headers_done && self.stalled.is_none() {
            let from = self.back.unwrap_or(self.tip().height + 1);

            return Some(LightRequest::Headers { from, count: MAX_LIGHT_HEADERS });
        }

        let interval = self.params.finality?.checkpoint_interval()?;
        let trusted = self.justified().epoch.min(self.tip().height / interval);
        // The lowest trusted checkpoint whose update it lacks, or else the next justification,
        // once it knows who votes for it: the node sends finality on from there.
        let update = (self.base.div_ceil(interval)..=trusted).find(|epoch| !self.updated.contains(&(epoch * interval)));
        let next = self.justified().epoch + 1;
        let justification = (!self.justifications_done && self.voters.contains_key(&next) && next * interval <= self.tip().height).then_some(next);

        update.or(justification).map(|from| LightRequest::Finality { from })
    }

    #[document(parent = "LightClient", summary = "Asks for what came since: headers, and justifications, again.")]
    pub fn refresh(&mut self) {
        self.headers_done = false;
        self.justifications_done = false;
    }

    /// Who may produce at a height, under a consensus that takes its producers from finality: the
    /// voters of the height's epoch, with their weights, once proved; None until then, or until
    /// the height's rules are known under governance. Empty under any other consensus.
    fn producers_for(&self, height: u64) -> Option<Vec<Member>> {
        if !self.rules_for(height)?.needs_producers() {
            return Some(Vec::new());
        }

        let interval = self.params.finality?.checkpoint_interval()?;
        let voters = self.voters.get(&(height / interval))?;

        Some(voters.set.members.clone())
    }

    #[document(
        parent = "LightClient",
        summary = "The consensus rules a height's header is checked by: genesis's, or, under governance, the consensus the height's epoch was fixed, learned from a light update as the epoch's voters are. None until it is: a header whose rules are not known yet waits, as one whose producers are not.",
    )]
    pub fn rules_for(&self, height: u64) -> Option<&dyn Consensus> {
        if self.params.governance.is_none() {
            return Some(&*self.consensus);
        }

        let interval = self.params.finality?.checkpoint_interval()?;
        let config = self.eras.get(&(height / interval))?;

        self.rules.iter().find(|(known, _)| known == config).map(|(_, rules)| &**rules)
    }

    #[document(
        parent = "LightClient",
        summary = "The consensus the height's epoch runs, as rules_for takes it: genesis's, or what governance fixed for the epoch; None until that is known.",
    )]
    pub fn consensus_at(&self, height: u64) -> Option<&ConsensusConfig> {
        if self.params.governance.is_none() {
            return Some(&self.params.consensus);
        }

        let interval = self.params.finality?.checkpoint_interval()?;

        self.eras.get(&(height / interval))
    }

    /// Learns an epoch's consensus, and builds its rules unless it has them.
    fn learn_era(&mut self, epoch: u64, config: ConsensusConfig) {
        if !self.rules.iter().any(|(known, _)| *known == config) {
            self.rules.push((config.clone(), self.params.rules_of(&config)));
        }

        self.eras.insert(epoch, config);
    }

    /// The most ancestors any rules known look at when checking a header.
    fn lookback(&self) -> usize {
        self.rules.iter().map(|(_, rules)| rules.lookback()).chain([self.consensus.lookback()]).max().unwrap_or(1).max(1)
    }

    #[document(
        parent = "LightClient",
        summary = "Checks the headers a node sent from a height on, and follows them: as more of the chain, or as a heavier branch from where they split off, never past a justified checkpoint.",
        params = {
            from = "The height they start at, as asked.",
            headers = "The node's main-chain headers from there, in order.",
            now = "The clock, in milliseconds since the Unix epoch, for consensus's check of timestamps.",
        },
    )]
    pub fn deliver_headers(&mut self, from: u64, headers: Vec<SealedHeader>, now: u64) -> Result<Extended, LightError> {
        let Some(first) = headers.first() else {
            self.headers_done = self.back.is_none();
            self.back = None;

            return Ok(Extended::UpToDate);
        };

        if from <= self.base || from > self.tip().height + 1 || first.header.height != from {
            return Err(LightError::Misplaced);
        }

        self.stalled = None;

        if first.header.parent != self.sealed(from - 1).expect("from is past base").id() {
            // A chain that leaves even the oldest header kept leaves the finalized block it was
            // kept from.
            if from == self.base + 1 {
                return Err(LightError::ConflictsWithFinality);
            }

            // The node's chain split off below from: look further back.
            self.back = Some(from.saturating_sub(MAX_LIGHT_HEADERS as u64).max(self.base + 1));

            return Ok(Extended::Unlinked);
        }

        self.back = None;
        let mut work = self.work[(from - 1 - self.base) as usize];
        // What consensus looks at: the parent first, then as many below as it wants and the
        // client holds.
        let lookback = self.lookback();
        let mut ancestors: Vec<BlockHeader> = (1..=lookback as u64)
            .map_while(|back| from.checked_sub(back))
            .map_while(|height| self.sealed(height).map(|sealed| sealed.header.clone()))
            .collect();

        let mut headers = headers;
        let mut checked = 0;

        for sealed in &headers {
            let height = sealed.header.height;
            let parent = ancestors.first().expect("from is past base");

            if height != parent.height + 1 || sealed.header.parent != parent.id() {
                return Err(LightError::BadHeader { height, why: "does not follow its parent".to_string() });
            }

            // Producers the client cannot know yet: it keeps what it checked, and asks again once
            // finality has named them.
            let Some(producers) = self.producers_for(height) else {
                self.stalled = Some(height);

                break;
            };

            let rules = self.rules_for(height).expect("producers_for found them");
            rules
                .check(&sealed.header, &sealed.seal, &ancestors, &producers, now)
                .map_err(|err: ConsensusError| LightError::BadHeader { height, why: err.to_string() })?;
            work += rules.work(&sealed.header);
            ancestors.insert(0, sealed.header.clone());
            ancestors.truncate(lookback);
            checked += 1;
        }

        headers.truncate(checked);

        if headers.is_empty() {
            return Ok(Extended::Waiting);
        }

        let fork = from - 1;
        let ours = *self.work.last().expect("genesis is always there");

        if fork == self.tip().height {
            let added = headers.len();
            self.append(headers, from);

            return Ok(Extended::Added(added));
        }

        if work <= ours {
            return Ok(Extended::Lighter);
        }

        if self.justified().epoch > 0 && self.justified_height() > fork {
            return Err(LightError::ConflictsWithFinality);
        }

        for dropped in self.headers.drain((from - self.base) as usize..) {
            self.heights.remove(&dropped.id());
        }

        self.work.truncate((from - self.base) as usize);
        self.updated.retain(|height| *height <= fork);
        self.append(headers, from);

        Ok(Extended::Switched(from))
    }

    fn append(&mut self, headers: Vec<SealedHeader>, from: u64) {
        let mut work = self.work[(from - 1 - self.base) as usize];
        // Justifications that waited for headers past the tip may hold now.
        self.justifications_done = false;

        for sealed in headers {
            work += self.rules_for(sealed.header.height).expect("checked before appended").work(&sealed.header);
            self.heights.insert(sealed.id(), sealed.header.height);
            self.work.push(work);
            self.headers.push(sealed);
        }
    }

    #[document(
        parent = "LightClient",
        summary = "A header it holds and its seal, by height: one it followed and checked, or an ancestor below those, trusted by its hash. None above the tip or below the lowest held.",
    )]
    pub fn sealed_at(&self, height: u64) -> Option<&SealedHeader> {
        self.sealed(height)
    }

    #[document(
        parent = "LightClient",
        summary = "The total work of the header it followed at a height, counted from the first one it followed: from genesis, or from the first header of a checkpoint start, whose own work counts as nothing. None for a height it did not follow.",
    )]
    pub fn work_at(&self, height: u64) -> Option<u256> {
        self.work.get(usize::try_from(height.checked_sub(self.base)?).ok()?).copied()
    }

    /// A header it holds and its seal by height: one it followed, or an ancestor below those.
    fn sealed(&self, height: u64) -> Option<&SealedHeader> {
        match height.checked_sub(self.base) {
            Some(above) => self.headers.get(usize::try_from(above).ok()?),
            None => self.ancestors.get(usize::try_from(self.base - 1 - height).ok()?),
        }
    }

    #[document(
        parent = "LightClient",
        summary = "What the client checked, to keep and restore it from: its headers from the finalized block on, since nothing below can come off, at most MAX_KEPT_HEADERS of them; the voters it learned; and the checkpoints it saw justified and finalized.",
        see_also = "LightClient::restore",
    )]
    pub fn save(&self) -> LightState {
        let tip = self.tip().height;
        let final_height = self.final_height().min(tip);
        let mut keep = match self.params.finality {
            Some(_) if final_height > 0 => final_height,
            _ => tip.saturating_sub(MAX_KEPT_HEADERS),
        };
        keep = keep.max(tip.saturating_sub(MAX_KEPT_HEADERS)).max(self.base);
        let from = (keep - self.base) as usize;
        let justified = self.justified();
        let interval = self.params.finality.and_then(|finality| finality.checkpoint_interval()).unwrap_or(1);

        LightState {
            params: self.params.clone(),
            genesis: self.genesis,
            headers: self.headers[from..].to_vec(),
            work: self.work[from..].iter().map(|work| work.to_le_bytes()).collect(),
            voters: self
                .voters
                .iter()
                .filter(|(epoch, _)| **epoch >= self.finalized.epoch)
                .map(|(epoch, voters)| (*epoch, voters.set.clone(), voters.needed.to_le_bytes()))
                .collect(),
            justified: self
                .justified
                .iter()
                .filter(|(epoch, _)| **epoch * interval >= keep || **epoch == justified.epoch)
                .map(|(epoch, block)| (*epoch, *block))
                .collect(),
            finalized: self.finalized,
            updated: self.updated.iter().copied().filter(|height| *height >= keep).collect(),
            ancestors: (1..self.lookback() as u64)
                .map_while(|back| keep.checked_sub(back))
                .map_while(|height| self.sealed(height).cloned())
                .collect(),
            eras: self
                .eras
                .iter()
                .filter(|(epoch, _)| **epoch >= self.finalized.epoch)
                .map(|(epoch, config)| (*epoch, config.clone()))
                .collect(),
        }
    }

    #[document(
        parent = "LightClient",
        summary = "A client from what one saved, to follow on from there rather than from genesis: once it checked the state is for the genesis it trusts, and its headers link up.",
        params = {
            state = "What LightClient::save gave.",
            trusted = "The genesis id the client trusts.",
        },
        returns = "The client, WrongGenesis for a state of another chain, Undecodable for one whose headers do not link.",
    )]
    pub fn restore(state: LightState, trusted: Hash) -> Result<LightClient, LightError> {
        if state.genesis != trusted || state.params.hash != trusted.algorithm() {
            return Err(LightError::WrongGenesis);
        }

        let Some(first) = state.headers.first() else {
            return Err(LightError::Undecodable);
        };
        let base = first.header.height;
        let links = state.headers.windows(2).all(|pair| pair[1].header.parent == pair[0].id() && pair[1].header.height == pair[0].header.height + 1);

        if !links || state.work.len() != state.headers.len() || (base == 0 && first.id() != trusted) {
            return Err(LightError::Undecodable);
        }

        // The ancestors hang off the first header, each the parent of the one above.
        let mut above = (first.header.parent, base);

        for sealed in &state.ancestors {
            if sealed.id() != above.0 || sealed.header.height + 1 != above.1 {
                return Err(LightError::Undecodable);
            }

            above = (sealed.header.parent, sealed.header.height);
        }

        let mut heights: HashMap<Hash, u64> = state.headers.iter().map(|sealed| (sealed.id(), sealed.header.height)).collect();
        heights.extend(state.ancestors.iter().map(|sealed| (sealed.id(), sealed.header.height)));
        let mut justified: BTreeMap<u64, Hash> = state.justified.into_iter().collect();

        if justified.is_empty() {
            justified.insert(0, trusted);
        }

        let mut client = LightClient {
            consensus: state.params.rules(),
            eras: BTreeMap::new(),
            rules: Vec::new(),
            genesis: trusted,
            headers: state.headers,
            base,
            ancestors: state.ancestors,
            work: state.work.into_iter().map(u256::from_le_bytes).collect(),
            heights,
            voters: state.voters.into_iter().map(|(epoch, set, needed)| (epoch, Voters { set, needed: u256::from_le_bytes(needed) })).collect(),
            justified,
            finalized: state.finalized,
            updated: state.updated.into_iter().collect(),
            headers_done: false,
            justifications_done: false,
            back: None,
            stalled: None,
            kept: None,
            params: state.params,
        };

        for (epoch, config) in state.eras {
            client.learn_era(epoch, config);
        }

        Ok(client)
    }

    #[document(
        parent = "LightClient",
        summary = "A client that starts at a checkpoint it trusts, rather than following from genesis: once it checked the genesis and parameters as new does, that the headers link up by their hashes to the checkpoint, so it trusts them all, and the updates among them against their state roots. It then follows on from the checkpoint, and never below it. Under voting finality the checkpoint is one an epoch opens; under any other, any block.",
        params = {
            params = "The chain's light parameters, from any node.",
            genesis = "The genesis header, from any node.",
            trusted = "The genesis id the client trusts.",
            checkpoint = "The block id it trusts, as someone it trusts gave it, or the node on first use.",
            start = "What the node sent, from GET /light/checkpoint.",
        },
        returns = "The client, or WrongCheckpoint for headers that do not end at the checkpoint or start too late, and what take_update finds of an update.",
    )]
    pub fn from_checkpoint(params: LightParams, genesis: SealedHeader, trusted: Hash, checkpoint: Hash, start: CheckpointStart) -> Result<LightClient, LightError> {
        // The start's own work is counted by genesis's rules: it is never compared with anything,
        // since nothing below the checkpoint is followed.
        let consensus = LightClient::new(params.clone(), genesis, trusted)?.consensus;
        let CheckpointStart { headers, updates } = start;
        let (Some(first), Some(last)) = (headers.first(), headers.last()) else {
            return Err(LightError::WrongCheckpoint);
        };
        let links = headers.windows(2).all(|pair| pair[1].header.parent == pair[0].id() && pair[1].header.height == pair[0].header.height + 1);

        if last.id() != checkpoint || !links || (first.header.height == 0 && first.id() != trusted) {
            return Err(LightError::WrongCheckpoint);
        }

        let height = last.header.height;
        let interval = params.finality.and_then(|finality| finality.checkpoint_interval().filter(|_| finality.fixed_ahead().is_some()));
        let (epoch, from) = checkpoint_start(params.finality, height).ok_or(LightError::WrongCheckpoint)?;
        let wanted = interval.map_or(0, |interval| (height - from) / interval + 1);

        if first.header.height > checkpoint_first_header(from, height, consensus.lookback()) || updates.len() as u64 != wanted {
            return Err(LightError::WrongCheckpoint);
        }

        // Headers below the epochs' start are ancestors: held for consensus to look at, trusted by
        // their hashes alone, and never followed from.
        let split = (from - first.header.height) as usize;
        let ancestors: Vec<SealedHeader> = headers[..split].iter().rev().cloned().collect();
        let headers: Vec<SealedHeader> = headers[split..].to_vec();
        let mut sum = u256::ZERO;
        let work = headers
            .iter()
            .enumerate()
            .map(|(index, sealed)| {
                if index > 0 {
                    sum += consensus.work(&sealed.header);
                }

                sum.to_le_bytes()
            })
            .collect();
        let at = Checkpoint { epoch, block: checkpoint };
        let mut client = LightClient::restore(
            LightState {
                params,
                genesis: trusted,
                headers,
                work,
                voters: Vec::new(),
                justified: vec![(epoch, checkpoint)],
                finalized: at,
                updated: Vec::new(),
                ancestors,
                eras: Vec::new(),
            },
            trusted,
        )?;

        if let Some(interval) = interval {
            for (offset, update) in updates.into_iter().enumerate() {
                let block = client.sealed(from + offset as u64 * interval).ok_or(LightError::WrongCheckpoint)?.id();
                client.take_update(&block, update)?;
            }
        }

        Ok(client)
    }

    fn justified_height(&self) -> u64 {
        let interval = self.params.finality.and_then(|finality| finality.checkpoint_interval()).unwrap_or(1);

        self.justified().epoch * interval
    }

    #[document(
        parent = "LightClient",
        summary = "Takes a block's light update, or its absence when the node has none: proves the voters it fixed against the block's state root, which the client trusts once the block is at or below its latest justified checkpoint.",
        params = {
            block = "The block asked for.",
            update = "What the node sent; None when it has none, as a node that synced from a snapshot lacks those before it.",
        },
    )]
    pub fn take_update(&mut self, block: &Hash, update: Option<LightUpdate>) -> Result<(), LightError> {
        let height = *self.heights.get(block).ok_or(LightError::NotOnChain)?;

        if height > self.justified_height() {
            return Err(LightError::Untrusted);
        }

        let Some(update) = update else {
            self.updated.insert(height);

            return Ok(());
        };
        let finality = self.params.finality.ok_or(LightError::BadProof)?;
        let root = self.sealed(height).ok_or(LightError::NotOnChain)?.header.state_root;
        let (learned, eras) = update.check(block, &finality, height, &root)?;

        self.voters.extend(learned);

        for (epoch, config) in eras {
            self.learn_era(epoch, config);
        }

        self.updated.insert(height);

        if let Some(kept) = &mut self.kept {
            kept.updates.push(update);
        }

        Ok(())
    }

    #[document(
        parent = "LightClient",
        summary = "From now on, keeps each justification it takes and each light update it trusts, for take_checked: what a node syncing from a snapshot stores, to serve light clients from genesis.",
        params = { already = "What it checked before keeping began, to keep with the rest: a checkpoint start's updates, which from_checkpoint took." },
    )]
    pub fn keep_checked(&mut self, already: Kept) {
        self.kept = Some(already);
    }

    #[document(parent = "LightClient", summary = "Takes what it kept since keep_checked, leaving it keeping on; empty for a client that keeps nothing.")]
    pub fn take_checked(&mut self) -> Kept {
        self.kept.as_mut().map(std::mem::take).unwrap_or_default()
    }

    #[document(
        parent = "LightClient",
        summary = "Checks the justifications a node sent, in order, and takes each that holds: its target on the client's chain at its checkpoint, its source justified, and votes from the target epoch's voters adding up to enough. The source of a direct successor is finalized. It stops, without an error, at one whose voters it cannot know yet: the checkpoints it justified make updates trusted that tell it, and next asks for them, then for the rest again. It stops too at one past the headers it followed, which the node justified after sending them: the next refresh brings both.",
        returns = "How many it took; the first that does not hold stops it, with why.",
    )]
    pub fn deliver_justifications(&mut self, justifications: &[Justification]) -> Result<usize, LightError> {
        if justifications.is_empty() {
            self.justifications_done = true;
        }

        let mut taken = 0;

        for justification in justifications {
            match self.take_justification(justification)? {
                Took::Taken => taken += 1,
                Took::Old => {}
                Took::Waits => break,
            }
        }

        Ok(taken)
    }

    #[document(
        parent = "LightClient",
        summary = "Checks the finality a node sent from an epoch on, an epoch at a time, as deliver_justifications and take_update check them one by one: each epoch's justification first, then the update its checkpoint keeps, which that justification may just have made trusted, and which names the voters of justifications further on. So one answer carries the client as far as many requests did. It stops, without an error, at an epoch the node justified no further than, at one past the headers it followed, and at one whose voters it cannot know yet.",
        params = {
            from = "The epoch they start at, as asked.",
            steps = "The node's finality for each epoch from there, in order; at most MAX_FINALITY_STEPS come from a node at once.",
        },
        returns = "How many justifications it took; the first step that does not hold stops it, with why.",
    )]
    pub fn deliver_finality(&mut self, from: u64, steps: Vec<FinalityStep>) -> Result<usize, LightError> {
        let interval = self
            .params
            .finality
            .and_then(|finality| finality.checkpoint_interval())
            .ok_or(LightError::BadJustification("the chain does not vote"))?;
        let first = self.base.div_ceil(interval);
        let mut taken = 0;

        if steps.is_empty() {
            self.justifications_done = true;
        }

        for (epoch, step) in (from..).zip(steps) {
            if epoch > self.justified().epoch {
                let Some(justification) = &step.justification else {
                    self.justifications_done = true;

                    break;
                };

                if justification.target.epoch != epoch {
                    return Err(LightError::Misplaced);
                }

                match self.take_justification(justification)? {
                    Took::Taken => taken += 1,
                    Took::Old => {}
                    Took::Waits => break,
                }
            }

            let height = epoch * interval;

            if epoch >= first && height <= self.justified_height() && !self.updated.contains(&height) {
                let block = self.sealed(height).ok_or(LightError::NotOnChain)?.id();
                self.take_update(&block, step.update)?;
            }
        }

        Ok(taken)
    }

    /// Takes a justification that holds, or says why it waits.
    fn take_justification(&mut self, justification: &Justification) -> Result<Took, LightError> {
        if justification.target.epoch <= self.justified().epoch {
            return Ok(Took::Old);
        }

        // The chain went on between the headers and this: it waits for the headers.
        let interval = self.params.finality.and_then(|finality| finality.checkpoint_interval());

        if interval.is_some_and(|interval| justification.target.epoch * interval > self.tip().height) {
            self.justifications_done = true;

            return Ok(Took::Waits);
        }

        if !self.voters.contains_key(&justification.target.epoch) {
            return Ok(Took::Waits);
        }

        self.check_justification(justification)?;
        let (source, target) = (justification.source, justification.target);
        self.justified.insert(target.epoch, target.block);

        if let Some(kept) = &mut self.kept {
            kept.justifications.push(justification.clone());
        }

        if target.epoch == source.epoch + 1 && source.epoch > self.finalized.epoch {
            self.finalized = source;
        }

        Ok(Took::Taken)
    }

    fn check_justification(&self, justification: &Justification) -> Result<(), LightError> {
        let (source, target) = (justification.source, justification.target);
        let interval = self
            .params
            .finality
            .and_then(|finality| finality.checkpoint_interval())
            .ok_or(LightError::BadJustification("the chain does not vote"))?;
        let on_chain = |checkpoint: &Checkpoint| self.sealed(checkpoint.epoch * interval).is_some_and(|sealed| sealed.id() == checkpoint.block);

        if source.epoch >= target.epoch || !on_chain(&target) || !on_chain(&source) {
            return Err(LightError::BadJustification("its link is not on the chain followed"));
        }

        if self.justified.get(&source.epoch) != Some(&source.block) {
            return Err(LightError::BadJustification("its source is not justified"));
        }

        let voters = self.voters.get(&target.epoch).ok_or(LightError::UnknownVoters(target.epoch))?;
        let mut weight = u256::ZERO;
        let mut previous: Option<PublicKey> = None;

        for vote in &justification.votes {
            if previous.is_some_and(|previous| previous >= vote.vote.voter) {
                return Err(LightError::BadJustification("its votes are not sorted by voter"));
            }

            if vote.vote.source != source || vote.vote.target != target {
                return Err(LightError::BadJustification("a vote is for another link"));
            }

            vote.verify(&self.genesis).map_err(|_| LightError::BadJustification("a vote's signature does not check"))?;
            weight += voters
                .set
                .member(&vote.vote.voter)
                .ok_or(LightError::BadJustification("a vote is from someone who does not vote"))?
                .weight;
            previous = Some(vote.vote.voter);
        }

        if weight < voters.needed {
            return Err(LightError::BadJustification("its votes fall short"));
        }

        Ok(())
    }

    fn assurance(&self, block: &Hash) -> Result<(Assurance, &BlockHeader), LightError> {
        let height = *self.heights.get(block).ok_or(LightError::NotOnChain)?;
        let header = &self.sealed(height).ok_or(LightError::NotOnChain)?.header;

        Ok((
            Assurance {
                block: *block,
                height,
                final_: self.params.finality.is_some() && height <= self.final_height(),
            },
            header,
        ))
    }

    #[document(
        parent = "LightClient",
        summary = "Checks a chain-state entry a node sent, with its proof, against the state root of a block the client followed.",
        params = {
            block = "The block it is proven at: a node's tip, or its finalized checkpoint.",
            column = "The entry's column.",
            key = "Its key.",
            value = "What the node says it holds, or None for nothing.",
            proof = "The node's proof.",
        },
        returns = "How far to trust it, or BadProof, or NotOnChain for a block the client has not followed.",
    )]
    pub fn check_state(&self, block: &Hash, column: Column, key: &[u8], value: Option<&[u8]>, proof: &StateProof) -> Result<Assurance, LightError> {
        let (assurance, header) = self.assurance(block)?;

        match proof.verify(&header.state_root, column, key, value) {
            true => Ok(assurance),
            false => Err(LightError::BadProof),
        }
    }

    #[document(
        parent = "LightClient",
        summary = "Checks a receipt a node sent, with its Merkle path, against the receipts root of a block the client followed.",
        params = {
            block = "The block it landed in.",
            receipt = "The CommittedReceipt, encoded.",
            index = "Its place among the block's transactions.",
            count = "How many there are.",
            path = "Its Merkle path.",
        },
        returns = "The receipt, and how far to trust it.",
    )]
    pub fn check_receipt(&self, block: &Hash, receipt: &[u8], index: usize, count: usize, path: &[Hash]) -> Result<(CommittedReceipt, Assurance), LightError> {
        let (assurance, header) = self.assurance(block)?;
        let leaf = merkle::leaf(self.params.hash, receipt);

        if !merkle::verify(&header.receipts_root, &leaf, index, count, path) {
            return Err(LightError::BadProof);
        }

        let decoded: CommittedReceipt = wincode::deserialize(receipt).map_err(|_| LightError::Undecodable)?;

        Ok((decoded, assurance))
    }

    #[document(
        parent = "LightClient",
        summary = "Checks a signed transaction a node sent, with its Merkle path, against the tx_root of a block the client followed: who sent what, which its receipt alone does not say.",
        params = {
            block = "The block it landed in.",
            tx = "The SignedTx, encoded.",
            index = "Its place among the block's transactions.",
            count = "How many there are.",
            path = "Its Merkle path.",
        },
        returns = "The transaction, and how far to trust it.",
        see_also = "LightClient::check_receipt, Block::tx_root",
    )]
    pub fn check_tx(&self, block: &Hash, tx: &[u8], index: usize, count: usize, path: &[Hash]) -> Result<(SignedTx, Assurance), LightError> {
        let (assurance, header) = self.assurance(block)?;
        let leaf = merkle::leaf(self.params.hash, tx);

        if !merkle::verify(&header.tx_root, &leaf, index, count, path) {
            return Err(LightError::BadProof);
        }

        let decoded: SignedTx = wincode::deserialize(tx).map_err(|_| LightError::Undecodable)?;

        Ok((decoded, assurance))
    }
}
