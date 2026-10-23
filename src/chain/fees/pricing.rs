//! Pricings, what gas costs: `base_fee`, a base price that rises when blocks are fuller than a
//! target and falls when they are emptier, plus a tip each sender adds, as in EIP-1559; and `bid`,
//! where each sender names its price, at least a minimum.

use {
    super::Pricing,
    crate::crypto::hashable::Hashable,
    crate::structs::tx::FeeOffer,
    crate::structs::u256::{U256Le, u256},
    r#macro::document,
    std::cmp::Ordering,
    wincode::{SchemaRead, SchemaWrite},
};

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

impl PricingConfig {
    #[document(parent = "PricingConfig", summary = "The kind's name: base_fee or bid.")]
    pub fn name(&self) -> &'static str {
        match self {
            PricingConfig::BaseFee(_) => "base_fee",
            PricingConfig::Bid(_) => "bid",
        }
    }

    #[document(parent = "PricingConfig", summary = "The pricing these settings describe.")]
    pub fn build(&self) -> Box<dyn Pricing> {
        match self {
            PricingConfig::BaseFee(base_fee) => Box::new(*base_fee),
            PricingConfig::Bid(bid) => Box::new(*bid),
        }
    }

    pub(super) fn problem(&self) -> Option<&'static str> {
        match self {
            PricingConfig::BaseFee(base_fee) => base_fee.problem(),
            PricingConfig::Bid(_) => None,
        }
    }
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

impl BaseFee {
    #[document(
        parent = "BaseFee",
        summary = "Development settings: starting at 1 gwei, never under 0.01 gwei, aiming for half-full blocks, moving by at most an eighth a block.",
    )]
    pub fn dev() -> BaseFee {
        BaseFee {
            initial: u256::from(1_000_000_000u64),
            min: u256::from(10_000_000u64),
            elasticity: 2,
            change_denominator: 8,
        }
    }

    fn problem(&self) -> Option<&'static str> {
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

impl Pricing for BaseFee {
    fn name(&self) -> &'static str {
        "base_fee"
    }

    fn genesis_price(&self) -> u256 {
        self.initial
    }

    fn next_price(&self, parent_price: u256, parent_gas: u64, block_gas_limit: u64) -> u256 {
        let target = (block_gas_limit / self.elasticity).max(1);
        let step = |gap: u64| {
            parent_price.saturating_mul(u256::from(gap)) / u256::from(target) / u256::from(self.change_denominator)
        };
        let next = match parent_gas.cmp(&target) {
            Ordering::Equal => parent_price,
            // Always up by at least 1, so a small base price still climbs.
            Ordering::Greater => parent_price.saturating_add(step(parent_gas - target).max(u256::ONE)),
            Ordering::Less => parent_price.saturating_sub(step(target - parent_gas)),
        };

        next.max(self.min)
    }

    fn price(&self, base: u256, offer: &FeeOffer) -> Option<u256> {
        if offer.max_price < base {
            return None;
        }

        Some(offer.max_price.min(base.saturating_add(offer.tip)))
    }

    fn floor(&self) -> u256 {
        self.min
    }
}

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

impl Bid {
    #[document(parent = "Bid", summary = "Development settings: bids of at least 1 gwei.")]
    pub fn dev() -> Bid {
        Bid {
            min_price: u256::from(1_000_000_000u64),
        }
    }
}

impl Pricing for Bid {
    fn name(&self) -> &'static str {
        "bid"
    }

    fn genesis_price(&self) -> u256 {
        self.min_price
    }

    fn next_price(&self, _parent_price: u256, _parent_gas: u64, _block_gas_limit: u64) -> u256 {
        self.min_price
    }

    fn price(&self, base: u256, offer: &FeeOffer) -> Option<u256> {
        (offer.max_price >= base).then_some(offer.max_price)
    }

    fn floor(&self) -> u256 {
        self.min_price
    }
}
