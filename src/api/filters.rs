//! What GET /accounts/{address}/txs narrows its listing by: a transaction's kind, whether the
//! address sent it, who else it touched, which token it moved, and when it landed. None of it is
//! stored: the address index is by height, so a height range is a seek and a stop, and everything
//! else is tested against the block and receipt an entry points to, as the listing reads them
//! anyway. That is why a request looks at no more than MAX_TXS_SCANNED entries.

use {
    super::{ApiError, parse_address, tip},
    crate::chain::{TxReceipt, stored_header_at, touched},
    crate::db::Committed,
    crate::structs::address::Address,
    crate::structs::tx::{SignedTx, Tx},
    crate::structs::u256::u256,
    std::collections::HashMap,
};

/// The most index entries one request looks at, matches or not, unless --tune filter.scan says
/// otherwise: a query that matches rarely comes back with fewer than its limit and a cursor, rather
/// than tying up the node.
pub(super) const MAX_TXS_SCANNED: usize = 1_000;

/// How many blocks past a time range a scan looks: a chain's timestamps need only pass the median
/// of a window before them, so a block's time may run a little behind its height.
const TIME_SLACK: u64 = 256;

/// Whether an address sent a transaction, or only took part in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    In,
    Out,
}

/// What a listing is to have moved.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Moved {
    Coin,
    Token(Address),
}

/// The filters a request asked for; nothing set passes everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct TxFilter {
    kind: Option<String>,
    direction: Option<Direction>,
    counterparty: Option<Address>,
    token: Option<Moved>,
    /// Block times, in milliseconds since the Unix epoch, both ends included.
    from: Option<u64>,
    until: Option<u64>,
    /// Block heights, both ends included.
    from_height: Option<u64>,
    until_height: Option<u64>,
}

impl TxFilter {
    /// Reads a request's parameters: one left empty is one not given, as a form sends them.
    pub(super) fn parse(query: &HashMap<String, String>) -> Result<TxFilter, ApiError> {
        let given = |name: &str| query.get(name).map(|text| text.trim()).filter(|text| !text.is_empty());
        let time = |name: &str| given(name).map(|text| text.parse::<u64>().map_err(|_| ApiError::BadRequest(format!("{name} must be a whole number")))).transpose();
        let direction = match given("direction") {
            None => None,
            Some("in") => Some(Direction::In),
            Some("out") => Some(Direction::Out),
            Some(_) => return Err(ApiError::BadRequest("direction must be in or out".to_string())),
        };
        let token = match given("token") {
            None => None,
            Some("coin") => Some(Moved::Coin),
            Some(token) => Some(Moved::Token(parse_address(token)?)),
        };

        Ok(TxFilter {
            kind: given("kind").map(str::to_string),
            direction,
            counterparty: given("counterparty").map(parse_address).transpose()?,
            token,
            from: time("from")?,
            until: time("until")?,
            from_height: time("from_height")?,
            until_height: time("until_height")?,
        })
    }

    /// The names of the filters set: what an answer says it went by, which an older node's leaves
    /// out, so an app can tell it was not filtered.
    pub(super) fn applied(&self) -> Vec<&'static str> {
        [
            (self.kind.is_some(), "kind"),
            (self.direction.is_some(), "direction"),
            (self.counterparty.is_some(), "counterparty"),
            (self.token.is_some(), "token"),
            (self.from.is_some(), "from"),
            (self.until.is_some(), "until"),
            (self.from_height.is_some(), "from_height"),
            (self.until_height.is_some(), "until_height"),
        ]
        .into_iter()
        .filter_map(|(set, name)| set.then_some(name))
        .collect()
    }

    /// The lowest height a scan need reach, and the highest it need start from, if any: the
    /// heights asked for, and those the times asked for come to, each with some slack.
    pub(super) fn heights(&self, db: &dyn Committed) -> Result<(u64, Option<u64>), ApiError> {
        let lowest = match self.from {
            Some(from) => first_at_or_after(db, from)?.saturating_sub(TIME_SLACK).max(self.from_height.unwrap_or(0)),
            None => self.from_height.unwrap_or(0),
        };
        let highest = match self.until {
            Some(until) => {
                let by_time = first_at_or_after(db, until.saturating_add(1))?.saturating_add(TIME_SLACK);

                Some(self.until_height.map_or(by_time, |height| height.min(by_time)))
            }
            None => self.until_height,
        };

        Ok((lowest, highest))
    }

    /// Whether a transaction touching an address passes, with what its block and receipt say.
    pub(super) fn keeps(&self, address: &Address, signed: &SignedTx, receipt: &TxReceipt, time: u64) -> bool {
        let inner = match &signed.tx {
            Tx::Multisig { tx, .. } => tx,
            tx => tx,
        };
        // A batch passes for what any of its steps does, and for being a batch.
        let steps = signed.tx.what_it_runs();

        self.from.is_none_or(|from| time >= from)
            && self.until.is_none_or(|until| time <= until)
            && self.kind.as_ref().is_none_or(|kind| kind_of(&signed.tx) == *kind || kind_of(inner) == *kind || steps.iter().any(|step| kind_of(step) == *kind))
            && self.direction.is_none_or(|direction| goes(direction, address, signed, steps))
            && self.counterparty.is_none_or(|party| touched(signed, &receipt.status, &receipt.payments).contains(&party))
            && self.token.as_ref().is_none_or(|token| moves(token, steps, receipt))
    }
}

/// The name a transaction's kind goes by in the API, as its "type".
fn kind_of(tx: &Tx) -> String {
    crate::structs::json::tx(tx)["type"].as_str().unwrap_or_default().to_string()
}

/// Whether a transaction goes that way for an address: out when it sent it, in otherwise, and both
/// for coin it paid itself.
fn goes(direction: Direction, address: &Address, signed: &SignedTx, steps: &[Tx]) -> bool {
    let sent = signed.sender() == *address;
    let to_itself = steps.iter().any(|step| matches!(step, Tx::Transfer { from, to, .. } if from == to));

    match direction {
        Direction::Out => sent,
        Direction::In => !sent || to_itself,
    }
}

/// Whether a transaction moved the coin, or called a token or had it emit something.
fn moves(token: &Moved, steps: &[Tx], receipt: &TxReceipt) -> bool {
    match token {
        Moved::Coin => {
            !receipt.payments.is_empty()
                || steps.iter().any(|step| match step {
                    Tx::Transfer { .. } | Tx::Mint { .. } | Tx::Burn { .. } | Tx::Claim { .. } => true,
                    Tx::ContractCall { amount, .. } => amount.to_u256() != u256::ZERO,
                    Tx::EvmCall { value, .. } => value.to_u256() != u256::ZERO,
                    _ => false,
                })
        }
        Moved::Token(token) => {
            let called = steps.iter().any(|step| match step {
                Tx::ContractCall { to, .. } | Tx::EvmCall { to, .. } => to == token,
                _ => false,
            });

            called || receipt.events.iter().any(|event| event.contract == *token)
        }
    }
}

/// The first main-chain height whose block was made at this time or later, by halving: one past the
/// tip when none was. A height with no header kept, below a snapshot's pivot, counts as older.
fn first_at_or_after(db: &dyn Committed, time: u64) -> Result<u64, ApiError> {
    let (mut low, mut high) = (0, tip(db)?.header.height + 1);

    while low < high {
        let middle = low + (high - low) / 2;

        match stored_header_at(db, middle)? {
            Some(sealed) if sealed.header.timestamp >= time => high = middle,
            _ => low = middle + 1,
        }
    }

    Ok(low)
}
