//! A chain's fee settings: the gas schedule, the pricing, and the block's gas limit and burned
//! share. What they charge is chain::fees's.

use {
    crate::crypto::hashable::Hashable,
    crate::structs::address::Address,
    crate::structs::u256::{U256Le, u256},
    crate::structs::unset::UnsetList,
    r#macro::document,
    wincode::{SchemaRead, SchemaWrite},
};

#[document(
    summary = "Every parameter of a chain's fees, set in genesis.",
    category = "Fees",
    see_also = "Genesis, GasSchedule, Pricing, FeeOffer",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct FeeConfig {
    #[document(summary = "The most gas one block's transactions may use together. A transaction is only taken while its gas limit fits in what the block has left.")]
    pub block_gas_limit: u64,
    #[document(summary = "How much of the base part of every fee is burned, in thousandths: 0 sends it all to the producer, 1000 burns it all. What senders pay above the base price always goes to the producer.")]
    pub burn_permille: u16,
    #[document(summary = "The scorer: how much gas a transaction costs.")]
    pub schedule: ScheduleConfig,
    #[document(summary = "What gas costs.")]
    pub pricing: PricingConfig,
    #[document(summary = "Where vote rewards come from, or None for genesis's vote_reward_source: a change of fees may move them from minting to the reward pool or back, from the block after the one that decides it. The pool keeps what it holds either way.")]
    pub vote_rewards: Option<VoteRewardSource>,
    #[document(summary = "What host operations cost in fuel, and the EVM's native and names precompiles in gas, or None for HostCosts::DEFAULT. A change of fees may name them, from the block after the one that decides it.")]
    pub costs: Option<super::vm::HostCosts>,
    #[document(summary = "What Lua contracts' opcodes and library work cost in fuel, or None for LuaCosts::DEFAULT, the Luau fork's own. A change of fees may name them, from the block after the one that decides it.")]
    // Boxed: a kilobyte, which a transaction changing fees would otherwise carry inline.
    pub lua_costs: Option<Box<super::vm::LuaCosts>>,
}

impl Hashable for FeeConfig {}

#[document(
    summary = "Where the rewards for finality votes come from.",
    category = "Chain",
    see_also = "Genesis::vote_reward_source, FeeConfig::vote_rewards, REWARD_POOL",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum VoteRewardSource {
    #[document(summary = "New supply: every reward is minted.")]
    #[default]
    #[wincode(tag = 0)]
    Mint,
    #[document(summary = "The reward pool: the burned share of every fee goes to REWARD_POOL instead of vanishing, and voters are paid from it, each as much of its reward as the pool holds. Supply then grows by block rewards alone.")]
    #[wincode(tag = 1)]
    Fees,
}

impl Hashable for VoteRewardSource {
    const TAG_LEN: usize = 1;
}

impl VoteRewardSource {
    #[document(parent = "VoteRewardSource", summary = "The name genesis files and the API use: mint or fees.")]
    pub fn name(&self) -> &'static str {
        match self {
            VoteRewardSource::Mint => "mint",
            VoteRewardSource::Fees => "fees",
        }
    }
}

impl std::str::FromStr for VoteRewardSource {
    type Err = String;

    fn from_str(text: &str) -> Result<VoteRewardSource, String> {
        match text {
            "mint" => Ok(VoteRewardSource::Mint),
            "fees" => Ok(VoteRewardSource::Fees),
            other => Err(format!("unknown vote reward source {other}; use mint or fees")),
        }
    }
}



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
    summary = "Which pricing a chain runs, and its parameters.",
    category = "Fees",
    see_also = "FeeConfig, Pricing",
    note = "Tags are pinned, so encodings match across builds.",
)]
#[derive(Clone, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
#[wincode(tag_encoding = "u8")]
pub enum PricingConfig {
    #[document(summary = "A base price that follows how full blocks are, plus a tip.")]
    #[wincode(tag = 0)]
    BaseFee(BaseFee),
    #[document(summary = "Each sender bids its price, at least a minimum.")]
    #[wincode(tag = 1)]
    Bid(Bid),
}

impl Hashable for PricingConfig {
    const TAG_LEN: usize = 1;
}

#[document(
    summary = "A base price per gas that every transaction pays, set by how full the parent block was against a target of block_gas_limit / elasticity: fuller raises it, emptier lowers it, by at most 1 / change_denominator a block. Senders add a tip for the producer and name the most they pay; a transaction pays the base plus its tip, as far as its max_price allows, and is not taken while its max_price is under the base.",
    category = "Fees",
    see_also = "PricingConfig, FeeOffer",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct BaseFee {
    #[document(summary = "The base price of the genesis block.")]
    #[wincode(with = "U256Le")]
    pub initial: u256,
    #[document(summary = "The base price never falls below this. At least 1, or it could never rise again.")]
    #[wincode(with = "U256Le")]
    pub min: u256,
    #[document(summary = "The block gas limit divided by this is the target: blocks using more raise the base price. 2 aims for half-full blocks.")]
    pub elasticity: u64,
    #[document(summary = "The base price moves by at most this fraction of itself a block: 8 is at most an eighth.")]
    pub change_denominator: u64,
}

impl Hashable for BaseFee {}

#[document(
    summary = "Each sender bids its price per gas, which it pays whole, and blocks take the highest bids first. Bids under min_price are not taken; min_price is every block's base price, so that much of each fee is split by burn_permille, and the rest of a bid goes to the producer.",
    category = "Fees",
    see_also = "PricingConfig, FeeOffer",
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, SchemaWrite, SchemaRead)]
pub struct Bid {
    #[document(summary = "The lowest bid taken. 0 lets transactions in for nothing, with only the block gas limit to bound them.")]
    #[wincode(with = "U256Le")]
    pub min_price: u256,
}

impl Hashable for Bid {}

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
    "remove",
    "bond",
    "unbond",
    "withdraw",
    "slash",
    "approve_signers",
    "withdraw_approval",
    "vote_change",
    "clear_vote",
    "name",
    "primary_name",
    "ethereum",
    "create_multisig",
    "change_multisig",
    "approve_multisig",
    "withdraw_multisig",
    "claim",
    "anchor",
    "set_directory",
    "hand_directory",
];

impl FeeConfig {
    /// Settings no chain could charge with.
    pub fn problem(&self) -> Option<&'static str> {
        if self.block_gas_limit == 0 {
            return Some("block_gas_limit must be at least 1");
        }

        if self.burn_permille > 1000 {
            return Some("burn_permille must be at most 1000");
        }

        self.schedule
            .problem()
            .or_else(|| self.pricing.problem())
            .or_else(|| self.costs.as_ref().and_then(super::vm::HostCosts::problem))
            .or_else(|| self.lua_costs.as_deref().and_then(super::vm::LuaCosts::problem))
    }
}

impl ScheduleConfig {
    #[document(parent = "ScheduleConfig", summary = "The kind's name: weighted or contract.")]
    pub fn name(&self) -> &'static str {
        match self {
            ScheduleConfig::Weighted(_) => "weighted",
            ScheduleConfig::Contract(_) => "contract",
        }
    }

    pub fn problem(&self) -> Option<&'static str> {
        match self {
            ScheduleConfig::Weighted(weighted) => weighted.problem(),
            ScheduleConfig::Contract(scorer) => scorer.problem(),
        }
    }
}

impl Weighted {
    pub fn problem(&self) -> Option<&'static str> {
        match self.fuel_per_gas {
            0 => Some("fuel_per_gas must be at least 1"),
            _ => None,
        }
    }
}

impl ContractScorer {
    pub fn problem(&self) -> Option<&'static str> {
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
}

impl PricingConfig {
    #[document(parent = "PricingConfig", summary = "The kind's name: base_fee or bid.")]
    pub fn name(&self) -> &'static str {
        match self {
            PricingConfig::BaseFee(_) => "base_fee",
            PricingConfig::Bid(_) => "bid",
        }
    }

    pub fn problem(&self) -> Option<&'static str> {
        match self {
            PricingConfig::BaseFee(base_fee) => base_fee.problem(),
            PricingConfig::Bid(_) => None,
        }
    }
}

impl BaseFee {
    pub fn problem(&self) -> Option<&'static str> {
        if self.min == u256::ZERO {
            return Some("the base fee's min must be at least 1, or it could never rise again");
        }

        if self.initial < self.min {
            return Some("the base fee's initial price must be at least its min");
        }

        if self.elasticity == 0 || self.change_denominator == 0 {
            return Some("the base fee's elasticity and change_denominator must be at least 1");
        }

        None
    }
}
