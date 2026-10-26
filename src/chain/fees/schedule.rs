//! Gas schedules, the scorers: `weighted`, which weighs a transaction by what it is, how big it is
//! and the fuel it burns, and `contract`, which weighs it the same way and then asks a contract on
//! chain what to charge.

use {
    crate::structs::unset::UnsetList,
    super::{GasSchedule, Scoring},
    crate::crypto::hashable::Hashable,
    crate::db::State,
    crate::registry::contracts::Contracts,
    crate::structs::address::Address,
    crate::structs::hash::Hash,
    crate::structs::tx::{SignedTx, Tx, TxCallArgumentType},
    crate::structs::u256::u256,
    crate::vm::{CallContext, Limits, selector},
    r#macro::document,
    std::sync::Arc,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Which scorer a chain runs, and its parameters.",
    category = "Fees",
    see_also = "FeeConfig, GasSchedule",
    note = "Tags are pinned, so encodings match across builds.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum ScheduleConfig {
    #[document(summary = "Gas by what a transaction is, how big it is and the fuel it burns.")]
    #[wincode(tag = 0)]
    Weighted(Weighted),
    #[document(summary = "Weighted, then a contract says what the sender is charged.")]
    #[wincode(tag = 1)]
    Contract(ContractScorer),
}

impl Hashable for ScheduleConfig {
    const TAG_LEN: usize = 1;
}

impl ScheduleConfig {
    #[document(parent = "ScheduleConfig", summary = "The kind's name: weighted or contract.")]
    pub fn name(&self) -> &'static str {
        match self {
            ScheduleConfig::Weighted(_) => "weighted",
            ScheduleConfig::Contract(_) => "contract",
        }
    }

    #[document(parent = "ScheduleConfig", summary = "The scorer these settings describe.")]
    pub fn build(&self) -> Box<dyn GasSchedule> {
        match self {
            ScheduleConfig::Weighted(weighted) => Box::new(*weighted),
            ScheduleConfig::Contract(scorer) => Box::new(scorer.clone()),
        }
    }

    pub(super) fn problem(&self) -> Option<&'static str> {
        match self {
            ScheduleConfig::Weighted(weighted) => weighted.problem(),
            ScheduleConfig::Contract(scorer) => scorer.problem(),
        }
    }
}

#[document(
    summary = "A scorer that weighs a transaction: a fixed cost, a cost per byte of it, a cost per byte of code it deploys or upgrades to, which every node keeps, and gas for the fuel its contracts burn.",
    category = "Fees",
    see_also = "ScheduleConfig, GasSchedule",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Weighted {
    #[document(summary = "Gas every transaction costs.")]
    pub per_tx: u64,
    #[document(summary = "Gas per byte of the signed transaction, as it travels and is stored.")]
    pub per_byte: u64,
    #[document(summary = "Gas per byte of code a deploy or upgrade stores, on top of per_byte.")]
    pub per_code_byte: u64,
    #[document(summary = "How much fuel one gas buys. At least 1.")]
    pub fuel_per_gas: u64,
}

impl Hashable for Weighted {}

impl Weighted {
    #[document(
        parent = "Weighted",
        summary = "Development weights: 1,000 gas a transaction, 10 a byte, 20 more a byte of code, and a gas for each fuel.",
    )]
    pub fn dev() -> Weighted {
        Weighted {
            per_tx: 1_000,
            per_byte: 10,
            per_code_byte: 20,
            fuel_per_gas: 1,
        }
    }

    fn problem(&self) -> Option<&'static str> {
        match self.fuel_per_gas {
            0 => Some("fuel_per_gas must be at least 1"),
            _ => None,
        }
    }
}

impl GasSchedule for Weighted {
    fn name(&self) -> &'static str {
        "weighted"
    }

    fn intrinsic(&self, tx: &SignedTx) -> u64 {
        let bytes = tx.hashable_bytes_with_tag().len() as u64;
        let code = match &tx.tx {
            Tx::Deploy { code } | Tx::Upgrade { code, .. } => code.bytes().len() as u64,
            _ => 0,
        };

        self.per_tx
            .saturating_add(self.per_byte.saturating_mul(bytes))
            .saturating_add(self.per_code_byte.saturating_mul(code))
    }

    fn for_fuel(&self, fuel: u64) -> u64 {
        fuel.div_ceil(self.fuel_per_gas.max(1))
    }

    fn fuel_for(&self, gas: u64) -> u64 {
        gas.saturating_mul(self.fuel_per_gas)
    }
}

#[document(
    summary = "A scorer that weighs a transaction as Weighted does, then calls score on a contract to say what the sender is charged, for the kinds of transaction it names, or every kind. The contract can make some transactions cheaper or dearer: by sender, by what they touch, by anything in chain state. Until a contract lives there, or when it fails, the weighted figure stands.",
    category = "Fees",
    see_also = "ScheduleConfig, GasSchedule::charge",
    note = "score is called as score(sender: address, kind: string, target: address, bytes: u64, fuel: u64, gas: u64, failed: bool) and returns the gas to charge as a whole number. kind is the transaction's type as the API names it, such as transfer or call; target is what it is sent to, or the sender. Nothing the call writes or emits is kept, it gets fuel of its own, and what it returns is capped at the transaction's gas limit.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct ContractScorer {
    #[document(summary = "The weights for the gas a transaction uses, which bound what it may run and count against the block's limit.")]
    pub weights: Weighted,
    #[document(summary = "The contract whose score function says what to charge.")]
    pub contract: Address,
    #[document(summary = "Fuel each score call may burn. The chain pays for it, not the sender.")]
    pub fuel: u64,
    #[document(summary = "The kinds of transaction score is called for, as the API names them, such as call or deploy; any other kind is charged what the weights say, with no call. Empty scores every kind. Added after chains launched: empty, it adds nothing to genesis's encoding, so chains from before keep their ids.")]
    #[wincode(with = "UnsetList")]
    pub kinds: Vec<String>,
}

impl Hashable for ContractScorer {}

#[document(
    summary = "Every kind of transaction, as the API and a scorer's kind argument name them.",
    category = "Fees",
    see_also = "ContractScorer::kinds",
)]
pub const TX_KINDS: &[&str] = &[
    "transfer",
    "mint",
    "burn",
    "call",
    "deploy",
    "upgrade",
    "bond",
    "unbond",
    "withdraw",
    "slash",
    "approve_signers",
    "withdraw_approval",
    "approve_change",
    "withdraw_change",
];

impl ContractScorer {
    fn problem(&self) -> Option<&'static str> {
        if !matches!(self.contract, Address::Contract(_)) {
            return Some("the scorer must be a contract address");
        }

        if self.fuel == 0 {
            return Some("the scorer's fuel must be at least 1");
        }

        if self.kinds.iter().any(|kind| !TX_KINDS.contains(&kind.as_str())) {
            return Some("the scorer's kinds must be kinds of transaction, such as call or deploy");
        }

        let mut kinds = self.kinds.clone();
        kinds.sort();
        kinds.dedup();

        if kinds.len() != self.kinds.len() {
            return Some("the scorer's kinds name a kind twice");
        }

        self.weights.problem()
    }

    #[document(parent = "ContractScorer", summary = "Whether score is called for this kind of transaction.")]
    pub fn scores(&self, kind: &str) -> bool {
        self.kinds.is_empty() || self.kinds.iter().any(|named| named == kind)
    }
}

/// A transaction's type, as the API names it, and what it is sent to.
fn describe(tx: &Tx, sender: Address) -> (&'static str, Address) {
    match tx {
        Tx::Transfer { to, .. } => ("transfer", *to),
        Tx::Mint { to, .. } => ("mint", *to),
        Tx::Burn { .. } => ("burn", sender),
        Tx::ContractCall { to, .. } => ("call", *to),
        Tx::Deploy { .. } => ("deploy", sender),
        Tx::Upgrade { contract, .. } => ("upgrade", *contract),
        Tx::Bond { .. } => ("bond", sender),
        Tx::Unbond { .. } => ("unbond", sender),
        Tx::Withdraw { .. } => ("withdraw", sender),
        Tx::Slash { .. } => ("slash", sender),
        Tx::ApproveSigners { .. } => ("approve_signers", sender),
        Tx::WithdrawApproval { .. } => ("withdraw_approval", sender),
        Tx::ApproveChange { .. } => ("approve_change", sender),
        Tx::WithdrawChange { .. } => ("withdraw_change", sender),
    }
}

/// Gas from what score returned: a whole number, however the contract's engine writes one.
fn gas_from(values: &[TxCallArgumentType]) -> Option<u64> {
    match values.first()? {
        TxCallArgumentType::U64(gas) => Some(*gas),
        TxCallArgumentType::I64(gas) => u64::try_from(*gas).ok(),
        TxCallArgumentType::U128(gas) => Some(u64::try_from(*gas).unwrap_or(u64::MAX)),
        TxCallArgumentType::U256(gas) => Some(u64::try_from(*gas).unwrap_or(u64::MAX)),
        TxCallArgumentType::Number(gas) if gas.is_finite() && *gas >= 0.0 => Some(*gas as u64),
        _ => None,
    }
}

impl GasSchedule for ContractScorer {
    fn name(&self) -> &'static str {
        "contract"
    }

    fn intrinsic(&self, tx: &SignedTx) -> u64 {
        self.weights.intrinsic(tx)
    }

    fn for_fuel(&self, fuel: u64) -> u64 {
        self.weights.for_fuel(fuel)
    }

    fn fuel_for(&self, gas: u64) -> u64 {
        self.weights.fuel_for(gas)
    }

    fn charge(&self, scoring: &Scoring) -> u64 {
        let (kind, target) = describe(&scoring.tx.tx, scoring.sender);

        if !self.scores(kind) {
            return scoring.gas;
        }

        let args = [
            TxCallArgumentType::Address(scoring.sender),
            TxCallArgumentType::String(kind.to_string()),
            TxCallArgumentType::Address(target),
            TxCallArgumentType::U64(scoring.tx.hashable_bytes_with_tag().len() as u64),
            TxCallArgumentType::U64(scoring.fuel_used),
            TxCallArgumentType::U64(scoring.gas),
            TxCallArgumentType::Bool(scoring.failed),
        ];
        let context = CallContext {
            sender: scoring.sender,
            contract: self.contract,
            value: u256::ZERO,
        };
        let limits = Limits {
            fuel: self.fuel,
            memory: scoring.limits.memory,
        };
        // Its own layer, dropped after: a scorer only reads.
        let layer = State::new(scoring.state.clone());
        let call = Contracts::new(&Arc::new(layer)).call(scoring.vm, &context, &Hash::Xxh3_64(selector("score")), &args, limits);

        match call {
            Ok(outcome) => outcome.result.ok().and_then(|values| gas_from(&values)).unwrap_or(scoring.gas),
            Err(_) => scoring.gas,
        }
    }
}
