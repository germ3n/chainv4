//! Running a range of blocks off the node, over a witness of what it reads: what a zkVM runs as its
//! program, and a node runs first to find out what the witness must hold.
//!
//! **What the witness holds.** The state the range starts from, as the entries the range reads
//! and the nodes of the state tree on their way down and on the way of everything the range
//! changes, each checked against the start's state root before anything runs; or, for a range
//! that scans a column, which no proof of single entries shows complete, every entry the state
//! holds, whose root is worked out from them. Beside it, the headers the range reads, the
//! checkpoints its justifications name, each checked against the start by its parent's id. Nothing
//! else is read: a range that reads anything else fails, so nothing the proof did not check goes
//! into what it proves.

use {
    super::{GatewayMode, ProvenRange, RangeEnd, withdrawals_root},
    crate::chain::{Chain, json::from_json, stored_block_at, stored_header_at},
    crate::crypto::hashable::Hashable,
    crate::db::{Column, Committed, Database, DbError, Historic, MemoryDb, Read, Recording, STATE_COLUMNS, State, StateTree, check_witness, root_of},
    crate::registry::changes::Changes,
    crate::structs::block::{Block, SealedHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::tx::TxCallArgumentType,
    r#macro::document,
    std::collections::{BTreeMap, BTreeSet},
    std::fmt,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "A state-tree node a witness gives: its key in Column::StateTree, and the node as stored there, or None where the position holds none.",
    category = "Proofs",
    see_also = "Witness",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct WitnessNode {
    #[document(summary = "Where the node is stored.")]
    pub key: Vec<u8>,
    #[document(summary = "The node, or None.")]
    pub node: Option<Vec<u8>>,
}

#[document(
    summary = "A chain-state entry a witness gives: its column and key, and its value, or None for one the state does not hold.",
    category = "Proofs",
    see_also = "Witness",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct WitnessEntry {
    #[document(summary = "One of STATE_COLUMNS.")]
    pub column: Column,
    #[document(summary = "Its key.")]
    pub key: Vec<u8>,
    #[document(summary = "Its value, or None.")]
    pub value: Option<Vec<u8>>,
}

#[document(
    summary = "The state a range starts from, as much of it as the range needs, checked against the start's state root.",
    category = "Proofs",
    see_also = "RangeInput, check_witness",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum Witness {
    #[document(summary = "The entries the range reads and the tree's nodes it reads or needs to reach them: for a range that scans no state column.")]
    #[wincode(tag = 0)]
    Partial {
        /// The tree's nodes.
        nodes: Vec<WitnessNode>,
        /// The entries.
        entries: Vec<WitnessEntry>,
    },
    #[document(summary = "Every entry the state holds, each with a value: for a range that scans one.")]
    #[wincode(tag = 1)]
    Whole {
        /// The entries.
        entries: Vec<WitnessEntry>,
    },
}

#[document(
    summary = "Everything a proof of a range runs on: the genesis, the start's header and those of the ancestors the range reads, the blocks, and the witness. A node builds it (RangeInput::build, GET /proofs/input); the prover feeds it to the zkVM, which runs replay on it.",
    category = "Proofs",
    see_also = "replay, Witness, ProvenRange",
)]
#[derive(Clone, Debug, PartialEq, SchemaWrite, SchemaRead)]
pub struct RangeInput {
    #[document(summary = "The chain's genesis.json, as the chain was started from it.")]
    pub genesis: Vec<u8>,
    #[document(summary = "The start's header, then each one's parent, as far back as the range reads.")]
    pub headers: Vec<SealedHeader>,
    #[document(summary = "Every block after the start, through the end, in order.")]
    pub blocks: Vec<Block>,
    #[document(summary = "The start's state, as much as the range reads.")]
    pub witness: Witness,
}

#[document(
    summary = "Why a range could not be built or run: anything it reads that no witness covers, a block that breaks a rule, or a database error.",
    category = "Proofs",
    see_also = "replay",
)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayError(pub String);

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReplayError {}

impl From<DbError> for ReplayError {
    fn from(err: DbError) -> ReplayError {
        ReplayError(err.to_string())
    }
}

fn fail(why: impl fmt::Display) -> ReplayError {
    ReplayError(why.to_string())
}

#[document(
    summary = "What running a range came to: its last block's header, and the digests of what the directory's gateway sent in it, in order.",
    category = "Proofs",
    see_also = "run_range",
)]
pub struct Ran {
    #[document(summary = "The last block's header.")]
    pub end: SealedHeader,
    #[document(summary = "Every transfer's digest the gateway sent, in order.")]
    pub withdrawals: Vec<[u8; 32]>,
    #[document(summary = "Governance's proof settings at the end, if it set any.")]
    pub settings: Option<super::ProofSettings>,
}

#[document(
    summary = "Runs blocks over a state, each on the last as a node runs it (Chain::run_block), checking that each builds on the one before and holds its own roots: what replay runs over a witness, and RangeInput::build over a node's history to record what the witness must hold.",
    params = {
        chain = "A chain of the range's genesis: only its rules and engines are used, never its database.",
        base = "The start's state.",
        start = "The start's header.",
        blocks = "The blocks after it, in order.",
    },
    category = "Proofs",
    see_also = "replay",
)]
pub fn run_range(chain: &Chain, base: Arc<dyn Committed>, start: &SealedHeader, blocks: &[Block]) -> Result<Ran, ReplayError> {
    let hash = chain.hash();
    let state = Arc::new(State::new(base));
    let mut parent = start.clone();
    let mut withdrawals = Vec::new();

    for block in blocks {
        let header = &block.header;

        if header.parent != parent.header.id() || header.height != parent.header.height + 1 {
            return Err(fail(format!("block {} does not build on the one before it", header.height)));
        }

        if header.tx_root != Block::tx_root(hash, &block.txs) || header.proof_root != Block::proof_root(hash, &block.proofs) || header.pool_root != Block::pool_root(hash, block.pool.as_ref()) {
            return Err(fail(format!("block {}'s roots are not its own", header.height)));
        }

        let gateway = Changes::new(&state).directory()?.and_then(|directory| directory.gateway);
        let (layer, receipts) = chain.run_block(&state, &parent.header, block).map_err(|err| fail(format!("block {}: {err}", header.height)))?;

        // What left through the gateway: its Sent events, (nonce, target, digest, sender).
        for event in receipts.iter().flat_map(|receipt| &receipt.events) {
            if Some(event.contract) == gateway
                && event.name == "Sent"
                && let Some(TxCallArgumentType::Bytes(digest)) = event.data.get(2)
                && let Ok(digest) = <[u8; 32]>::try_from(digest.as_slice())
            {
                withdrawals.push(digest);
            }
        }

        state.merge(layer.take_changes());
        // The block itself, for the blocks after it to read as a node keeps it.
        let id = header.id();
        state.put_value(Column::Heights, header.height.to_be_bytes().to_vec(), &id);
        state.put_value(Column::Headers, id.hashable_bytes_with_tag(), &block.sealed_header());
        parent = block.sealed_header();
    }

    let settings = Changes::new(&state).proofs()?;

    Ok(Ran { end: parent, withdrawals, settings })
}

#[document(
    summary = "Runs a range over its witness and says what it proves: what the zkVM program is. The witness is checked against the start's state root, the headers against the start by their parents' ids, and every block run by the chain's rules; the range fails on anything that does not hold, or that it reads beyond them.",
    params = { input = "The range's input." },
    returns = "What a proof of it proves.",
    category = "Proofs",
    see_also = "RangeInput, ProvenRange, run_range",
)]
pub fn replay(input: &RangeInput) -> Result<ProvenRange, ReplayError> {
    let json: serde_json::Value = serde_json::from_slice(&input.genesis).map_err(|err| fail(format!("genesis.json: {err}")))?;
    let genesis = from_json(&json).map_err(|err| fail(format!("genesis.json: {err}")))?;
    let chain = Chain::open(Arc::new(MemoryDb::default()), &genesis).map_err(|err| fail(format!("genesis: {err}")))?;
    let start = input.headers.first().ok_or_else(|| fail("no start header"))?;
    let base = WitnessDb::new(chain.hash(), &start.header.state_root, &input.witness, &input.headers)?;
    let ran = run_range(&chain, Arc::new(base), start, &input.blocks)?;
    let end_of = |sealed: &SealedHeader| RangeEnd { height: sealed.header.height, block: sealed.header.id(), state_root: sealed.header.state_root };

    Ok(ProvenRange {
        chain_id: crate::crypto::ethereum::chain_id(&chain.genesis()),
        genesis: chain.genesis(),
        start: end_of(start),
        end: end_of(&ran.end),
        withdrawals: withdrawals_root(&ran.withdrawals),
        program: ran.settings.map_or([0; 32], |settings| settings.program),
        gateway: ran.settings.map_or(GatewayMode::default(), |settings| settings.gateway),
    })
}

impl RangeInput {
    #[document(parent = "RangeInput", summary = "The input as bytes, as GET /validity/input sends it and the zkVM reads it.")]
    pub fn to_bytes(&self) -> Vec<u8> {
        wincode::serialize(self).expect("a range's input always encodes")
    }

    #[document(parent = "RangeInput", summary = "An input from its bytes.")]
    pub fn from_bytes(bytes: &[u8]) -> Result<RangeInput, ReplayError> {
        wincode::deserialize(bytes).map_err(|err| fail(format!("a range's input that does not decode: {err}")))
    }

    #[document(
        parent = "RangeInput",
        summary = "Builds a range's input from a node's database: runs the range over the state at its start, through history, noting everything it reads, and gives what it read with the tree's nodes that prove it, or the whole state when it scanned.",
        params = {
            chain = "A chain of the range's genesis, whose rules and engines run it: never its database.",
            db = "The node's database, or a snapshot of it, which holds the range's blocks and history back to its start: an archive, or a --state-history window that reaches it.",
            genesis = "The chain's genesis.json.",
            from = "The start's height.",
            to = "The end's height.",
        },
        returns = "The input, or why the range cannot be built: blocks not held, history that does not reach the start, or a read no witness covers.",
    )]
    pub fn build(chain: &Chain, db: Arc<dyn Committed>, genesis: Vec<u8>, from: u64, to: u64) -> Result<RangeInput, ReplayError> {
        let hash = chain.hash();

        if to <= from {
            return Err(fail("a range ends after it starts"));
        }

        let start = stored_header_at(&*db, from)?.ok_or_else(|| fail(format!("no block at {from} on the main chain")))?;

        if crate::chain::stored_state_at(db.clone(), &start.header.id())?.is_none() {
            return Err(fail(format!("history here does not reach the range's start at {from}: build it on an archive, or a node whose --state-history reaches it")));
        }

        let blocks = (from + 1..=to)
            .map(|height| stored_block_at(&*db, height)?.ok_or_else(|| fail(format!("no block body at {height}: pruned, or past the tip"))))
            .collect::<Result<Vec<_>, ReplayError>>()?;
        let historic: Arc<dyn Committed> = Arc::new(Historic::new(db.clone(), from));
        let recording = Arc::new(Recording::new(historic.clone()));
        run_range(chain, recording.clone(), &start, &blocks)?;

        let whole = recording.scanned_columns().iter().any(|column| STATE_COLUMNS.contains(column) || *column == Column::StateTree);
        let reads = recording.reads();

        // Anything read but the state, the tree and the headers is nothing a proof checks.
        if let Some((column, _)) = reads.iter().find(|(column, _)| !STATE_COLUMNS.contains(column) && !matches!(column, Column::StateTree | Column::Heights | Column::Headers)) {
            return Err(fail(format!("the range reads {column:?}, which no witness covers")));
        }

        if let Some(column) = recording.scanned_columns().into_iter().find(|column| !STATE_COLUMNS.contains(column)) {
            return Err(fail(format!("the range scans {column:?}, which no witness covers")));
        }

        let witness = match whole {
            true => {
                let mut entries = Vec::new();

                for column in STATE_COLUMNS {
                    historic.scan(column, &[], &[], &mut |key, value| {
                        entries.push(WitnessEntry { column, key: key.to_vec(), value: Some(value.to_vec()) });

                        true
                    })?;
                }

                Witness::Whole { entries }
            }
            false => {
                // The way down to every entry read, through the recording, so its nodes are read.
                let walk = State::new(recording.clone());

                for (column, key) in reads.iter().filter(|(column, _)| STATE_COLUMNS.contains(column)) {
                    StateTree::new(&walk, hash).prove(*column, key)?;
                }

                // And the nodes above every node read, which a node the range changed first may
                // have kept the range from reading here.
                for (_, key) in reads.iter().filter(|(column, _)| *column == Column::StateTree) {
                    for above in crate::db::ancestor_keys(key) {
                        recording.read(Column::StateTree, &above)?;
                    }
                }

                let mut nodes = Vec::new();
                let mut entries = Vec::new();

                for (column, key) in recording.reads() {
                    match column {
                        Column::StateTree => nodes.push(WitnessNode { node: historic.read(column, &key)?, key }),
                        column if STATE_COLUMNS.contains(&column) => entries.push(WitnessEntry { value: historic.read(column, &key)?, column, key }),
                        _ => {}
                    }
                }

                Witness::Partial { nodes, entries }
            }
        };

        // The headers it read: the ancestors from the start down to the lowest of them.
        let mut lowest = from;

        for (column, key) in &reads {
            match column {
                Column::Heights => lowest = lowest.min(u64::from_be_bytes(key.as_slice().try_into().map_err(|_| fail("a height key of other than 8 bytes"))?)),
                Column::Headers => {
                    let sealed = db.read(Column::Headers, key)?.and_then(|bytes| wincode::deserialize::<SealedHeader>(&bytes).ok()).ok_or_else(|| fail("the range reads a header this node does not hold"))?;
                    lowest = lowest.min(sealed.header.height);
                }
                _ => {}
            }
        }

        let mut headers = vec![start];

        while let Some(last) = headers.last()
            && last.header.height > lowest
        {
            let parent = crate::chain::stored_header(&*db, &last.header.parent)?.ok_or_else(|| fail(format!("no header below {}", last.header.height)))?;
            headers.push(parent);
        }

        Ok(RangeInput { genesis, headers, blocks, witness })
    }
}

/// The start's state as a witness gives it, checked, and its headers: what replay runs a range
/// over. Anything else it is asked for is an error, as nothing else was checked.
struct WitnessDb {
    whole: bool,
    db: MemoryDb,
    known: BTreeSet<(Column, Vec<u8>)>,
    heights: BTreeMap<Vec<u8>, Vec<u8>>,
    headers: BTreeMap<Vec<u8>, Vec<u8>>,
}

impl WitnessDb {
    fn new(hash: HashType, root: &Hash, witness: &Witness, headers: &[SealedHeader]) -> Result<WitnessDb, ReplayError> {
        let db = MemoryDb::default();
        let state = State::new(Arc::new(MemoryDb::default()));
        let mut known = BTreeSet::new();

        let whole = match witness {
            Witness::Partial { nodes, entries } => {
                let node_map: BTreeMap<Vec<u8>, Option<Vec<u8>>> = nodes.iter().map(|node| (node.key.clone(), node.node.clone())).collect();
                let entry_map: BTreeMap<(Column, Vec<u8>), Option<Vec<u8>>> = entries.iter().map(|entry| ((entry.column, entry.key.clone()), entry.value.clone())).collect();

                if entries.iter().any(|entry| !STATE_COLUMNS.contains(&entry.column)) {
                    return Err(fail("a witness entry outside the state"));
                }

                check_witness(hash, root, &node_map, &entry_map).map_err(|why| fail(format!("the witness does not hold: {why}")))?;

                for (key, node) in node_map {
                    if let Some(node) = node {
                        state.put(Column::StateTree, key.clone(), node);
                    }

                    known.insert((Column::StateTree, key));
                }

                for ((column, key), value) in entry_map {
                    if let Some(value) = value {
                        state.put(column, key.clone(), value);
                    }

                    known.insert((column, key));
                }

                false
            }
            Witness::Whole { entries } => {
                if entries.iter().any(|entry| !STATE_COLUMNS.contains(&entry.column) || entry.value.is_none()) {
                    return Err(fail("a whole witness's entries are the state's, each with a value"));
                }

                let all = entries.iter().map(|entry| (entry.column, entry.key.as_slice(), entry.value.as_deref().unwrap_or_default()));

                if root_of(hash, all) != *root {
                    return Err(fail("the whole witness is not the start's state"));
                }

                for entry in entries {
                    state.put(entry.column, entry.key.clone(), entry.value.clone().unwrap_or_default());
                }

                // Its tree, built as genesis builds one, so the range's changes follow it.
                StateTree::new(&state, hash).follow()?;

                true
            }
        };
        db.commit(state.take_changes())?;

        // Each header checked against the one after it, from the start down.
        for pair in headers.windows(2) {
            if pair[0].header.parent != pair[1].header.id() || pair[1].header.height + 1 != pair[0].header.height {
                return Err(fail(format!("the header at {} is not its child's parent", pair[1].header.height)));
            }
        }

        let mut heights = BTreeMap::new();
        let mut by_id = BTreeMap::new();

        for sealed in headers {
            let id = sealed.header.id();
            heights.insert(sealed.header.height.to_be_bytes().to_vec(), id.hashable_bytes_with_tag());
            by_id.insert(id.hashable_bytes_with_tag(), sealed.hashable_bytes_with_tag());
        }

        Ok(WitnessDb { whole, db, known, heights, headers: by_id })
    }
}

impl Read for WitnessDb {
    fn read(&self, column: Column, key: &[u8]) -> Result<Option<Vec<u8>>, DbError> {
        match column {
            Column::Heights => Ok(self.heights.get(key).cloned()),
            Column::Headers => Ok(self.headers.get(key).cloned()),
            column if STATE_COLUMNS.contains(&column) || column == Column::StateTree => match self.whole || self.known.contains(&(column, key.to_vec())) {
                true => self.db.read(column, key),
                false => Err(DbError::Backend(format!("the witness holds no {column:?} entry at {}", crate::structs::hex::to_hex(key)))),
            },
            column => Err(DbError::Backend(format!("a range reads {column:?}, which no witness covers"))),
        }
    }
}

impl Committed for WitnessDb {
    fn scan(&self, column: Column, prefix: &[u8], start: &[u8], visit: &mut dyn FnMut(&[u8], &[u8]) -> bool) -> Result<(), DbError> {
        match self.whole && (STATE_COLUMNS.contains(&column) || column == Column::StateTree) {
            true => self.db.scan(column, prefix, start, visit),
            false => Err(DbError::Backend(format!("a range scans {column:?}, which only a whole witness covers"))),
        }
    }
}
