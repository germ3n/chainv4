//! A range of main-chain blocks for GET /blocks/range: each with what recomputes its header's roots,
//! its receipts, and proofs at its state_root of each state entry it changed. light::range holds what
//! is sent, and what a client checks of it.
//!
//! What a block changed is already kept: the list of keys it wrote that its history entries are
//! pruned by, one per height. That list lives exactly as long as the state it is any use against,
//! is taken back with its block in a reorg, and is there on nodes that ran blocks before this route
//! existed, so nothing here writes a thing. It also names the state tree's nodes, which are left out.
//!
//! Proofs are worked out as they are asked for, so what one answer may cost is bounded here: blocks,
//! bytes, proofs and time. Past any of them it stops, and says where to go on from: if need be from
//! inside a block's entries, so even the largest block's proofs arrive within the bounds, with only
//! its body and receipts, which cannot be split, past them.

use {
    super::{HISTORY_FROM_KEY, HistoryFrom, HistoryKey, HistoryList, TIP_KEY, read_value, stored_block_at, stored_header, stored_header_at, stored_receipt, stored_state_at},
    crate::db::{Column, Committed, DbError, STATE_COLUMNS, StateTree, history_list_key},
    crate::light::{BlockRange, NoProofs, ProvenEntry, RangeBody, RangeChanges, RangeCursor, RangeItem},
    crate::structs::block::{Block, SealedHeader},
    crate::structs::hash::{Hash, HashType},
    crate::structs::receipt::CommittedReceipt,
    r#macro::document,
    std::sync::Arc,
    std::time::{Duration, Instant},
};

#[document(
    summary = "What one answer of a block range may cost a node. Past any bound it stops and says where to go on from, but always gives the first block's body and receipts, and at least one of its entries, so a client always gets further.",
    category = "Chain",
    see_also = "stored_block_range, BlockRange",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RangeLimits {
    #[document(summary = "The most blocks.")]
    pub blocks: u64,
    #[document(summary = "The most bytes, encoded.")]
    pub bytes: u64,
    #[document(summary = "The most state entries proven: what its CPU and disk go on.")]
    pub entries: u32,
    #[document(summary = "How long it may spend, past which it stops wherever it has got to: what bounds a slow disk.")]
    pub time: Duration,
}

impl RangeLimits {
    #[document(
        parent = "RangeLimits",
        summary = "What GET /blocks/range answers within: 100 blocks, 8 MiB, 4,096 entries and a second. 8 MiB holds a full block and its first proofs; 4,096 proofs take about 70 ms on a state of 60,000 accounts held in cache, and the second is for a disk that holds it up.",
    )]
    pub const SERVED: RangeLimits = RangeLimits {
        blocks: 100,
        bytes: 8 * 1024 * 1024,
        entries: 4_096,
        time: Duration::from_secs(1),
    };
}

#[document(
    summary = "Why a block range cannot be answered at all.",
    category = "Chain",
    see_also = "stored_block_range",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeProblem {
    #[document(summary = "The node holds nothing at that height, not even a header: it started at a checkpoint above it.")]
    NotHeld,
    #[document(summary = "The block's list holds fewer entries than asked to go on from: a reorg since the answer before.")]
    NoSuchEntry,
}

impl std::fmt::Display for RangeProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RangeProblem::NotHeld => write!(f, "this node holds no block or header that low"),
            RangeProblem::NoSuchEntry => write!(f, "the block has no entry that far: has the chain moved to another branch?"),
        }
    }
}

/// How far an answer has got against its limits.
struct Budget<'a> {
    limits: &'a RangeLimits,
    started: Instant,
    bytes: u64,
    entries: u32,
}

impl Budget<'_> {
    /// Whether this many more bytes, and an entry if it is one, fit.
    fn fits(&self, bytes: u64, entry: bool) -> bool {
        self.bytes.saturating_add(bytes) <= self.limits.bytes
            && (!entry || self.entries < self.limits.entries)
            && self.started.elapsed() < self.limits.time
    }
}

/// What a value takes, encoded, or a database error for what will not encode: a limit passed by what
/// is stored, which fails this one answer, never the thread.
fn encoded<T: wincode::SchemaWrite<wincode::config::DefaultConfig, Src = T>>(value: &T) -> Result<u64, DbError> {
    wincode::serialized_size(value).map_err(|err| DbError::Corrupt(format!("a block range: {err}")))
}

#[document(
    summary = "Main-chain blocks from a height on, with what checks them, read straight from a database within limits. For one answer, give it a snapshot, so the whole answer is of one commit.",
    params = {
        db = "A snapshot of the node's database.",
        hash = "The chain's hash.",
        from = "The first height.",
        entry = "How many of that block's entries came already: 0 unless going on from a cursor. Above 0 the block's body is not sent again.",
        count = "How many blocks at most: at least 1 is taken, at most the limits'.",
        limits = "What the answer may cost.",
    },
    returns = "The range, empty for a height past the tip; or why it cannot be answered.",
    category = "Chain",
    see_also = "BlockRange, RangeLimits, check_range_item",
)]
pub fn stored_block_range(db: Arc<dyn Committed>, hash: HashType, from: u64, entry: u32, count: u64, limits: &RangeLimits) -> Result<Result<BlockRange, RangeProblem>, DbError> {
    let Some(tip) = read_value::<Hash>(&*db, Column::Meta, TIP_KEY)?.map(|tip| stored_header(&*db, &tip)).transpose()?.flatten().map(|sealed| sealed.header.height) else {
        return Err(DbError::Corrupt("no tip".to_string()));
    };
    let kept_from = read_value::<HistoryFrom>(&*db, Column::Meta, HISTORY_FROM_KEY)?.map_or(0, |from| from.height);
    let wanted = count.clamp(1, limits.blocks.max(1));
    let mut range = BlockRange { tip, items: Vec::new(), next: None };
    let mut budget = Budget {
        limits,
        started: Instant::now(),
        bytes: encoded(&range)? + encoded(&Some(RangeCursor { height: 0, entry: 0 }))?,
        entries: 0,
    };
    let mut height = from;
    let mut going_on = entry;

    while height <= tip {
        if range.items.len() as u64 == wanted {
            range.next = Some(RangeCursor { height, entry: 0 });
            break;
        }

        let Some(sealed) = stored_header_at(&*db, height)? else {
            return Ok(Err(RangeProblem::NotHeld));
        };
        let first = range.items.is_empty();
        let body = body_at(&*db, hash, &sealed, going_on)?;
        let holding = encoded(&RangeItem { sealed: sealed.clone(), body: RangeBody::Continued, changes: RangeChanges::Proven { from: 0, listed: 0, entries: Vec::new() } })? + encoded(&body)?;

        // Past the first, a block that does not fit waits for the next answer.
        if !first && !budget.fits(holding, false) {
            range.next = Some(RangeCursor { height, entry: 0 });
            break;
        }

        budget.bytes = budget.bytes.saturating_add(holding);
        let Some((changes, stopped)) = changes_at(&db, hash, &sealed, kept_from, going_on, first, &mut budget)? else {
            return Ok(Err(RangeProblem::NoSuchEntry));
        };

        // Past the first, a block none of whose entries fit waits too, so its body is not sent twice.
        if !first && stopped == Some(0) {
            range.next = Some(RangeCursor { height, entry: 0 });
            break;
        }

        range.items.push(RangeItem { sealed, body, changes });

        if let Some(entry) = stopped {
            range.next = Some(RangeCursor { height, entry });
            break;
        }

        going_on = 0;
        height = match height.checked_add(1) {
            Some(height) => height,
            None => break,
        };
    }

    Ok(Ok(range))
}

/// What an item holds of a block's body: nothing when going on through its entries, or below where
/// a snapshot node took state.
fn body_at(db: &dyn Committed, hash: HashType, sealed: &SealedHeader, going_on: u32) -> Result<RangeBody, DbError> {
    if going_on > 0 {
        return Ok(RangeBody::Continued);
    }

    let Some(Block { txs, proofs, pool, .. }) = stored_block_at(db, sealed.header.height)? else {
        return Ok(RangeBody::HeaderOnly);
    };
    let id = sealed.id();
    let mut receipts: Option<Vec<CommittedReceipt>> = Some(Vec::with_capacity(txs.len()));

    for tx in &txs {
        let tx = tx.id(hash);

        // The block a snapshot node took state at never ran here, so it has no receipts: all or
        // none, as a client can check only all.
        match stored_receipt(db, &tx)?.filter(|receipt| receipt.block == id) {
            Some(receipt) => {
                if let Some(receipts) = &mut receipts {
                    receipts.push(receipt.committed(tx));
                }
            }
            None => {
                receipts = None;
                break;
            }
        }
    }

    Ok(RangeBody::Whole { txs, proofs, pool, receipts })
}

/// The entries of a block that fit, from where the answer before stopped, and where they stop short
/// of the list if they do. None when the list is shorter than that. Whatever the budget says, the
/// first item of an answer gets one entry, so each answer gets further.
fn changes_at(db: &Arc<dyn Committed>, hash: HashType, sealed: &SealedHeader, kept_from: u64, going_on: u32, first: bool, budget: &mut Budget<'_>) -> Result<Option<(RangeChanges, Option<u32>)>, DbError> {
    let height = sealed.header.height;

    if height < kept_from {
        return Ok(Some((RangeChanges::None(NoProofs::StatePruned { kept_from }), None)));
    }

    let Some(list) = read_value::<HistoryList>(&**db, Column::History, &history_list_key(height))? else {
        return Ok(Some((RangeChanges::None(NoProofs::NoList), None)));
    };
    let keys: Vec<HistoryKey> = list.keys.into_iter().filter(|key| STATE_COLUMNS.contains(&key.column)).collect();
    let count = keys.len() as u32;

    if going_on > 0 && going_on >= count {
        return Ok(None);
    }

    let Some(state) = stored_state_at(db.clone(), &sealed.id())? else {
        return Ok(Some((RangeChanges::None(NoProofs::StatePruned { kept_from }), None)));
    };
    let tree = StateTree::new(&state, hash);
    let mut entries = Vec::new();
    let mut stopped = None;

    for (at, HistoryKey { column, key }) in keys.into_iter().enumerate().skip(going_on as usize) {
        if !(first && entries.is_empty()) && !budget.fits(0, true) {
            stopped = Some(at as u32);
            break;
        }

        let value = state.get(column, &key)?;
        let proof = tree.prove(column, &key)?;
        let entry = ProvenEntry { column, key, value, proof };
        let size = encoded(&entry)?;

        if !(first && entries.is_empty()) && !budget.fits(size, true) {
            stopped = Some(at as u32);
            break;
        }

        budget.bytes = budget.bytes.saturating_add(size);
        budget.entries = budget.entries.saturating_add(1);
        entries.push(entry);
    }

    Ok(Some((RangeChanges::Proven { from: going_on, listed: count, entries }, stopped)))
}
