//! What the history shows: every transaction an account sent, or that touched it, described from
//! what the light client proved of it. Each is kept once proven, so it shows again without asking:
//! a light client restored from storage keeps headers only from the finalized block on, and could
//! not prove an older one again.

use {
    crate::chain::{Landed, Proven},
    crate::format,
    chainv4::crypto::hasher::Hasher,
    chainv4::governance::{Change, ChangeKind},
    chainv4::structs::address::Address,
    chainv4::structs::event::Event,
    chainv4::structs::hash::{Hash, HashType},
    chainv4::structs::receipt::CommittedOutcome,
    chainv4::registry::names::NameAction,
    chainv4::structs::tx::{Tx, TxCallArgumentType},
    chainv4::structs::u256::u256,
    serde::{Deserialize, Serialize},
};

/// Functions a call may name, so the history says which one ran; a call to any other says only
/// whom it called.
const KNOWN: [&str; 10] = ["transfer", "transfer_from", "approve", "mint", "burn", "sign", "tip", "ping", "greet", "upgrade"];

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
}

/// A governance transaction, as proven. Each kind of change has a ballot and a version of its own;
/// a record kept before votes named their kind reads as the consensus's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Vote {
    /// An approval of a change, at the version of its kind it named: the change's JSON, and for
    /// the approval that was the last one needed, the decision it was, the chain's count, and the
    /// version of its kind it made.
    Approved {
        version: u64,
        change: String,
        decided: Option<u64>,
        #[serde(default = "consensus", with = "kind_name")]
        kind: ChangeKind,
        #[serde(default)]
        kind_version: Option<u64>,
    },
    /// An approval of a change of a kind withdrawn, at the kind's version it named.
    Withdrew {
        #[serde(default = "consensus", with = "kind_name")]
        kind: ChangeKind,
        version: u64,
    },
}

/// What a vote kept before votes named their kind was of.
fn consensus() -> ChangeKind {
    ChangeKind::Consensus
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
            Tx::ApproveChange { version, change } => {
                let decided = match outcome {
                    CommittedOutcome::Decided { decision, version, .. } => Some((*decision, *version)),
                    _ => None,
                };

                Some(Vote::Approved {
                    version: *version,
                    change: chainv4::params::json::change_json(change).to_string(),
                    decided: decided.map(|(decision, _)| decision),
                    kind: change.kind(),
                    kind_version: decided.map(|(_, made)| made),
                })
            }
            Tx::WithdrawChange { kind, version } => Some(Vote::Withdrew { kind: *kind, version: *version }),
            _ => None,
        }
    }
}

/// A kind of change in words: name prices, not name_prices.
pub fn kind_words(kind: ChangeKind) -> String {
    kind.name().replace('_', " ")
}

impl Checked {
    pub fn of(proven: &Proven<Landed>) -> Checked {
        let Landed { signed, receipt, time } = &proven.value;
        let done = receipt.outcome != CommittedOutcome::Failed;
        let sender = signed.sender().to_string();
        let mut moves = Vec::new();

        if done {
            let coin = |from: Option<String>, to: Option<String>, amount: String| Move { token: None, from, to, amount };

            // A multisig's own transaction moves what it does.
            let tx = match &signed.tx {
                Tx::Multisig { tx, .. } => tx,
                tx => tx,
            };

            match tx {
                Tx::Transfer { from, to, amount } => moves.push(coin(Some(from.to_string()), Some(to.to_string()), amount.to_u256().to_string())),
                Tx::Mint { to, amount } => moves.push(coin(None, Some(to.to_string()), amount.to_u256().to_string())),
                Tx::Burn { from, amount } => moves.push(coin(Some(from.to_string()), None, amount.to_u256().to_string())),
                Tx::ContractCall { to, amount, .. } if amount.to_u256() != u256::ZERO => {
                    moves.push(coin(Some(sender.clone()), Some(to.to_string()), amount.to_u256().to_string()));
                }
                _ => {}
            }

            // What contracts paid out, proven with the receipt, then tokens by their events.
            moves.extend(receipt.payments.iter().map(|paid| coin(Some(paid.from.to_string()), Some(paid.to.to_string()), paid.amount.to_string())));
            moves.extend(receipt.events.iter().filter_map(token_move));
        }

        Checked {
            height: proven.assurance.height,
            time: *time,
            final_: proven.assurance.final_,
            done,
            nonce: signed.nonce,
            action: action(&signed.tx, &receipt.outcome),
            moves,
            fee: receipt.fee.to_string(),
            events: receipt.events.iter().map(describe).collect(),
            sender,
            genesis: String::new(),
            vote: Vote::of(&signed.tx, &receipt.outcome),
        }
    }

    /// What it moved for an account of these addresses: each amount with whether it came in.
    pub fn for_account<'a>(&'a self, addresses: &'a [String]) -> impl Iterator<Item = (bool, &'a Move)> + 'a {
        let ours = move |party: &Option<String>| party.as_ref().is_some_and(|party| addresses.contains(party));

        self.moves.iter().filter_map(move |moved| match (ours(&moved.from), ours(&moved.to)) {
            (false, true) => Some((true, moved)),
            (true, false) => Some((false, moved)),
            // Between two of its addresses, or none: nothing moved, as far as it is concerned.
            _ => None,
        })
    }
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
    })
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
        (Tx::Bond { .. }, _) => "Bonded a voter".to_string(),
        (Tx::Unbond { .. }, _) => "Unbonded a voter".to_string(),
        (Tx::Withdraw { .. }, _) => "Withdrew a bond".to_string(),
        (Tx::Slash { .. }, _) => "Reported a double vote".to_string(),
        (Tx::ApproveSigners { version, .. }, _) => format!("Approved signers v{version}"),
        (Tx::WithdrawApproval { version }, _) => format!("Withdrew approval of signers v{version}"),
        // The approval that was the last one needed: its receipt says which decision it was, and
        // which version of its kind it made.
        (Tx::ApproveChange { .. }, CommittedOutcome::Decided { decision, kind, version }) => {
            format!("Decided change {decision} · {}, making {} v{version}", action(tx, &CommittedOutcome::Done), kind_words(*kind))
        }
        (Tx::ApproveChange { change: Change::Upgrade(upgrade), .. }, _) => {
            format!("Approved an upgrade: nodes older than {} stop at block {}", upgrade.least, upgrade.from)
        }
        (Tx::ApproveChange { change: Change::Name(name), .. }, _) => format!("Approved renaming the chain to {name}"),
        (Tx::ApproveChange { version, change }, _) => format!("Approved a {} change from v{version}", change.name().replace('_', " ")),
        (Tx::WithdrawChange { kind, version }, _) => format!("Withdrew approval of a {} change from v{version}", kind_words(*kind)),
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
        (Tx::Ethereum { raw }, _) => match chainv4::crypto::ethereum::decode(raw).ok().and_then(|ethereum| ethereum.to) {
            Some(to) => format!("Sent from an Ethereum wallet to {}", format::short(&chainv4::structs::hex::to_hex(&to))),
            None => "Deploy from an Ethereum wallet".to_string(),
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
        (Tx::Claim { to, .. }, _) => format!("Claimed mining pool earnings for {}", format::short(&to.to_string())),
    }
}

/// The name a call's selector stands for, among those the history knows.
fn function(func: &Hash) -> Option<&'static str> {
    KNOWN.into_iter().find(|name| Hasher::digest(HashType::Xxh3_64, name.as_bytes()) == *func)
}

/// An event in a line: its name and what it carries, addresses cut short.
pub fn describe(event: &Event) -> String {
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
        let signed = SignedTx::sign(tx, 4, FeeOffer::NONE, &Keypair::development());

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
        };
        let changes = [
            (Change::Fees(FeeConfig {
                block_gas_limit: 50_000_000,
                burn_permille: 500,
                schedule: ScheduleConfig::Weighted(Weighted { per_tx: 1_000, per_byte: 10, per_code_byte: 20, fuel_per_gas: 1 }),
                pricing: PricingConfig::Bid(Bid { min_price: u256::new(7) }),
            }), "Approved a fees change from v3"),
            (Change::Reward(Reward { initial: u256::new(25), halving_interval: Some(210_000) }), "Approved a reward change from v3"),
            (Change::Limits(BlockLimits { tx_limits: Limits { fuel: 10_000_000, memory: 64 << 20 }, max_block_bytes: None }), "Approved a limits change from v3"),
            (Change::Finality(FinalityConfig::Ffg(ffg)), "Approved a finality change from v3"),
            (Change::Finality(FinalityConfig::Depth(DepthConfig { depth: 6 })), "Approved a finality change from v3"),
            (Change::Upgrade(Upgrade { least: Version::new(0, 2, 0), from: 150_000 }), "Approved an upgrade: nodes older than 0.2.0 stop at block 150000"),
            (Change::Name("chainv4 mainnet".to_string()), "Approved renaming the chain to chainv4 mainnet"),
        ];

        for (change, said) in changes {
            let signed = SignedTx::sign(Tx::ApproveChange { version: 3, change: change.clone() }, 4, FeeOffer::NONE, &Keypair::development());
            let back: SignedTx = wincode::deserialize(&wincode::serialize(&signed).expect("encodes")).expect("decodes in a light build");

            assert_eq!(back, signed);
            assert!(matches!(&back.tx, Tx::ApproveChange { change: read, .. } if *read == change));
            assert_eq!(Checked::of(&landed(back.tx, CommittedOutcome::Done, Vec::new())).action, said);
        }
    }

    #[test]
    fn the_approval_that_decided_a_change_says_which_decision_and_version_of_its_kind_it_made() {
        let approve = Tx::ApproveChange { version: 4, change: Change::Name("second".to_string()) };
        let made = CommittedOutcome::Decided { decision: 5, kind: ChangeKind::Name, version: 5 };
        let decided = Checked::of(&landed(approve.clone(), made.clone(), Vec::new()));

        assert_eq!(decided.action, "Decided change 5 · Approved renaming the chain to second, making name v5");
        assert!(decided.done, "a decision did what it asked");
        assert_eq!(Checked::of(&landed(approve.clone(), CommittedOutcome::Done, Vec::new())).action, "Approved renaming the chain to second", "one that only counted");

        // The chain's count of decisions and the kind's version are apart: the ninth decision
        // made the reward v2.
        let reward = Tx::ApproveChange { version: 1, change: Change::Reward(chainv4::params::Reward { initial: u256::new(1), halving_interval: None }) };
        let ninth = Checked::of(&landed(reward, CommittedOutcome::Decided { decision: 9, kind: ChangeKind::Reward, version: 2 }, Vec::new()));
        assert_eq!(ninth.action, "Decided change 9 · Approved a reward change from v1, making reward v2");
        assert!(matches!(ninth.vote, Some(Vote::Approved { decided: Some(9), kind: ChangeKind::Reward, kind_version: Some(2), version: 1, .. })));

        // A multisig's approval that decided says so too.
        let multisig = Tx::Multisig { account: Address::Contract([6; 32]), tx: Box::new(approve), signatures: Vec::new() };
        let by_multisig = Checked::of(&landed(multisig, made, Vec::new()));

        assert!(by_multisig.action.starts_with("Decided change 5 · Approved renaming the chain to second, making name v5 · by the multisig contract:0x0606"), "{}", by_multisig.action);
        assert_eq!(
            by_multisig.vote,
            Some(Vote::Approved { version: 4, change: r#"{"name":"second"}"#.to_string(), decided: Some(5), kind: ChangeKind::Name, kind_version: Some(5) }),
            "kept for the Governance page"
        );
        assert_eq!(decided.vote, by_multisig.vote);
        assert_eq!(Checked::of(&landed(Tx::Burn { from: Address::Ed25519([1; 32]), amount: Amount::U128(1) }, CommittedOutcome::Done, Vec::new())).vote, None);
    }

    #[test]
    fn a_withdrawal_names_the_kind_it_took_an_approval_off() {
        let withdrew = Checked::of(&landed(Tx::WithdrawChange { kind: ChangeKind::NamePrices, version: 6 }, CommittedOutcome::Done, Vec::new()));

        assert_eq!(withdrew.action, "Withdrew approval of a name prices change from v6");
        assert_eq!(withdrew.vote, Some(Vote::Withdrew { kind: ChangeKind::NamePrices, version: 6 }));
        assert_eq!(
            action(&Tx::WithdrawChange { kind: ChangeKind::Consensus, version: 0 }, &CommittedOutcome::Done),
            "Withdrew approval of a consensus change from v0"
        );
    }

    #[test]
    fn votes_keep_their_kind_and_ones_kept_before_kinds_read_as_the_consensus() {
        let vote = Vote::Withdrew { kind: ChangeKind::Fees, version: 3 };
        let kept = serde_json::to_value(&vote).unwrap();

        assert_eq!(kept, serde_json::json!({ "Withdrew": { "kind": "fees", "version": 3 } }), "the kind by its name");
        assert_eq!(serde_json::from_value::<Vote>(kept).unwrap(), vote);

        let approved = Vote::Approved { version: 2, change: r#"{"name":"x"}"#.to_string(), decided: Some(7), kind: ChangeKind::Name, kind_version: Some(3) };
        assert_eq!(serde_json::from_value::<Vote>(serde_json::to_value(&approved).unwrap()).unwrap(), approved);

        // As a wallet kept them before each kind had a version of its own.
        let old_withdrawal: Vote = serde_json::from_value(serde_json::json!({ "Withdrew": { "version": 6 } })).unwrap();
        let old_approval: Vote = serde_json::from_value(serde_json::json!({ "Approved": { "version": 4, "change": "{}", "decided": 5 } })).unwrap();

        assert_eq!(old_withdrawal, Vote::Withdrew { kind: ChangeKind::Consensus, version: 6 });
        assert_eq!(old_approval, Vote::Approved { version: 4, change: "{}".to_string(), decided: Some(5), kind: ChangeKind::Consensus, kind_version: None });
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

        assert_eq!(checked.action, format!("Claimed mining pool earnings for {}", format::short(&miner.to_string())));
        assert!(!checked.action.contains("5000"), "all it earned is not what this claim paid");
    }

    #[test]
    fn events_read_in_a_line() {
        let event = Event {
            contract: Address::Contract([1; 32]),
            name: "Signed".to_string(),
            data: vec![TxCallArgumentType::Address(Address::Ed25519([0xab; 32])), TxCallArgumentType::String("gm".to_string()), TxCallArgumentType::I64(3)],
        };

        assert_eq!(describe(&event), "Signed ed25519:0xababab…abab \"gm\" 3");
    }
}
