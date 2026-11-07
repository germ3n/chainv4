//! What the history shows: every transaction an account sent, or that touched it, described from
//! what the light client proved of it. Each is kept once proven, so it shows again without asking:
//! a light client restored from storage keeps headers only from the finalized block on, and could
//! not prove an older one again.
//!
//! What it says of EVM calls and logs is read when it shows, as an ABI may be saved afterwards, so
//! their bytes are kept, as far as they are short. Everything added to what is kept here must read
//! back from what older wallets wrote: settings that do not decode lose everything in them.

use {
    crate::chain::{Landed, Proven},
    crate::format,
    chainv4::crypto::hasher::Hasher,
    chainv4::governance::{Change, ChangeKind},
    chainv4::structs::address::Address,
    chainv4::abi::known::token_moves,
    chainv4::structs::event::Event,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::hex::to_hex,
    chainv4::structs::receipt::CommittedOutcome,
    chainv4::registry::names::NameAction,
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    serde::{Deserialize, Serialize},
};

/// Functions a call may name, so the history says which one ran; a call to any other says only
/// whom it called.
pub(crate) const KNOWN: [&str; 10] = ["transfer", "transfer_from", "approve", "mint", "burn", "sign", "tip", "ping", "greet", "upgrade"];

/// The most of a transaction's calldata, and of all its logs, that is kept, in hex digits. A log
/// takes a few hundred at most in anything common. Past this, what is kept says how long it was; a
/// node serving a block's worth of one log must not fill the wallet's settings with it.
const MAX_KEPT: usize = 16 * 1024;

/// Value that moved: the chain's coin, or a token by the standard's Transfer event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Move {
    /// The token contract, or None for the coin.
    pub token: Option<String>,
    /// Who paid, or None for a mint.
    pub from: Option<String>,
    /// Who got it, or None for a burn.
    pub to: Option<String>,
    /// Whole units, as decimal digits.
    pub amount: String,
    /// Which token of its collection, for an NFT or a multi-token's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Which standard's shape its EVM log fit: an EVM move's parties are 20 bytes, 0x…, and any
    /// contract can emit one of these, so what the move says is only a claim, until the registry
    /// lists the contract. It never goes into a balance: those are what the light client proves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
}

impl Move {
    /// Whether this is only what a contract's log claims, by whether the registry lists it.
    pub fn claimed(&self, listed: bool) -> bool {
        self.standard.is_some() && !listed
    }
}

/// An EVM call's bytes, with whom it called: what Activity reads, by the ABIs it holds by then.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptCall {
    /// The contract, as the chain writes it: evm:0x….
    pub to: String,
    /// The calldata, in hex.
    pub input: String,
}

/// An EVM log's bytes, with which contract emitted it: what the events list holds in a few words
/// is all that shows of one too long to keep.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptLog {
    /// Its place among the transaction's events.
    pub position: u32,
    /// Its contract, as the chain writes it: evm:0x….
    pub contract: String,
    /// Its topics and its data, in hex.
    pub topics: Vec<String>,
    pub data: String,
}

/// A transaction, as proven: enough to show it without proving it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checked {
    pub height: u64,
    /// When its block was made, in milliseconds since the Unix epoch; 0 when unknown.
    pub time: u64,
    pub final_: bool,
    /// Whether it did what it asked: a failed one moved nothing but its fee.
    pub done: bool,
    pub sender: String,
    pub nonce: u64,
    /// What it asked for, in a few words: Transfer, Called sign on contract:0x…, Deployed ….
    pub action: String,
    /// Its kind by the name the API gives it, and what a multisig sent as it. Empty for one kept
    /// before it was said, which the Activity page's kind filter lets by.
    #[serde(default)]
    pub kinds: Vec<String>,
    pub moves: Vec<Move>,
    /// What the sender paid, in whole units of the coin.
    pub fee: String,
    /// What it emitted, described: Signed ed25519:0x3b…f2c0 "gm" 3.
    pub events: Vec<String>,
    /// The chain it is on, by its genesis id: a node on a new chain of the same keys has its own
    /// heights. Empty for one kept before it was said, until the chain's node mentions it.
    #[serde(default)]
    pub genesis: String,
    /// What it did in governance, for one that approved a change or withdrew an approval, its own
    /// or a multisig's: the Governance page lists the account's votes by it.
    #[serde(default)]
    pub vote: Option<Vote>,
    /// The EVM call it made, if short enough to keep; none for one kept before they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<KeptCall>,
    /// The EVM logs it emitted, as far as they are short enough to keep; none for one kept before
    /// they were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<KeptLog>,
    /// What each step did, for a batch; none for anything else, or one kept before batches were.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<CheckedStep>,
    /// How it came to be known. Never written: it says which ones are private to a wallet's index, and
    /// so stay out of the settings file.
    #[serde(skip)]
    pub came: Origin,
}

/// Transactions as the settings file keeps them: without those private to a wallet's index, or not
/// meant to last.
pub fn kept_only<S: serde::Serializer>(checked: &std::collections::HashMap<String, Checked>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(checked.iter().filter(|(_, checked)| checked.came == Origin::Proven))
}

/// How a transaction shown came to be known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// Proven against a receipt a node sent, as ever: kept with the settings.
    #[default]
    Proven,
    /// Looked up by the wallet that sent it, reading checked blocks: the block's in, and it counts as
    /// not final until the reading reaches it. Kept nowhere.
    Looked,
    /// Read from a checked block: kept in the wallet's sealed index alone.
    Read,
}

/// One step of a proven batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckedStep {
    /// What it asked for, in a few words, as a whole transaction's says it.
    pub action: String,
    pub result: StepResult,
    /// The EVM call it made, if short enough to keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<KeptCall>,
}

/// What came of one step of a batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepResult {
    Done,
    /// It ran, then a later step failed, which undid it.
    Undone,
    /// It stopped the batch.
    Failed,
    /// One before it stopped the batch.
    NotRun,
}

/// A governance transaction, as proven. Each kind of change has a ballot and a version of its own;
/// a record kept before votes named their kind reads as the consensus's, and one kept before votes
/// could be No, an approval, as a Yes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vote {
    /// A vote on a change, at the version of its kind it named: the change's JSON, yes, no or
    /// abstain, and for the vote that decided it, the decision it was, the chain's count, and the
    /// version of its kind it made; for the vote that rejected it, the chain's count of rejections.
    #[serde(alias = "Approved")]
    Voted {
        version: u64,
        change: String,
        #[serde(default = "yes")]
        vote: String,
        decided: Option<u64>,
        #[serde(default)]
        rejected: Option<u64>,
        #[serde(default = "consensus", with = "kind_name")]
        kind: ChangeKind,
        #[serde(default)]
        kind_version: Option<u64>,
    },
    /// A vote on a change of a kind cleared, at the kind's version it named: the change's JSON, or
    /// none for one kept when a withdrawal named its kind alone.
    #[serde(alias = "Withdrew")]
    Cleared {
        #[serde(default = "consensus", with = "kind_name")]
        kind: ChangeKind,
        version: u64,
        #[serde(default)]
        change: Option<String>,
    },
}

/// What a vote kept before votes named their kind was of.
fn consensus() -> ChangeKind {
    ChangeKind::Consensus
}

/// What a vote kept before votes could be No was: an approval.
fn yes() -> String {
    "yes".to_string()
}

/// A kind of change kept by its name, as the API writes it.
mod kind_name {
    use {chainv4::governance::ChangeKind, serde::{Deserialize, Deserializer, Serializer}};

    pub fn serialize<S: Serializer>(kind: &ChangeKind, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(kind.name())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<ChangeKind, D::Error> {
        String::deserialize(deserializer)?.parse().map_err(serde::de::Error::custom)
    }
}

impl Vote {
    /// The vote a transaction is, the one a multisig sent for it included, with what its receipt
    /// proved: None for anything else.
    pub fn of(tx: &Tx, outcome: &CommittedOutcome) -> Option<Vote> {
        match tx {
            Tx::Multisig { tx, .. } => Vote::of(tx, outcome),
            Tx::VoteChange { version, change, vote } => {
                let decided = match outcome {
                    CommittedOutcome::Decided { decision, version, .. } => Some((*decision, *version)),
                    _ => None,
                };

                Some(Vote::Voted {
                    version: *version,
                    change: chainv4::params::json::change_json(change).to_string(),
                    vote: vote.name().to_string(),
                    decided: decided.map(|(decision, _)| decision),
                    rejected: match outcome {
                        CommittedOutcome::Rejected { rejection, .. } => Some(*rejection),
                        _ => None,
                    },
                    kind: change.kind(),
                    kind_version: decided.map(|(_, made)| made),
                })
            }
            Tx::ClearVote { version, change } => Some(Vote::Cleared { kind: change.kind(), version: *version, change: Some(chainv4::params::json::change_json(change).to_string()) }),
            _ => None,
        }
    }
}

/// A vote in a word: Yes, No or Abstain.
pub fn voted(vote: chainv4::governance::ChangeVote) -> &'static str {
    match vote {
        chainv4::governance::ChangeVote::Yes => "Yes",
        chainv4::governance::ChangeVote::No => "No",
        chainv4::governance::ChangeVote::Abstain => "Abstain",
    }
}

/// A kind of change in words: name prices, not name_prices.
pub fn kind_words(kind: ChangeKind) -> String {
    kind.name().replace('_', " ")
}

impl Checked {
    /// Whether it may be taken as what became of a transaction on this chain. A transaction signed
    /// now has an id of its chain's alone, as its signature names the chain, but rows on a device may
    /// be from before that, when the same keys sending the same thing on a new chain made the same id,
    /// which was proven there afresh. One kept before its chain was said goes with any, until a node
    /// mentions it.
    pub fn on(&self, genesis: Option<&str>) -> bool {
        self.genesis.is_empty() || Some(self.genesis.as_str()) == genesis
    }

    pub fn of(proven: &Proven<Landed>) -> Checked {
        let Landed { signed, receipt, time } = &proven.value;
        let done = !receipt.outcome.failed();
        let sender = signed.sender().to_string();
        let mut moves = Vec::new();

        if done {
            let coin = |from: Option<String>, to: Option<String>, amount: String| Move { token: None, from, to, amount, id: None, standard: None };

            // A multisig's own transaction moves what it does, and a batch what its steps do.
            for tx in signed.tx.what_it_runs() {
                match tx {
                    Tx::Transfer { from, to, amount } => moves.push(coin(Some(from.to_string()), Some(to.to_string()), amount.to_u256().to_string())),
                    Tx::Mint { to, amount } => moves.push(coin(None, Some(to.to_string()), amount.to_u256().to_string())),
                    Tx::Burn { from, amount } => moves.push(coin(Some(from.to_string()), None, amount.to_u256().to_string())),
                    Tx::ContractCall { to, amount, .. } if amount.to_u256() != u256::ZERO => {
                        moves.push(coin(Some(sender.clone()), Some(to.to_string()), amount.to_u256().to_string()));
                    }
                    _ => {}
                }
            }

            // What contracts paid out, proven with the receipt, then tokens by their events.
            moves.extend(receipt.payments.iter().map(|paid| coin(Some(paid.from.to_string()), Some(paid.to.to_string()), paid.amount.to_string())));
            moves.extend(receipt.events.iter().filter_map(token_move));
            moves.extend(receipt.events.iter().flat_map(evm_token_moves));
        }

        Checked {
            height: proven.assurance.height,
            time: *time,
            final_: proven.assurance.final_,
            done,
            nonce: signed.nonce,
            action: action(&signed.tx, &receipt.outcome),
            kinds: kinds_of(&signed.tx),
            moves,
            fee: receipt.fee.to_string(),
            events: receipt.events.iter().map(describe).collect(),
            sender,
            genesis: String::new(),
            vote: Vote::of(&signed.tx, &receipt.outcome),
            call: kept_call(&signed.tx),
            logs: kept_logs(&receipt.events),
            steps: checked_steps(&signed.tx, &receipt.outcome),
            came: Origin::default(),
        }
    }

    /// What it moved for an account of these addresses: each amount with whether it came in.
    pub fn for_account<'a>(&'a self, addresses: &'a [String]) -> impl Iterator<Item = (bool, &'a Move)> + 'a {
        let ours = move |party: &Option<String>| party.as_ref().is_some_and(|party| addresses.contains(party));

        self.moves.iter().filter_map(move |moved| match (ours(&moved.from), ours(&moved.to)) {
            (false, true) => Some((true, moved)),
            (true, false) => Some((false, moved)),
            // Between two of its addresses, or none: nothing came in or left, as far as it is
            // concerned. What moved between them is between's.
            _ => None,
        })
    }

    /// What it moved from one of these addresses to another: an account sending to itself, which
    /// adds nothing to it and takes nothing from it but the fee, and is still a move to show.
    pub fn between<'a>(&'a self, addresses: &'a [String]) -> impl Iterator<Item = &'a Move> + 'a {
        let ours = move |party: &Option<String>| party.as_ref().is_some_and(|party| addresses.contains(party));

        self.moves.iter().filter(move |moved| ours(&moved.from) && ours(&moved.to))
    }
}

/// A transaction's kind, by the name the API gives it, and what a multisig sent as it.
fn kinds_of(tx: &Tx) -> Vec<String> {
    let name = |tx: &Tx| chainv4::structs::json::tx(tx)["type"].as_str().unwrap_or_default().to_string();

    match tx {
        Tx::Multisig { tx, .. } => [kinds_of(tx), vec!["multisig".to_string()]].concat(),
        // For being a batch, and for what any of its steps is.
        Tx::Batch { steps } => {
            let mut kinds = vec![name(tx)];

            for step in steps {
                let kind = name(step);

                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }

            kinds
        }
        tx => vec![name(tx)],
    }
}

/// What each step of a batch did, by what its receipt proved: the steps before the one that failed
/// ran and were undone, and those after it never ran. EVM calls are kept as far as MAX_KEPT goes
/// for all of them together, as for one transaction.
fn checked_steps(tx: &Tx, outcome: &CommittedOutcome) -> Vec<CheckedStep> {
    let steps = match tx {
        Tx::Batch { steps } => steps.as_slice(),
        Tx::Multisig { tx, .. } => return checked_steps(tx, outcome),
        _ => return Vec::new(),
    };
    let mut left = MAX_KEPT;

    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let (said, result) = match outcome {
                CommittedOutcome::Batch(outcomes) => (outcomes.get(index).map_or(CommittedOutcome::Done, |outcome| outcome.effect.outcome()), StepResult::Done),
                CommittedOutcome::BatchFailed { step: stopped } => (
                    CommittedOutcome::Failed,
                    match (index as u32).cmp(stopped) {
                        std::cmp::Ordering::Less => StepResult::Undone,
                        std::cmp::Ordering::Equal => StepResult::Failed,
                        std::cmp::Ordering::Greater => StepResult::NotRun,
                    },
                ),
                _ => (CommittedOutcome::Failed, StepResult::Undone),
            };
            let call = kept_call(step).filter(|call| call.input.len() <= left);
            left -= call.as_ref().map_or(0, |call| call.input.len());

            CheckedStep { action: action(step, &said), result, call }
        })
        .collect()
}

/// A token moving, by the standard's Transfer event: from, to and an amount. A mint comes from the
/// zero address, a burn goes to it.
fn token_move(event: &Event) -> Option<Move> {
    let [TxCallArgumentType::Address(from), TxCallArgumentType::Address(to), amount] = event.data.as_slice() else {
        return None;
    };

    if event.name != "Transfer" {
        return None;
    }

    let party = |address: &Address| address.bytes().iter().any(|byte| *byte != 0).then(|| address.to_string());

    Some(Move {
        token: Some(event.contract.to_string()),
        from: party(from),
        to: party(to),
        amount: whole(amount)?,
        id: None,
        standard: None,
    })
}

/// The tokens an EVM log says moved, an NFT's and a multi-token's as well as an ERC-20's: shown as
/// native ones are, once they are said to be only a claim.
fn evm_token_moves(event: &Event) -> Vec<Move> {
    let Some((topics, data)) = event.evm_log() else {
        return Vec::new();
    };

    token_moves(&topics, data)
        .into_iter()
        .map(|moved| Move {
            token: Some(event.contract.to_string()),
            from: moved.from.map(|from| to_hex(&from)),
            to: moved.to.map(|to| to_hex(&to)),
            amount: moved.amount.to_string(),
            id: moved.id.map(|id| id.to_string()),
            standard: Some(moved.standard.to_string()),
        })
        .collect()
}

/// The EVM call a transaction made, a multisig's or an Ethereum wallet's too, if short enough.
fn kept_call(tx: &Tx) -> Option<KeptCall> {
    let (to, input) = match tx {
        Tx::Multisig { tx, .. } => return kept_call(tx),
        Tx::EvmCall { to, input, .. } => (*to, input.clone()),
        Tx::Ethereum { raw } => {
            let ethereum = chainv4::crypto::ethereum::decode(raw).ok()?;

            (Address::Evm(ethereum.to?), ethereum.input)
        }
        _ => return None,
    };

    (!input.is_empty() && input.len() * 2 <= MAX_KEPT).then(|| KeptCall { to: to.to_string(), input: to_hex(&input) })
}

/// A transaction's EVM logs, in hex, in order, until they would come to more than is kept.
fn kept_logs(events: &[Event]) -> Vec<KeptLog> {
    let mut left = MAX_KEPT;
    let mut logs = Vec::new();

    for (position, event) in events.iter().enumerate() {
        let Some((topics, data)) = event.evm_log() else {
            continue;
        };
        let size = data.len() * 2 + topics.len() * 66;

        if size > left {
            break;
        }

        left -= size;
        logs.push(KeptLog {
            position: position as u32,
            contract: event.contract.to_string(),
            topics: topics.iter().map(|topic| to_hex(topic)).collect(),
            data: to_hex(data),
        });
    }

    logs
}

/// An account's addresses with the 20 bytes each goes by in the EVM added, in the form an EVM move
/// names its parties: what says which of those are this account's.
pub fn with_evm_forms(addresses: &[String]) -> Vec<String> {
    let forms = addresses.iter().filter_map(|address| address.parse::<Address>().ok()).map(|address| to_hex(&chainv4::abi::evm_address(&address)));

    addresses.iter().cloned().chain(forms).collect()
}

/// A whole amount from whatever type a contract emitted it as.
fn whole(value: &TxCallArgumentType) -> Option<String> {
    match value {
        TxCallArgumentType::U256(amount) => Some(amount.to_string()),
        TxCallArgumentType::Amount(amount) => Some(amount.to_u256().to_string()),
        TxCallArgumentType::U128(amount) => Some(amount.to_string()),
        TxCallArgumentType::U64(amount) => Some(amount.to_string()),
        TxCallArgumentType::I64(amount) if *amount >= 0 => Some(amount.to_string()),
        TxCallArgumentType::I128(amount) if *amount >= 0 => Some(amount.to_string()),
        TxCallArgumentType::I256(amount) if *amount >= 0 => Some(amount.to_string()),
        TxCallArgumentType::Number(amount) if *amount >= 0.0 && amount.fract() == 0.0 && *amount < 9.0e15 => Some(format!("{amount:.0}")),
        TxCallArgumentType::String(digits) if !digits.is_empty() && digits.chars().all(|digit| digit.is_ascii_digit()) => Some(digits.clone()),
        _ => None,
    }
}

/// What a transaction asked for, in a few words.
pub fn action(tx: &Tx, outcome: &CommittedOutcome) -> String {
    match (tx, outcome) {
        (Tx::Transfer { .. }, _) => "Transfer".to_string(),
        (Tx::Mint { .. }, _) => "Mint".to_string(),
        (Tx::Burn { .. }, _) => "Burn".to_string(),
        (Tx::ContractCall { to, func, .. }, _) => match function(func) {
            Some(name) => format!("Called {name} on {}", format::short(&to.to_string())),
            None => format!("Called {}", format::short(&to.to_string())),
        },
        (Tx::EvmCall { to, input, .. }, _) if input.is_empty() => format!("Sent through the EVM to {}", format::short(&to.to_string())),
        (Tx::EvmCall { to, .. }, _) => format!("Called {} on the EVM", format::short(&to.to_string())),
        (Tx::Deploy { .. }, CommittedOutcome::Deployed(contract)) => format!("Deployed {}", format::short(&contract.to_string())),
        (Tx::Deploy { .. }, _) => "Deploy".to_string(),
        (Tx::Upgrade { contract, .. }, _) => format!("Upgraded {}", format::short(&contract.to_string())),
        (Tx::Remove { contract, .. }, _) => format!("Removed {}", format::short(&contract.to_string())),
        (Tx::Bond { .. }, _) => "Bonded a voter".to_string(),
        (Tx::Unbond { .. }, _) => "Unbonded a voter".to_string(),
        (Tx::Withdraw { .. }, _) => "Withdrew a bond".to_string(),
        (Tx::SetAttester { attester: Some(_), .. }, _) => "Set a validator's attester".to_string(),
        (Tx::SetAttester { attester: None, .. }, _) => "Cleared a validator's attester".to_string(),
        (Tx::Slash { .. }, _) => "Reported a double vote".to_string(),
        (Tx::ApproveSigners { version, .. }, _) => format!("Approved signers v{version}"),
        (Tx::WithdrawApproval { version }, _) => format!("Withdrew approval of signers v{version}"),
        // The vote that decided its change: its receipt says which decision it was, and which
        // version of its kind it made; or the one that left it unable to pass.
        (Tx::VoteChange { .. }, CommittedOutcome::Decided { decision, kind, version }) => {
            format!("Decided change {decision} · {}, making {} v{version}", action(tx, &CommittedOutcome::Done), kind_words(*kind))
        }
        (Tx::VoteChange { .. }, CommittedOutcome::Rejected { rejection, kind, .. }) => {
            format!("Rejected a {} change, the chain's rejection {rejection} · {}", kind_words(*kind), action(tx, &CommittedOutcome::Done))
        }
        (Tx::VoteChange { change: Change::Upgrade(upgrade), vote, .. }, _) => {
            format!("Voted {} on an upgrade: nodes older than {} stop at block {}", voted(*vote), upgrade.least, upgrade.from)
        }
        (Tx::VoteChange { change: Change::Name(name), vote, .. }, _) => format!("Voted {} on renaming the chain to {name}", voted(*vote)),
        (Tx::VoteChange { version, change, vote }, _) => format!("Voted {} on a {} change from v{version}", voted(*vote), change.name().replace('_', " ")),
        (Tx::ClearVote { version, change }, _) => format!("Cleared a vote on a {} change from v{version}", kind_words(change.kind())),
        (Tx::SetDirectory { .. }, _) => "Set the chain's directory".to_string(),
        (Tx::HandDirectory { to: Some(to) }, _) => format!("Handed the directory to {}", format::short(&to.to_string())),
        (Tx::HandDirectory { to: None }, _) => "Renounced the directory".to_string(),
        (Tx::Name { name, action }, _) => match action {
            NameAction::Register { years } => format!("Registered the name {name} for {years} year{}", if *years == 1 { "" } else { "s" }),
            NameAction::Renew { years } => format!("Renewed {name} for {years} year{}", if *years == 1 { "" } else { "s" }),
            NameAction::Transfer { to } => format!("Gave {name} to {}", format::short(&to.to_string())),
            NameAction::Target { target: Some(target) } => format!("Pointed {name} at {}", format::short(&target.to_string())),
            NameAction::Target { target: None } => format!("Pointed {name} at nothing"),
            NameAction::Text { key, .. } => format!("Set {key} on {name}"),
            NameAction::Subname { label, .. } => format!("Made {label}.{name}"),
        },
        (Tx::PrimaryName { name: Some(name) }, _) => format!("Made {name} the primary name"),
        (Tx::PrimaryName { name: None }, _) => "Cleared the primary name".to_string(),
        (Tx::Ethereum { .. }, CommittedOutcome::Deployed(contract)) => format!("Deployed {} from an Ethereum wallet", format::short(&contract.to_string())),
        (Tx::Ethereum { raw }, _) => match chainv4::crypto::ethereum::decode(raw).ok() {
            // EIP-7702's type 4: each authorization sets whose code an account runs, or clears it.
            Some(ethereum) if !ethereum.authorizations.is_empty() => match ethereum.authorizations.as_slice() {
                [one] if one.address == [0; 20] => "Cleared an account's delegation, from an Ethereum wallet".to_string(),
                [one] => format!("Delegated an account to {}, from an Ethereum wallet", format::short(&Address::Evm(one.address).to_string())),
                many => format!("Set {} accounts' delegations, from an Ethereum wallet", many.len()),
            },
            Some(ethereum) => match ethereum.to {
                Some(to) => format!("Sent from an Ethereum wallet to {}", format::short(&chainv4::structs::hex::to_hex(&to))),
                None => "Deploy from an Ethereum wallet".to_string(),
            },
            None => "Sent from an Ethereum wallet".to_string(),
        },
        (Tx::Multisig { account, tx, .. }, _) => format!("{} · by the multisig {}", action(tx, outcome), format::short(&account.to_string())),
        (Tx::CreateMultisig { members, threshold, .. }, CommittedOutcome::Deployed(account)) => {
            format!("Created the multisig {}, {threshold} of {}", format::short(&account.to_string()), members.len())
        }
        (Tx::CreateMultisig { members, threshold, .. }, _) => format!("Create a multisig, {threshold} of {}", members.len()),
        (Tx::ChangeMultisig { members, threshold, .. }, _) => format!("Made the multisig {threshold} of {}", members.len()),
        (Tx::ApproveMultisig { account, nonce, .. }, _) => format!("Approved {}'s transaction {nonce}", format::short(&account.to_string())),
        (Tx::WithdrawMultisig { account, .. }, _) => format!("Withdrew an approval of {}'s", format::short(&account.to_string())),
        // What it names is all the miner earned, not what this claim paid: that is less what it
        // claimed before, which the chain knows and the transaction does not say.
        (Tx::Claim { to, .. }, _) => format!("Claimed pool earnings for {}", format::short(&to.to_string())),
        (Tx::Anchor { proof: chainv4::structs::tx::AnchorProof::Bitcoin(_) }, _) => "Showed a checkpoint anchored in Bitcoin".to_string(),
        (Tx::Anchor { proof: chainv4::structs::tx::AnchorProof::Ethereum(_) }, _) => "Showed a checkpoint anchored in Ethereum".to_string(),
        (Tx::Batch { steps }, CommittedOutcome::BatchFailed { step }) => format!("A batch of {} step{}, which step {} stopped", steps.len(), if steps.len() == 1 { "" } else { "s" }, step + 1),
        (Tx::Batch { steps }, _) => format!("A batch of {} step{}", steps.len(), if steps.len() == 1 { "" } else { "s" }),
    }
}

/// The name a call's selector stands for, among those the history knows.
pub(crate) fn function(func: &Hash) -> Option<&'static str> {
    KNOWN.into_iter().find(|name| Hasher::digest(HashType::Xxh3_64, name.as_bytes()) == *func)
}

/// An event in a line: its name and what it carries, addresses cut short. An EVM log, all bytes,
/// says what its first topic is, for when its bytes are too long to keep.
pub fn describe(event: &Event) -> String {
    if let Some((topics, data)) = event.evm_log() {
        return match topics.first() {
            Some(first) => format!("{} {} · {} topics, {} bytes", event.name, format::short(&to_hex(first)), topics.len(), data.len()),
            None => format!("{} · {} bytes", event.name, data.len()),
        };
    }

    let data: Vec<String> = event
        .data
        .iter()
        .map(|value| match value {
            TxCallArgumentType::Address(address) => format::short(&address.to_string()),
            TxCallArgumentType::String(text) => format!("{text:?}"),
            TxCallArgumentType::Bytes(bytes) => format!("{} bytes", bytes.len()),
            TxCallArgumentType::Bool(flag) => flag.to_string(),
            TxCallArgumentType::Nil => "nil".to_string(),
            other => whole(other).unwrap_or_else(|| format!("{other:?}")),
        })
        .collect();

    match data.is_empty() {
        true => event.name.clone(),
        false => format!("{} {}", event.name, data.join(" ")),
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        chainv4::crypto::keypair::Keypair,
        chainv4::light::Assurance,
        chainv4::structs::amount::Amount,
        chainv4::structs::receipt::CommittedReceipt,
        chainv4::structs::tx::{FeeOffer, SignedTx},
    };

    fn landed(tx: Tx, outcome: CommittedOutcome, events: Vec<Event>) -> Proven<Landed> {
        // What a row says comes from its receipt and transaction: no chain here checks a signature.
        let signed = SignedTx::sign(tx, 4, FeeOffer::NONE, &Keypair::development(), &chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()));

        Proven {
            value: Landed {
                receipt: CommittedReceipt {
                    tx: signed.id(HashType::default_chain()),
                    fuel_used: 0,
                    gas_used: 21_000,
                    fee: u256::new(21),
                    outcome,
                    events,
                    payments: Vec::new(),
                },
                signed,
                time: 1_000,
            },
            assurance: Assurance {
                block: Hash::zero(HashType::default_chain()),
                height: 9,
                final_: true,
            },
        }
    }

    #[test]
    fn the_directory_admin_s_transactions_read_as_what_they_did() {
        let to = Address::Ed25519([5; 32]);

        assert_eq!(action(&Tx::SetDirectory { directory: Default::default() }, &CommittedOutcome::Done), "Set the chain's directory");
        assert!(action(&Tx::HandDirectory { to: Some(to) }, &CommittedOutcome::Done).starts_with("Handed the directory to"));
        assert_eq!(action(&Tx::HandDirectory { to: None }, &CommittedOutcome::Done), "Renounced the directory");
    }

    #[test]
    fn changes_to_fees_the_reward_limits_finality_upgrades_and_the_name_read_back_without_the_node() {
        use chainv4::structs::version::{Upgrade, Version};
        use chainv4::params::fees::{Bid, FeeConfig, PricingConfig, ScheduleConfig, Weighted};
        use chainv4::params::finality::{DepthConfig, FfgConfig, FinalityConfig};
        use chainv4::params::{BlockLimits, Limits, Reward};
        use chainv4::registry::validators::Leak;

        let ffg = FfgConfig {
            checkpoint_interval: 8,
            vote_depth: 3,
            min_bond: u256::new(500),
            activation_delay: 2,
            withdrawal_delay: 64,
            reporter_divisor: 16,
            max_validators: 1_000,
            vote_reward: u256::new(1),
            max_proofs_per_block: 4,
            validators: Vec::new(),
            leak: Some(Leak { after_epochs: 4, permille: 10 }),
            slash_permille: None,
        };
        let changes = [
            (Change::Fees(FeeConfig {
                block_gas_limit: 50_000_000,
                burn_permille: 500,
                schedule: ScheduleConfig::Weighted(Weighted { per_tx: 1_000, per_byte: 10, per_code_byte: 20, fuel_per_gas: 1 }),
                pricing: PricingConfig::Bid(Bid { min_price: u256::new(7) }),
                costs: None,
                lua_costs: None,
                vote_rewards: None,
            }), "Voted Yes on a fees change from v3"),
            (Change::Reward(Reward { initial: u256::new(25), halving_interval: Some(210_000) }), "Voted Yes on a reward change from v3"),
            (Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10_000_000, memory: 64 << 20 }, max_block_bytes: None, contract: None, max_tx_bytes: None, multisig_members: None }), "Voted Yes on a limits change from v3"),
            (Change::Finality(FinalityConfig::Ffg(ffg)), "Voted Yes on a finality change from v3"),
            (Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })), "Voted Yes on a finality change from v3"),
            (Change::Upgrade(Upgrade { least: Version::new(0, 2, 0), from: 150_000 }), "Voted Yes on an upgrade: nodes older than 0.2.0 stop at block 150000"),
            (Change::Name("chainv4 mainnet".to_string()), "Voted Yes on renaming the chain to chainv4 mainnet"),
        ];

        for (change, said) in changes {
            let signed = SignedTx::sign(Tx::VoteChange { version: 3, change: change.clone(), vote: chainv4::governance::ChangeVote::Yes }, 4, FeeOffer::NONE, &Keypair::development(), &chainv4::structs::hash::Hash::zero(chainv4::structs::hash::HashType::default_chain()));
            let back: SignedTx = wincode::deserialize(&wincode::serialize(&signed).expect("encodes")).expect("decodes in a light build");

            assert_eq!(back, signed);
            assert!(matches!(&back.tx, Tx::VoteChange { change: read, .. } if *read == change));
            assert_eq!(Checked::of(&landed(back.tx, CommittedOutcome::Done, Vec::new())).action, said);
        }
    }

    #[test]
    fn the_vote_that_decided_a_change_says_which_decision_and_version_of_its_kind_it_made() {
        let approve = Tx::VoteChange { version: 4, change: Change::Name("second".to_string()), vote: chainv4::governance::ChangeVote::Yes };
        let made = CommittedOutcome::Decided { decision: 5, kind: ChangeKind::Name, version: 5 };
        let decided = Checked::of(&landed(approve.clone(), made.clone(), Vec::new()));

        assert_eq!(decided.action, "Decided change 5 · Voted Yes on renaming the chain to second, making name v5");
        assert!(decided.done, "a decision did what it asked");
        assert_eq!(Checked::of(&landed(approve.clone(), CommittedOutcome::Done, Vec::new())).action, "Voted Yes on renaming the chain to second", "one that only counted");

        // The chain's count of decisions and the kind's version are apart: the ninth decision
        // made the reward v2.
        let reward = Tx::VoteChange { version: 1, change: Change::Reward(chainv4::params::Reward { initial: u256::new(1), halving_interval: None }), vote: chainv4::governance::ChangeVote::Yes };
        let ninth = Checked::of(&landed(reward, CommittedOutcome::Decided { decision: 9, kind: ChangeKind::Reward, version: 2 }, Vec::new()));
        assert_eq!(ninth.action, "Decided change 9 · Voted Yes on a reward change from v1, making reward v2");
        assert!(matches!(ninth.vote, Some(Vote::Voted { decided: Some(9), kind: ChangeKind::Reward, kind_version: Some(2), version: 1, .. })));

        // A multisig's approval that decided says so too.
        let multisig = Tx::Multisig { account: Address::Contract([6; 32]), tx: Box::new(approve), signatures: Vec::new() };
        let by_multisig = Checked::of(&landed(multisig, made, Vec::new()));

        assert!(by_multisig.action.starts_with("Decided change 5 · Voted Yes on renaming the chain to second, making name v5 · by the multisig contract:0x0606"), "{}", by_multisig.action);
        assert_eq!(
            by_multisig.vote,
            Some(Vote::Voted { version: 4, change: r#"{"name":"second"}"#.to_string(), vote: "yes".to_string(), decided: Some(5), rejected: None, kind: ChangeKind::Name, kind_version: Some(5) }),
            "kept for the Governance page"
        );
        assert_eq!(decided.vote, by_multisig.vote);
        assert_eq!(Checked::of(&landed(Tx::Burn { from: Address::Ed25519([1; 32]), amount: Amount::U128(1) }, CommittedOutcome::Done, Vec::new())).vote, None);
    }

    #[test]
    fn a_cleared_vote_names_its_change_and_a_no_that_rejected_says_so() {
        let prices = Change::Names(chainv4::registry::names::NameSettings { prices: vec![u256::new(1)], grace_ms: 1_000, max_years: 5 });
        let cleared = Checked::of(&landed(Tx::ClearVote { version: 6, change: prices.clone() }, CommittedOutcome::Done, Vec::new()));

        assert_eq!(cleared.action, "Cleared a vote on a names change from v6");
        assert_eq!(cleared.vote, Some(Vote::Cleared { kind: ChangeKind::Names, version: 6, change: Some(chainv4::params::json::change_json(&prices).to_string()) }));

        let no = Tx::VoteChange { version: 2, change: Change::Name("second".to_string()), vote: chainv4::governance::ChangeVote::No };
        let rejected = Checked::of(&landed(no, CommittedOutcome::Rejected { rejection: 3, kind: ChangeKind::Name, version: 2 }, Vec::new()));

        assert_eq!(rejected.action, "Rejected a name change, the chain's rejection 3 · Voted No on renaming the chain to second");
        assert!(matches!(rejected.vote, Some(Vote::Voted { rejected: Some(3), decided: None, ref vote, .. }) if vote == "no"));
    }

    #[test]
    fn votes_keep_their_kind_and_ones_kept_before_kinds_read_as_the_consensus() {
        let vote = Vote::Cleared { kind: ChangeKind::Fees, version: 3, change: None };
        let kept = serde_json::to_value(&vote).unwrap();

        assert_eq!(kept, serde_json::json!({ "Cleared": { "kind": "fees", "version": 3, "change": null } }), "the kind by its name");
        assert_eq!(serde_json::from_value::<Vote>(kept).unwrap(), vote);

        let approved = Vote::Voted { version: 2, change: r#"{"name":"x"}"#.to_string(), vote: "no".to_string(), decided: Some(7), rejected: None, kind: ChangeKind::Name, kind_version: Some(3) };
        assert_eq!(serde_json::from_value::<Vote>(serde_json::to_value(&approved).unwrap()).unwrap(), approved);

        // As a wallet kept them before each kind had a version of its own.
        let old_withdrawal: Vote = serde_json::from_value(serde_json::json!({ "Withdrew": { "version": 6 } })).unwrap();
        let old_approval: Vote = serde_json::from_value(serde_json::json!({ "Approved": { "version": 4, "change": "{}", "decided": 5 } })).unwrap();

        assert_eq!(old_withdrawal, Vote::Cleared { kind: ChangeKind::Consensus, version: 6, change: None }, "a withdrawal, as a cleared vote");
        assert_eq!(old_approval, Vote::Voted { version: 4, change: "{}".to_string(), vote: "yes".to_string(), decided: Some(5), rejected: None, kind: ChangeKind::Consensus, kind_version: None }, "an approval, as a Yes");
        assert!(serde_json::from_value::<Vote>(serde_json::json!({ "Withdrew": { "kind": "weather", "version": 1 } })).is_err(), "a kind the wallet does not know");
    }

    #[test]
    fn a_coin_transfer_comes_in_for_one_and_goes_out_for_the_other() {
        let me = Keypair::development().public().address();
        let them = Address::Ed25519([3; 32]);
        let checked = Checked::of(&landed(Tx::Transfer { from: me, to: them, amount: Amount::U128(15) }, CommittedOutcome::Done, Vec::new()));

        assert_eq!((checked.height, checked.nonce, checked.fee.as_str(), checked.action.as_str()), (9, 4, "21", "Transfer"));
        assert_eq!(checked.for_account(&[them.to_string()]).map(|(incoming, moved)| (incoming, moved.amount.as_str())).collect::<Vec<_>>(), [(true, "15")]);
        assert_eq!(checked.for_account(&[me.to_string()]).map(|(incoming, _)| incoming).collect::<Vec<_>>(), [false]);
        assert_eq!(checked.for_account(&["ed25519:0x00".to_string()]).count(), 0, "it touched neither");
        assert_eq!(checked.for_account(&[me.to_string(), them.to_string()]).count(), 0, "between one account's own addresses");
    }

    #[test]
    fn tokens_move_by_their_transfer_events_and_a_failure_moves_nothing() {
        let me = Keypair::development().public().address();
        let (them, token) = (Address::Ed25519([3; 32]), Address::Contract([5; 32]));
        let call = || Tx::ContractCall {
            to: token,
            func: Hasher::digest(HashType::Xxh3_64, b"transfer"),
            amount: Amount::U128(0),
            data: vec![TxCallArgumentType::Address(them), TxCallArgumentType::U256(u256::new(200))],
        };
        let transfer = Event {
            contract: token,
            name: "Transfer".to_string(),
            data: vec![TxCallArgumentType::Address(me), TxCallArgumentType::Address(them), TxCallArgumentType::U256(u256::new(200))],
        };
        let checked = Checked::of(&landed(call(), CommittedOutcome::Returned(Vec::new()), vec![transfer.clone()]));
        let moved = &checked.moves[0];

        assert_eq!((moved.token.clone(), moved.amount.as_str()), (Some(token.to_string()), "200"));
        assert!(checked.action.starts_with("Called transfer on contract:0x050505"), "{}", checked.action);
        assert_eq!(checked.for_account(&[them.to_string()]).count(), 1);
        assert!(checked.events[0].starts_with("Transfer ed25519:0x"));

        let failed = Checked::of(&landed(call(), CommittedOutcome::Failed, Vec::new()));

        assert!(!failed.done);
        assert!(failed.moves.is_empty());

        // A mint comes from the zero address, which is no one's.
        let minted = Event {
            data: vec![TxCallArgumentType::Address(Address::Ed25519([0; 32])), TxCallArgumentType::Address(them), TxCallArgumentType::String("7".to_string())],
            ..transfer
        };

        assert_eq!(token_move(&minted).map(|moved| (moved.from, moved.amount)), Some((None, "7".to_string())));
    }

    #[test]
    fn coin_a_contract_paid_comes_in_for_whom_it_paid() {
        let (contract, them) = (Address::Contract([5; 32]), Address::Ed25519([3; 32]));
        let call = Tx::ContractCall { to: contract, func: Hasher::digest(HashType::Xxh3_64, b"claim"), amount: Amount::U128(0), data: Vec::new() };
        let mut proven = landed(call, CommittedOutcome::Returned(Vec::new()), Vec::new());
        proven.value.receipt.payments = vec![chainv4::structs::receipt::Payment { from: contract, to: them, amount: u256::new(900) }];
        let checked = Checked::of(&proven);

        assert_eq!(checked.for_account(&[them.to_string()]).map(|(incoming, moved)| (incoming, moved.token.clone(), moved.amount.as_str())).collect::<Vec<_>>(), [(true, None, "900")]);
        assert_eq!(checked.for_account(&[contract.to_string()]).map(|(incoming, _)| incoming).collect::<Vec<_>>(), [false]);

        proven.value.receipt.outcome = CommittedOutcome::Failed;
        assert!(Checked::of(&proven).moves.is_empty(), "a failure paid nothing");
    }

    #[test]
    fn a_claim_says_whom_it_was_for_and_not_what_it_paid() {
        let miner = Address::Ed25519([3; 32]);
        let claim = Tx::Claim { height: 40, to: miner, earned: u256::new(5_000), index: 2, path: vec![Hash::zero(HashType::default_chain())] };
        let checked = Checked::of(&landed(claim, CommittedOutcome::Done, Vec::new()));

        assert_eq!(checked.action, format!("Claimed pool earnings for {}", format::short(&miner.to_string())));
        assert!(!checked.action.contains("5000"), "all it earned is not what this claim paid");
    }

    /// An EVM log, as the chain holds one.
    fn log(contract: [u8; 20], topics: Vec<[u8; 32]>, data: Vec<u8>) -> Event {
        Event {
            contract: Address::Evm(contract),
            name: chainv4::structs::event::EVM_LOG.to_string(),
            data: topics.iter().map(|topic| TxCallArgumentType::Bytes(topic.to_vec())).chain([TxCallArgumentType::Bytes(data)]).collect(),
        }
    }

    fn pad(address: [u8; 20]) -> [u8; 32] {
        let mut topic = [0; 32];
        topic[12..].copy_from_slice(&address);

        topic
    }

    fn transfer() -> [u8; 32] {
        chainv4::abi::signature_topic("Transfer(address,address,uint256)").unwrap()
    }

    fn evm_call(input: Vec<u8>) -> Tx {
        Tx::EvmCall { to: Address::Evm([5; 20]), value: Amount::U128(0), input }
    }

    #[test]
    fn an_erc20_s_and_an_nft_s_logs_are_token_moves_for_whoever_their_topics_name() {
        let (me, them) = ([1; 20], [2; 20]);
        let events = vec![
            log([5; 20], vec![transfer(), pad(me), pad(them)], u256::new(200).to_be_bytes().to_vec()),
            log([6; 20], vec![transfer(), pad(them), pad(me), u256::new(9).to_be_bytes()], Vec::new()),
            log([5; 20], vec![transfer(), [0; 32], pad(me)], u256::new(7).to_be_bytes().to_vec()),
            log([5; 20], vec![[9; 32]], Vec::new()),
        ];
        let checked = Checked::of(&landed(evm_call(vec![1, 2, 3, 4]), CommittedOutcome::Returned(Vec::new()), events));
        let mine = [to_hex(&me)];
        let moved: Vec<(bool, &Move)> = checked.for_account(&mine).collect();

        assert_eq!(moved.len(), 3, "the unknown log moved nothing");
        assert_eq!((moved[0].0, moved[0].1.token.as_deref(), moved[0].1.amount.as_str(), moved[0].1.id.as_deref()), (false, Some("evm:0x0505050505050505050505050505050505050505"), "200", None));
        assert_eq!((moved[1].0, moved[1].1.amount.as_str(), moved[1].1.id.as_deref(), moved[1].1.standard.as_deref()), (true, "1", Some("9"), Some("ERC-721")));
        assert_eq!((moved[2].0, moved[2].1.from.clone()), (true, None), "a mint comes from no one");

        // A failure moved nothing, whatever it logged.
        let failed = Checked::of(&landed(evm_call(vec![1, 2, 3, 4]), CommittedOutcome::Failed, vec![log([5; 20], vec![transfer(), pad(me), pad(them)], u256::new(1).to_be_bytes().to_vec())]));
        assert!(failed.moves.is_empty());
    }

    #[test]
    fn an_account_s_evm_forms_say_which_parties_are_its_own() {
        let (secp, ed) = (Address::Secp256k1([3; 20]), Address::Ed25519([4; 32]));
        let forms = with_evm_forms(&[secp.to_string(), ed.to_string(), "not an address".to_string()]);

        assert_eq!(&forms[..3], [secp.to_string(), ed.to_string(), "not an address".to_string()], "what it had, first");
        assert_eq!(forms[3], to_hex(&[3; 20]), "a secp256k1 account's own 20 bytes");
        assert_eq!(forms[4], to_hex(&chainv4::abi::evm_address(&ed)), "any other's alias");
        assert_eq!(forms.len(), 5);

        let checked = Checked::of(&landed(evm_call(vec![1, 2, 3, 4]), CommittedOutcome::Returned(Vec::new()), vec![log([5; 20], vec![transfer(), pad([9; 20]), pad(chainv4::abi::evm_address(&ed))], u256::new(1).to_be_bytes().to_vec())]));

        assert_eq!(checked.for_account(&with_evm_forms(&[ed.to_string()])).count(), 1, "the alias a token paid is this account");
        assert_eq!(checked.for_account(&[ed.to_string()]).count(), 0, "as its own address alone says nothing");
    }

    #[test]
    fn a_move_from_a_contract_nothing_lists_is_only_a_claim() {
        let moved = Checked::of(&landed(evm_call(vec![1, 2, 3, 4]), CommittedOutcome::Returned(Vec::new()), vec![log([5; 20], vec![transfer(), pad([1; 20]), pad([2; 20])], u256::new(1).to_be_bytes().to_vec())])).moves.remove(0);
        let native = Move { token: Some("contract:0x05".to_string()), from: None, to: None, amount: "1".to_string(), id: None, standard: None };

        assert!(moved.claimed(false));
        assert!(!moved.claimed(true), "the registry lists it");
        assert!(!native.claimed(false), "native moves are as they were");
    }

    #[test]
    fn a_log_s_bytes_and_a_call_s_survive_being_kept() {
        let input = vec![0xa9, 0x05, 0x9c, 0xbb, 1];
        let checked = Checked::of(&landed(evm_call(input.clone()), CommittedOutcome::Returned(Vec::new()), vec![
            Event { contract: Address::Contract([1; 32]), name: "Native".to_string(), data: Vec::new() },
            log([5; 20], vec![transfer(), pad([1; 20]), pad([2; 20])], u256::new(7).to_be_bytes().to_vec()),
        ]));
        let back: Checked = serde_json::from_value(serde_json::to_value(&checked).unwrap()).unwrap();

        assert_eq!(back, checked, "whole, through what settings are kept as");
        assert_eq!(back.call, Some(KeptCall { to: "evm:0x0505050505050505050505050505050505050505".to_string(), input: to_hex(&input) }));
        assert_eq!(back.logs[0].position, 1, "after the native event");
        assert_eq!(back.logs[0].topics[0], to_hex(&transfer()));
        assert_eq!(back.logs[0].data, to_hex(&u256::new(7).to_be_bytes()));
        assert!(back.events[1].starts_with("log 0xddf2"), "{}", back.events[1]);

        // A multisig's call is the call it made.
        let by_multisig = Tx::Multisig { account: Address::Multisig([6; 32]), tx: Box::new(evm_call(input.clone())), signatures: Vec::new() };
        assert_eq!(Checked::of(&landed(by_multisig, CommittedOutcome::Returned(Vec::new()), Vec::new())).call.map(|call| call.input), Some(to_hex(&input)));

        // What is too long to keep says how long it was, and moves are still seen.
        let long = log([5; 20], vec![transfer(), pad([1; 20]), pad([2; 20])], vec![0; MAX_KEPT]);
        let checked = Checked::of(&landed(evm_call(vec![0; MAX_KEPT]), CommittedOutcome::Returned(Vec::new()), vec![long]));

        assert!(checked.logs.is_empty() && checked.call.is_none());
        assert!(checked.events[0].ends_with(&format!("{MAX_KEPT} bytes")), "{}", checked.events[0]);
    }

    #[test]
    fn what_older_wallets_kept_still_reads() {
        // A Checked and a Move as a wallet wrote them before calls and logs were kept: settings that
        // fail to decode lose everything in them.
        let old = serde_json::json!({
            "height": 7, "time": 0, "final_": true, "done": true, "sender": "ed25519:0xaa", "nonce": 1,
            "action": "Called evm:0x05 on the EVM",
            "moves": [{ "token": "contract:0x05", "from": "ed25519:0xaa", "to": "ed25519:0xbb", "amount": "5" }],
            "fee": "0", "events": ["log 32 bytes 32 bytes"],
        });
        let checked: Checked = serde_json::from_value(old).unwrap();

        assert_eq!((checked.call.clone(), checked.logs.len()), (None, 0));
        assert_eq!((checked.moves[0].id.clone(), checked.moves[0].standard.clone()), (None, None));

        // And one kept with nothing new to say writes as it did.
        let kept = serde_json::to_value(&checked).unwrap();

        assert!(kept.get("call").is_none() && kept.get("logs").is_none() && kept["moves"][0].get("standard").is_none());
    }

    #[test]
    fn events_read_in_a_line() {
        let event = Event {
            contract: Address::Contract([1; 32]),
            name: "Signed".to_string(),
            data: vec![TxCallArgumentType::Address(Address::Ed25519([0xab; 32])), TxCallArgumentType::String("gm".to_string()), TxCallArgumentType::I64(3)],
        };

        assert_eq!(describe(&event), "Signed ed25519:0xababab…abab \"gm\" 3");

        // A native event called log is no EVM log.
        let named_log = Event { contract: Address::Contract([1; 32]), name: "log".to_string(), data: vec![TxCallArgumentType::Bytes(vec![1; 3])] };
        assert_eq!(describe(&named_log), "log 3 bytes");
    }
}

#[cfg(test)]
mod batch_tests {
    use {
        super::*,
        chainv4::crypto::keypair::Keypair,
        chainv4::light::Assurance,
        chainv4::structs::amount::Amount,
        chainv4::structs::receipt::{CommittedReceipt, StepEffect, StepOutcome},
        chainv4::structs::tx::{FeeOffer, SignedTx},
    };

    fn checked(tx: Tx, outcome: CommittedOutcome) -> Checked {
        let key = Keypair::from_seed([7; 32]);
        let signed = SignedTx::sign(tx, 0, FeeOffer::NONE, &key, &chainv4::structs::hash::Hash::zero(HashType::default_chain()));
        let id = signed.id(HashType::default_chain());
        let receipt = CommittedReceipt::new(id, 0, 0, u256::ZERO, outcome, Vec::new(), Vec::new());

        Checked::of(&Proven {
            value: Landed { signed, receipt, time: 0 },
            assurance: Assurance { block: id, height: 3, final_: true },
        })
    }

    fn me() -> Address {
        Keypair::from_seed([7; 32]).public().address()
    }

    fn steps() -> Vec<Tx> {
        let to = Keypair::from_seed([8; 32]).public().address();

        vec![
            Tx::Transfer { from: me(), to, amount: Amount::U128(5) },
            Tx::EvmCall { to: Address::Evm([1; 20]), value: Amount::U128(0), input: vec![1, 2, 3, 4] },
            Tx::Deploy { code: chainv4::structs::code::Code::Lua("return {}".to_string()) },
        ]
    }

    #[test]
    fn a_proven_batch_says_each_step_and_what_it_did() {
        let made = Address::Contract([9; 32]);
        let done = |effect| StepOutcome { effect, events: 0, payments: 0 };
        let kept = checked(Tx::Batch { steps: steps() }, CommittedOutcome::Batch(vec![done(StepEffect::Done), done(StepEffect::Returned(Vec::new())), done(StepEffect::Deployed(made))]));

        assert!(kept.done);
        assert_eq!(kept.action, "A batch of 3 steps");
        assert_eq!(kept.kinds, ["batch", "transfer", "evm_call", "deploy"]);
        assert_eq!(kept.steps.iter().map(|step| step.result).collect::<Vec<_>>(), [StepResult::Done; 3]);
        assert_eq!(kept.steps[0].action, "Transfer");
        assert!(kept.steps[2].action.starts_with("Deployed "), "{}", kept.steps[2].action);
        assert_eq!(kept.steps[1].call.as_ref().map(|call| call.input.as_str()), Some("0x01020304"));
        assert_eq!(kept.moves.len(), 1, "the coin its transfer moved");
    }

    #[test]
    fn a_failed_batch_moved_nothing_and_says_which_step_stopped_it() {
        let kept = checked(Tx::Batch { steps: steps() }, CommittedOutcome::BatchFailed { step: 1 });

        assert!(!kept.done);
        assert!(kept.moves.is_empty(), "what the first step moved went back");
        assert_eq!(kept.action, "A batch of 3 steps, which step 2 stopped");
        assert_eq!(kept.steps.iter().map(|step| step.result).collect::<Vec<_>>(), [StepResult::Undone, StepResult::Failed, StepResult::NotRun]);
    }

    #[test]
    fn a_multisigs_batch_says_the_same_and_counts_as_a_multisigs() {
        let account = Address::Multisig([6; 32]);
        let carried = Tx::Multisig { account, tx: Box::new(Tx::Batch { steps: vec![Tx::Burn { from: account, amount: Amount::U128(1) }] }), signatures: Vec::new() };
        let kept = checked(carried, CommittedOutcome::Batch(vec![StepOutcome { effect: StepEffect::Done, events: 0, payments: 0 }]));

        assert_eq!(kept.kinds, ["batch", "burn", "multisig"]);
        assert_eq!(kept.steps.len(), 1);
        assert!(kept.action.starts_with("A batch of 1 step · by the multisig"), "{}", kept.action);
        assert_eq!(kept.moves.len(), 1);
    }

    #[test]
    fn what_older_wallets_kept_reads_back_with_no_steps_and_a_batch_reads_back_whole() {
        let old = checked(Tx::Burn { from: me(), amount: Amount::U128(1) }, CommittedOutcome::Done);
        let written = serde_json::to_string(&old).unwrap();

        assert!(!written.contains("steps"), "nothing new in what any other transaction keeps");
        assert_eq!(serde_json::from_str::<Checked>(&written).unwrap(), old);

        let batch = checked(Tx::Batch { steps: steps() }, CommittedOutcome::BatchFailed { step: 0 });

        assert_eq!(serde_json::from_str::<Checked>(&serde_json::to_string(&batch).unwrap()).unwrap(), batch);
    }

    #[test]
    fn an_eip_7702_transaction_says_whose_code_an_account_now_runs() {
        use chainv4::crypto::ethereum::{Authorization, EthereumKind, EthereumTx};

        let secret = [0x47; 32];
        let raw = |addresses: &[[u8; 20]]| {
            let authorizations = addresses.iter().map(|address| Authorization { chain_id: u256::ONE, address: *address, nonce: 1, parity: 0, signature: [0; 64] }.signed(&secret)).collect();
            let tx = EthereumTx {
                kind: EthereumKind::SetCode,
                chain_id: 1,
                nonce: 0,
                max_price: u256::ONE,
                tip: u256::ZERO,
                gas_limit: 100_000,
                to: Some([1; 20]),
                value: u256::ZERO,
                input: Vec::new(),
                access_list: Vec::new(),
                signature: [0; 64],
                parity: 0,
                authorizations,
            };

            Tx::Ethereum { raw: tx.signed(&secret).encode() }
        };
        let said = |addresses: &[[u8; 20]]| action(&raw(addresses), &CommittedOutcome::Done);

        assert_eq!(said(&[[0xd7; 20]]), format!("Delegated an account to {}, from an Ethereum wallet", format::short(&Address::Evm([0xd7; 20]).to_string())));
        assert_eq!(said(&[[0; 20]]), "Cleared an account's delegation, from an Ethereum wallet");
        assert_eq!(said(&[[0xd7; 20], [0xd8; 20]]), "Set 2 accounts' delegations, from an Ethereum wallet");
    }
}

