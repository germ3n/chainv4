//! Proofs of finality, beside the proofs of ranges of blocks: what is final, rather than whether the
//! blocks were right.
//!
//! **A finality proof** proves one link: two thirds of the target epoch's voters, by weight, signed
//! a vote from the source checkpoint to the target. Their own keys and votes are checked, not the
//! attesters'. The voters are read from a witness of the source's state, which fixed them two
//! epochs ahead, and the proof says the source's state root: whoever takes it checks that root
//! against the one it holds for the source, as ChainV4Finality.sol does for any checkpoint it
//! recorded, by attestations or by proofs. So nothing is tracked from link to link, and the two
//! paths take turns as they like. A witness of the target's state, whose root its header names and
//! the votes vouch for, gives the next epoch's attesting set and governance's finality program.
//!
//! **A header-chain proof** proves the chain's finalized checkpoints from genesis: the links from
//! the last proof's justified checkpoint on, each by the voters its source's state fixed, and the
//! previous proof itself, checked inside by its kind's verifier. One proof then carries a new app
//! from genesis to a recent final checkpoint, with no votes to download.
//!
//! Each is its own program (proofs/sp1/program's binaries), named by governance in Change::Proofs.

use {
    super::links::ProvenLink,
    super::replay::{WitnessDb, fail, witness_of},
    super::{ReplayError, Witness},
    crate::chain::{Chain, json::from_json, stored_header_at},
    crate::db::{Committed, DbError, Historic, MemoryDb, Recording, State},
    crate::registry::changes::Changes,
    crate::structs::attestation::AttestingSet,
    crate::structs::block::SealedHeader,
    crate::structs::u256::u256,
    crate::structs::vote::{Checkpoint, SignedVote},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[cfg(feature = "header-proofs")]
use {
    super::links::{ChainPoint, ProvenHeaders, TakenHeaders},
    crate::chain::stored_header,
};

#[cfg(feature = "header-proofs")]
#[document(
    summary = "The most links one header-chain proof takes: a longer stretch is proven a step at a time, each proof checking the one before.",
    category = "Proofs",
    see_also = "HeadersInput",
)]
pub const MAX_HEADER_LINKS: usize = 256;

#[document(
    summary = "One link as a proof takes it: its source and the source's state root, the target's header, the votes for it, a witness of the source's state, which holds the target epoch's voters, and one of the target's, which holds the next epoch's attesters and governance's settings.",
    category = "Proofs",
    see_also = "FinalityInput, HeadersInput, check_link",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct LinkInput {
    #[document(summary = "The link's source.")]
    pub source: Checkpoint,
    #[document(summary = "The source's state root, which the proof says for its taker to check.")]
    pub source_root: crate::structs::hash::Hash,
    #[document(summary = "The target's header.")]
    pub target: SealedHeader,
    #[document(summary = "The votes for it, by voter ascending.")]
    pub votes: Vec<SignedVote>,
    #[document(summary = "The source's state, as much as reading the target epoch's voters takes.")]
    pub source_witness: Witness,
    #[document(summary = "The target's state, as much as the check reads of it.")]
    pub witness: Witness,
}

#[document(
    summary = "Everything a finality proof runs on: the genesis and one link.",
    category = "Proofs",
    see_also = "prove_finality",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct FinalityInput {
    #[document(summary = "The chain's genesis.json.")]
    pub genesis: Vec<u8>,
    #[document(summary = "The link.")]
    pub link: LinkInput,
}

#[cfg(feature = "header-proofs")]
#[document(
    summary = "Everything a header-chain proof runs on: the genesis, the program's own id, the previous proof, None to start from genesis, and the links after it, in order.",
    category = "Proofs",
    see_also = "prove_headers",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct HeadersInput {
    #[document(summary = "The chain's genesis.json.")]
    pub genesis: Vec<u8>,
    #[document(summary = "The program's id, which the previous proof must be by and this one commits to.")]
    pub program: [u8; 32],
    #[document(summary = "The proof this one goes on from, or None from genesis.")]
    pub previous: Option<TakenHeaders>,
    #[document(summary = "The links, each starting where the one before ended.")]
    pub links: Vec<LinkInput>,
}

fn open(genesis: &[u8]) -> Result<Chain, ReplayError> {
    let json: serde_json::Value = serde_json::from_slice(genesis).map_err(|err| fail(format!("genesis.json: {err}")))?;
    let genesis = from_json(&json).map_err(|err| fail(format!("genesis.json: {err}")))?;

    Chain::open(Arc::new(MemoryDb::default()), &genesis).map_err(|err| fail(format!("genesis: {err}")))
}

fn interval(chain: &Chain) -> Result<u64, ReplayError> {
    chain.finality_config().map(|config| config.checkpoint_interval()).filter(|interval| *interval > 0).ok_or_else(|| fail("the chain's finality has no checkpoints"))
}

#[document(
    summary = "Checks one link: the target a checkpoint after the source; the target epoch's voters read from the witness of the source's state, checked against the source root the link names; and votes for exactly this link, each well signed by one of them, at least the weight a link needs. The target's witness, checked against its header's root, gives the next attesting set. What both programs run for each link; the source root is the taker's to check."
    params = { chain = "A chain of the genesis: its rules alone are used.", link = "The link." },
    returns = "What it proves.",
    category = "Proofs",
    see_also = "LinkInput, ProvenLink",
)]
pub fn check_link(chain: &Chain, link: &LinkInput) -> Result<ProvenLink, ReplayError> {
    let interval = interval(chain)?;
    let header = &link.target.header;

    if header.height == 0 || !header.height.is_multiple_of(interval) {
        return Err(fail(format!("the target at {} is no checkpoint", header.height)));
    }

    let target = Checkpoint { epoch: header.height / interval, block: header.id() };

    if link.source.epoch >= target.epoch {
        return Err(fail("a link to a checkpoint no later than its source"));
    }

    let source_state = State::new(Arc::new(WitnessDb::new(chain.hash(), &link.source_root, &link.source_witness, &[])?));
    let (set, needed) = chain.voters_in(&source_state, target.epoch)?.ok_or_else(|| fail(format!("the source's state fixes no voters for epoch {}: a link that skips more epochs than voters are fixed ahead", target.epoch)))?;
    let state = State::new(Arc::new(WitnessDb::new(chain.hash(), &header.state_root, &link.witness, std::slice::from_ref(&link.target))?));
    let (next, next_needed) = chain.voters_in(&state, target.epoch + 1)?.ok_or_else(|| fail(format!("no voters for epoch {} in the target's state", target.epoch + 1)))?;
    // The attesting set as attesters sign it at the target: naming its threshold from the proofs fork.
    let next_attesting = AttestingSet::of(next.members.iter().map(|member| (member.attester, member.weight)));
    let next_attesting = match Changes::new(&state).forks()?.at(header.height).has(&crate::params::forks::PROOFS) {
        true => next_attesting.needing(next_needed),
        false => next_attesting,
    };
    let genesis = chain.genesis();
    let mut weight = u256::ZERO;
    let mut previous = None;

    for vote in &link.votes {
        if previous.is_some_and(|previous| previous >= vote.vote.voter) {
            return Err(fail("votes not by voter ascending, each once"));
        }

        if vote.vote.source != link.source || vote.vote.target != target {
            return Err(fail("a vote for another link"));
        }

        vote.verify(&genesis).map_err(|err| fail(format!("a vote that is not its voter's: {err}")))?;
        weight = weight.saturating_add(set.member(&vote.vote.voter).ok_or_else(|| fail("a vote by no voter of the epoch"))?.weight);
        previous = Some(vote.vote.voter);
    }

    if weight < needed {
        return Err(fail(format!("votes of {weight}, short of the {needed} a link needs")));
    }

    let finality_program = Changes::new(&state).proofs()?.map_or([0; 32], |settings| settings.finality_program);

    Ok(ProvenLink {
        chain_id: crate::crypto::ethereum::chain_id(&genesis),
        genesis,
        source: link.source,
        source_root: link.source_root,
        target,
        height: header.height,
        state_root: header.state_root,
        next_attesting_set: next_attesting.hash(),
        finality_program,
    })
}

#[document(
    summary = "Runs a finality proof's input and says what it proves: what the finality program is.",
    params = { input = "The input." },
    category = "Proofs",
    see_also = "FinalityInput, check_link",
)]
pub fn prove_finality(input: &FinalityInput) -> Result<ProvenLink, ReplayError> {
    check_link(&open(&input.genesis)?, &input.link)
}

#[cfg(feature = "header-proofs")]
#[document(
    summary = "Runs a header-chain proof's input and says what it proves: what the header-chain program is. From the previous proof, checked first by its kind's verifier under this program, or from genesis; then each link, starting at the justified checkpoint so far, with its state root, which fixed the link's voters, finalizing its source when its target is the very next epoch. It must finalize something new.",
    params = { input = "The input." },
    category = "Proofs",
    see_also = "HeadersInput, ProvenHeaders",
)]
pub fn prove_headers(input: &HeadersInput) -> Result<ProvenHeaders, ReplayError> {
    let chain = open(&input.genesis)?;
    let genesis = chain.genesis();
    let chain_id = crate::crypto::ethereum::chain_id(&genesis);

    if input.links.len() > MAX_HEADER_LINKS {
        return Err(fail(format!("more than {MAX_HEADER_LINKS} links in one proof")));
    }

    let (mut finalized, mut justified) = match &input.previous {
        Some(previous) => {
            let before = ProvenHeaders::from_public_values(chain.hash(), &previous.values).ok_or_else(|| fail("a previous proof whose public values are no header chain"))?;

            if before.genesis != genesis || before.chain_id != chain_id {
                return Err(fail("a previous proof of another chain"));
            }

            if before.program != input.program {
                return Err(fail("a previous proof by another program: one by an older program starts again from genesis"));
            }

            let verifier = super::verifier(previous.kind).ok_or_else(|| fail(format!("this build checks no {} proofs", previous.kind.name())))?;
            verifier.verify(&input.program, &previous.values, &previous.proof).map_err(|why| fail(format!("the previous proof: {why}")))?;

            (before.finalized, before.justified)
        }
        None => {
            let root = chain.block_at(0)?.ok_or_else(|| fail("no genesis block"))?.header.state_root;
            let start = ChainPoint { checkpoint: Checkpoint { epoch: 0, block: genesis }, height: 0, state_root: root };

            (start, start)
        }
    };
    let before = finalized.checkpoint.epoch;

    for link in &input.links {
        // From the justified checkpoint so far, whose state, as this proof holds its root, fixed
        // the link's voters.
        if link.source != justified.checkpoint || link.source_root != justified.state_root {
            return Err(fail(format!("a link from epoch {}, where the justified checkpoint is epoch {}", link.source.epoch, justified.checkpoint.epoch)));
        }

        let proven = check_link(&chain, link)?;

        if proven.target.epoch == link.source.epoch + 1 {
            finalized = justified;
        }

        justified = ChainPoint { checkpoint: proven.target, height: proven.height, state_root: proven.state_root };
    }

    if finalized.checkpoint.epoch <= before && input.previous.is_some() {
        return Err(fail("a proof that finalizes nothing new"));
    }

    Ok(ProvenHeaders { chain_id, genesis, program: input.program, finalized, justified })
}

impl LinkInput {
    #[document(
        parent = "LinkInput",
        summary = "Builds a link's input from a node's database: the justification stored for the checkpoint of an epoch, its target's header, and a witness of the target's state for what the check reads, built through history.",
        params = {
            chain = "A chain of the genesis, whose rules decide what is read.",
            db = "The node's database, which holds history back to the target: an archive, or a --state-history window that reaches it.",
            epoch = "The target's epoch.",
        },
        returns = "The input, or why not: no checkpoint or justification there, or history that does not reach it.",
    )]
    pub fn build(chain: &Chain, db: Arc<dyn Committed>, epoch: u64) -> Result<LinkInput, ReplayError> {
        let height = epoch.checked_mul(interval(chain)?).ok_or_else(|| fail("an epoch past any height"))?;
        let target = stored_header_at(&*db, height)?.ok_or_else(|| fail(format!("no block at {height} on the main chain")))?;
        let justification = crate::chain::stored_justification(&*db, &target.id())?.ok_or_else(|| fail(format!("epoch {epoch}'s checkpoint is not justified here")))?;

        if crate::chain::stored_state_at(db.clone(), &target.id())?.is_none() {
            return Err(fail(format!("history here does not reach epoch {epoch}'s checkpoint at {height}: build it on an archive, or a node whose --state-history reaches it")));
        }

        // What the check reads of a state, through history at a height, as a witness.
        let witnessed = |at: u64, read: &dyn Fn(&State) -> Result<(), DbError>| -> Result<Witness, ReplayError> {
            let historic: Arc<dyn Committed> = Arc::new(Historic::new(db.clone(), at));
            let recording = Arc::new(Recording::new(historic.clone()));
            read(&State::new(recording.clone()))?;

            witness_of(chain.hash(), &historic, &recording, "a link's check")
        };
        let source = stored_header_at(&*db, justification.source.epoch * interval(chain)?)?.filter(|sealed| sealed.id() == justification.source.block).ok_or_else(|| fail(format!("epoch {epoch}'s source is not on the main chain here")))?;

        if crate::chain::stored_state_at(db.clone(), &source.id())?.is_none() {
            return Err(fail(format!("history here does not reach the source at {}: build it on an archive, or a node whose --state-history reaches it", source.header.height)));
        }

        let source_witness = witnessed(source.header.height, &|state| chain.voters_in(state, epoch).map(|_| ()))?;
        let witness = witnessed(height, &|state| {
            chain.voters_in(state, epoch + 1)?;
            Changes::new(state).forks()?;
            Changes::new(state).proofs().map(|_| ())
        })?;
        let mut votes = justification.votes;
        votes.sort_by_key(|vote| vote.vote.voter);
        // The attestations ride along unsigned by the vote; the proof needs none.
        votes.iter_mut().for_each(|vote| vote.attestation = None);

        Ok(LinkInput { source: justification.source, source_root: source.header.state_root, target, votes, source_witness, witness })
    }
}

impl FinalityInput {
    #[document(parent = "FinalityInput", summary = "Builds a finality proof's input for the link to an epoch's checkpoint, as LinkInput::build does.")]
    pub fn build(chain: &Chain, db: Arc<dyn Committed>, genesis: Vec<u8>, epoch: u64) -> Result<FinalityInput, ReplayError> {
        Ok(FinalityInput { genesis, link: LinkInput::build(chain, db, epoch)? })
    }

    #[document(parent = "FinalityInput", summary = "The input as bytes, as GET /validity/finality-input sends it and the zkVM reads it.")]
    pub fn to_bytes(&self) -> Vec<u8> {
        wincode::serialize(self).expect("a finality input always encodes")
    }

    #[document(parent = "FinalityInput", summary = "An input from its bytes.")]
    pub fn from_bytes(bytes: &[u8]) -> Result<FinalityInput, ReplayError> {
        wincode::deserialize(bytes).map_err(|err| fail(format!("a finality input that does not decode: {err}")))
    }
}

#[cfg(feature = "header-proofs")]
impl HeadersInput {
    #[document(
        parent = "HeadersInput",
        summary = "Builds a header-chain proof's input from a node's database: from the previous proof's justified checkpoint, or genesis, every justified checkpoint after it in turn, each link from the one before, up to the link that finalizes the epoch asked for, at most MAX_HEADER_LINKS of them.",
        params = {
            chain = "A chain of the genesis.",
            db = "The node's database, with history back to the first link's target.",
            genesis = "The chain's genesis.json.",
            program = "The header-chain program's id.",
            previous = "The proof to go on from, as the chain took it, or None from genesis.",
            to = "The epoch to finalize, at most the node's own finalized one.",
        },
        returns = "The input, or why not: an epoch not justified, links that do not follow on, or history that does not reach them.",
    )]
    pub fn build(chain: &Chain, db: Arc<dyn Committed>, genesis: Vec<u8>, program: [u8; 32], previous: Option<TakenHeaders>, to: u64) -> Result<HeadersInput, ReplayError> {
        let start = match &previous {
            Some(previous) => ProvenHeaders::from_public_values(chain.hash(), &previous.values).ok_or_else(|| fail("a previous proof whose public values are no header chain"))?.justified.checkpoint,
            None => Checkpoint { epoch: 0, block: chain.genesis() },
        };

        Ok(HeadersInput { previous, ..HeadersInput::build_from(chain, db, genesis, program, start, to)? })
    }

    #[document(
        parent = "HeadersInput",
        summary = "Builds a header-chain proof's links from a justified checkpoint as HeadersInput::build does, with no previous proof in it: for a prover going on from a proof of its own the chain has not taken, which it puts in as previous before proving.",
        params = { start = "The justified checkpoint the previous proof ends at." },
    )]
    pub fn build_from(chain: &Chain, db: Arc<dyn Committed>, genesis: Vec<u8>, program: [u8; 32], start: Checkpoint, to: u64) -> Result<HeadersInput, ReplayError> {
        let interval = interval(chain)?;
        let previous = None;
        let mut justified = start;
        let mut links = Vec::new();

        for epoch in start.epoch + 1.. {
            if links.len() >= MAX_HEADER_LINKS {
                break;
            }

            let Some(target) = stored_header_at(&*db, epoch * interval)? else {
                break;
            };
            // An epoch never justified has no link of its own: the next one's starts behind it.
            let Some(justification) = crate::chain::stored_justification(&*db, &target.id())? else {
                continue;
            };

            if justification.source != justified {
                return Err(fail(format!("the link to epoch {epoch} starts at epoch {}, not at the justified epoch {}", justification.source.epoch, justified.epoch)));
            }

            links.push(LinkInput::build(chain, db.clone(), epoch)?);
            let finalizes = justification.source.epoch + 1 == epoch;
            justified = justification.target;

            if finalizes && justification.source.epoch >= to {
                break;
            }
        }

        if links.is_empty() {
            return Err(fail(format!("nothing justified after epoch {} yet", start.epoch)));
        }

        // The source's header must be here too, as a check of the database's own chain.
        stored_header(&*db, &links[0].source.block)?.ok_or_else(|| fail("the first link's source is not held here"))?;

        Ok(HeadersInput { genesis, program, previous, links })
    }

    #[document(parent = "HeadersInput", summary = "The input as bytes, as GET /validity/headers-input sends it and the zkVM reads it.")]
    pub fn to_bytes(&self) -> Vec<u8> {
        wincode::serialize(self).expect("a header-chain input always encodes")
    }

    #[document(parent = "HeadersInput", summary = "An input from its bytes.")]
    pub fn from_bytes(bytes: &[u8]) -> Result<HeadersInput, ReplayError> {
        wincode::deserialize(bytes).map_err(|err| fail(format!("a header-chain input that does not decode: {err}")))
    }
}
