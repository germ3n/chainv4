//! Gas schedules, the scorers: `weighted`, which weighs a transaction by what it is, how big it is
//! and the fuel it burns, and `contract`, which weighs it the same way and then asks a contract on
//! chain what to charge.

use {
    super::{ContractScorer, GasSchedule, ScheduleConfig, Scoring, Weighted},
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
};

impl ScheduleConfig {
    #[document(parent = "ScheduleConfig", summary = "The scorer these settings describe.")]
    pub fn build(&self) -> Box<dyn GasSchedule> {
        match self {
            ScheduleConfig::Weighted(weighted) => Box::new(*weighted),
            ScheduleConfig::Contract(scorer) => Box::new(scorer.clone()),
        }
    }
}

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
}

impl GasSchedule for Weighted {
    fn name(&self) -> &'static str {
        "weighted"
    }

    fn intrinsic(&self, tx: &SignedTx) -> u64 {
        let bytes = tx.hashable_bytes_with_tag().len() as u64;
        let of_code = |tx: &Tx| match tx {
            Tx::Deploy { code } | Tx::Upgrade { code, .. } => code.bytes().len() as u64,
            _ => 0,
        };
        // A batch does the work of as many transactions as it has steps, and all the bytes it
        // carries are in its size. A member's approval only stores what it would do, so it counts
        // as one, as a multisig's own deploy always has.
        let (count, code) = match &tx.tx {
            Tx::Batch { steps } => (steps.len() as u64, steps.iter().map(of_code).sum()),
            Tx::Multisig { tx: inner, .. } if matches!(**inner, Tx::Batch { .. }) => (inner.what_it_runs().len() as u64, inner.what_it_runs().iter().map(of_code).sum()),
            tx => (1, of_code(tx)),
        };

        self.per_tx
            .saturating_mul(count)
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

impl ContractScorer {
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
        Tx::EvmCall { to, .. } => ("evm_call", *to),
        Tx::Deploy { .. } => ("deploy", sender),
        Tx::Upgrade { contract, .. } => ("upgrade", *contract),
        Tx::Remove { contract, .. } => ("remove", *contract),
        Tx::Bond { .. } => ("bond", sender),
        Tx::Unbond { .. } => ("unbond", sender),
        Tx::Withdraw { .. } => ("withdraw", sender),
        Tx::SetAttester { .. } => ("set_attester", sender),
        Tx::SetRestake { .. } => ("set_restake", sender),
        Tx::Slash { .. } => ("slash", sender),
        Tx::ApproveSigners { .. } => ("approve_signers", sender),
        Tx::WithdrawApproval { .. } => ("withdraw_approval", sender),
        Tx::VoteChange { .. } => ("vote_change", sender),
        Tx::ClearVote { .. } => ("clear_vote", sender),
        Tx::SetDirectory { .. } => ("set_directory", sender),
        Tx::HandDirectory { to } => ("hand_directory", to.unwrap_or(sender)),
        Tx::Name { .. } => ("name", sender),
        Tx::PrimaryName { .. } => ("primary_name", sender),
        Tx::Ethereum { raw } => ("ethereum", crate::crypto::ethereum::decode(raw).ok().and_then(|ethereum| ethereum.to).map_or(sender, Address::Evm)),
        // A multisig's own transaction is scored as what it does.
        Tx::Multisig { tx, .. } => describe(tx, sender),
        Tx::CreateMultisig { .. } => ("create_multisig", sender),
        Tx::ChangeMultisig { .. } => ("change_multisig", sender),
        Tx::ApproveMultisig { account, .. } => ("approve_multisig", *account),
        Tx::WithdrawMultisig { account, .. } => ("withdraw_multisig", *account),
        Tx::Claim { to, .. } => ("claim", *to),
        Tx::Anchor { .. } => ("anchor", sender),
        Tx::Batch { .. } => ("batch", sender),
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

        // A scorer that weighs a kind weighs a batch holding one, so a batch is no way around it.
        let scored = match kind {
            "batch" => self.scores(kind) || scoring.tx.tx.what_it_runs().iter().any(|step| self.scores(describe(step, scoring.sender).0)),
            kind => self.scores(kind),
        };

        if !scored {
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
            block: scoring.block,
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
