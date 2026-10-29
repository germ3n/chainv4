//! Pricings, what gas costs: `base_fee`, a base price that rises when blocks are fuller than a
//! target and falls when they are emptier, plus a tip each sender adds, as in EIP-1559; and `bid`,
//! where each sender names its price, at least a minimum.

use {
    super::{BaseFee, Bid, Pricing, PricingConfig},
    crate::structs::tx::FeeOffer,
    crate::structs::u256::u256,
    r#macro::document,
    std::cmp::Ordering,
};

impl PricingConfig {
    #[document(parent = "PricingConfig", summary = "The pricing these settings describe.")]
    pub fn build(&self) -> Box<dyn Pricing> {
        match self {
            PricingConfig::BaseFee(base_fee) => Box::new(*base_fee),
            PricingConfig::Bid(bid) => Box::new(*bid),
        }
    }
}

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
