//! Fees: what transactions pay for the gas they use, so spam is never free. Genesis sets every
//! part of it:
//!
//! - a gas schedule (schedule.rs), the scorer that says how much gas a transaction costs: for
//!   what it is, how big it is and the fuel it burns. `weighted` weighs those three; `contract`
//!   weighs them the same way, then lets a contract on chain say what the sender is charged.
//! - a pricing (pricing.rs), what each gas costs: `base_fee`, a base price that follows how full
//!   blocks are, as EIP-1559 has it, plus a tip from the sender; or `bid`, where each sender names
//!   its price above a minimum.
//! - the most gas one block may use, and what share of the base part of every fee is burned.
//!
//! A transaction names the most gas it may use and the most it pays per gas. That much is
//! reserved from the sender before it runs, and what it does not use comes back after. Of what
//! it pays, the base price times its gas is split: burn_permille of it is destroyed, the rest goes
//! to the block's producer, along with everything paid above the base price. Failed transactions
//! pay too.

use {
    crate::db::State,
    crate::structs::address::Address,
    crate::structs::tx::{FeeOffer, SignedTx, Tx},
    crate::structs::u256::u256,
    crate::vm::{Limits, Vm},
    r#macro::document,
    std::sync::Arc,
};

mod pricing;
mod schedule;
#[cfg(test)]
mod tests;

pub use crate::params::fees::{BaseFee, Bid, ContractScorer, FeeConfig, PricingConfig, ScheduleConfig, Weighted};
pub use crate::params::fees::TX_KINDS;

impl FeeConfig {
    #[document(
        parent = "FeeConfig",
        summary = "The development fees: the dev weighted schedule, a base fee starting at 1 gwei, blocks of up to 50 million gas, and half of the base part burned.",
    )]
    pub fn dev() -> FeeConfig {
        FeeConfig {
            block_gas_limit: 50_000_000,
            burn_permille: 500,
            schedule: ScheduleConfig::Weighted(Weighted::dev()),
            pricing: PricingConfig::BaseFee(BaseFee::dev()),
        }
    }

    #[document(parent = "FeeConfig", summary = "The fees these settings describe.")]
    pub fn build(&self) -> Fees {
        Fees {
            config: self.clone(),
            schedule: self.schedule.build(),
            pricing: self.pricing.build(),
        }
    }
}

#[document(
    summary = "The scorer: how much gas a transaction costs. Genesis picks the kind with ScheduleConfig; a new kind is a new implementation of this.",
    category = "Fees",
    see_also = "ScheduleConfig, Weighted, ContractScorer",
)]
pub trait GasSchedule: Send + Sync {
    #[document(parent = "GasSchedule", summary = "The kind's name, as genesis files spell it.")]
    fn name(&self) -> &'static str;

    #[document(
        parent = "GasSchedule",
        summary = "Gas a transaction costs whatever it does: for what it is and how big it is. Its gas limit must cover at least this.",
    )]
    fn intrinsic(&self, tx: &SignedTx) -> u64;

    #[document(parent = "GasSchedule", summary = "Gas for fuel a transaction's contracts burned.")]
    fn for_fuel(&self, fuel: u64) -> u64;

    #[document(
        parent = "GasSchedule",
        summary = "The fuel this much gas buys: what a transaction's contracts may burn with the gas its limit has left over its intrinsic cost.",
    )]
    fn fuel_for(&self, gas: u64) -> u64;

    #[document(
        parent = "GasSchedule",
        summary = "The gas a sender is charged for, once its transaction ran. By default the gas it used; a scorer may charge more or less. Never more than the gas limit, whatever this returns, and blocks count the gas used, not this.",
    )]
    fn charge(&self, scoring: &Scoring) -> u64 {
        scoring.gas
    }
}

#[document(
    summary = "What a scorer knows about a transaction that ran.",
    category = "Fees",
    see_also = "GasSchedule::charge",
)]
pub struct Scoring<'a> {
    #[document(summary = "The transaction.")]
    pub tx: &'a SignedTx,
    #[document(summary = "Its sender.")]
    pub sender: Address,
    #[document(summary = "Fuel its contracts burned.")]
    pub fuel_used: u64,
    #[document(summary = "The gas it used, intrinsic cost and fuel together, at most its limit.")]
    pub gas: u64,
    #[document(summary = "Whether it failed.")]
    pub failed: bool,
    #[document(summary = "The block's state after it ran, for a scorer that reads chain state. Nothing a scorer writes is kept.")]
    pub state: &'a Arc<State>,
    #[document(summary = "The contract engines, for a scorer that runs a contract.")]
    pub vm: &'a Arc<Vm>,
    #[document(summary = "The chain's limits on one call.")]
    pub limits: Limits,
    #[document(summary = "The block, which the scorer's call runs in.")]
    pub block: crate::vm::BlockAt,
}

#[document(
    summary = "What gas costs. Genesis picks the kind with PricingConfig; a new kind is a new implementation of this.",
    category = "Fees",
    see_also = "PricingConfig, BaseFee, Bid",
)]
pub trait Pricing: Send + Sync {
    #[document(parent = "Pricing", summary = "The kind's name, as genesis files spell it.")]
    fn name(&self) -> &'static str;

    #[document(parent = "Pricing", summary = "The base price of the genesis block.")]
    fn genesis_price(&self) -> u256;

    #[document(
        parent = "Pricing",
        summary = "The base price of the block after one with this base price and gas used.",
    )]
    fn next_price(&self, parent_price: u256, parent_gas: u64, block_gas_limit: u64) -> u256;

    #[document(
        parent = "Pricing",
        summary = "What a sender with this offer pays per gas in a block with this base price, or None when the offer is too low to be taken.",
    )]
    fn price(&self, base: u256, offer: &FeeOffer) -> Option<u256>;

    #[document(
        parent = "Pricing",
        summary = "The least base price any block can have: offers below it are turned away from the mempool.",
    )]
    fn floor(&self) -> u256;
}

#[document(
    summary = "A chain's fees, built from its FeeConfig: the scorer, the pricing, and how the two combine into what a transaction pays.",
    category = "Fees",
    see_also = "FeeConfig, GasSchedule, Pricing",
)]
pub struct Fees {
    config: FeeConfig,
    schedule: Box<dyn GasSchedule>,
    pricing: Box<dyn Pricing>,
}

impl std::fmt::Debug for Fees {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fees").field("config", &self.config).finish()
    }
}

#[document(
    summary = "What a transaction was charged, and where it went.",
    category = "Fees",
    see_also = "Fees::settle",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Charged {
    #[document(summary = "The gas it used, which counts against the block's limit.")]
    pub gas_used: u64,
    #[document(summary = "What the sender paid: the gas charged for at the price it paid.")]
    pub fee: u256,
    #[document(summary = "What the producer got.")]
    pub to_producer: u256,
    #[document(summary = "What was burned.")]
    pub burned: u256,
}

#[document(
    summary = "The parts of a fee offer a sender set itself; Fees::offer fills in the rest.",
    category = "Fees",
    see_also = "Fees::offer, FeeOffer",
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OfferChoice {
    #[document(summary = "The most gas it may use.")]
    pub gas_limit: Option<u64>,
    #[document(summary = "The most it pays per gas.")]
    pub max_price: Option<u256>,
    #[document(summary = "What it adds per gas for the producer, under a base fee.")]
    pub tip: Option<u256>,
}

impl Fees {
    #[document(parent = "Fees", summary = "The settings these fees came from.")]
    pub fn config(&self) -> &FeeConfig {
        &self.config
    }

    #[document(parent = "Fees", summary = "The scorer.")]
    pub fn schedule(&self) -> &dyn GasSchedule {
        &*self.schedule
    }

    #[document(parent = "Fees", summary = "The pricing.")]
    pub fn pricing(&self) -> &dyn Pricing {
        &*self.pricing
    }

    #[document(parent = "Fees", summary = "The most gas one block may use.")]
    pub fn block_gas_limit(&self) -> u64 {
        self.config.block_gas_limit
    }

    #[document(
        parent = "Fees",
        summary = "What a producer gains per gas from a transaction in a block with this base price: what it pays above the base. What producers order transactions by; 0 for an offer too low to be taken.",
    )]
    pub fn priority(&self, base: u256, offer: &FeeOffer) -> u256 {
        self.pricing.price(base, offer).map_or(u256::ZERO, |price| price - base)
    }

    #[document(
        parent = "Fees",
        summary = "A fee offer sure to cover a transaction sent now, keeping what its sender chose. A gas limit left open is the transaction's intrinsic cost, plus the fuel cap's worth for a call, deploy or upgrade, at most what a block holds. A max price left open is twice the base price plus the tip under a base fee, and the base price plus the tip when senders bid. A tip left open is 0.",
        params = {
            base = "The base price the next block will have.",
            tx_fuel = "The chain's fuel cap per transaction.",
            probe = "The transaction, signed by its sender with any offer. Every field of an offer has a fixed size, so it weighs what the real one will.",
            chosen = "What the sender set itself.",
        },
        see_also = "OfferChoice",
    )]
    pub fn offer(&self, base: u256, tx_fuel: u64, probe: &SignedTx, chosen: OfferChoice) -> FeeOffer {
        let tip = chosen.tip.unwrap_or_default();
        let max_price = match (chosen.max_price, &self.config.pricing) {
            (Some(max_price), _) => max_price,
            (None, PricingConfig::BaseFee(_)) => base * u256::from(2u64) + tip,
            (None, PricingConfig::Bid(_)) => base + tip,
        };
        let gas_limit = chosen.gas_limit.unwrap_or_else(|| {
            let fuel = match &probe.tx {
                Tx::ContractCall { .. } | Tx::EvmCall { .. } | Tx::Ethereum { .. } | Tx::Deploy { .. } | Tx::Upgrade { .. } => self.schedule.for_fuel(tx_fuel),
                // One allowance for a batch, as its steps share it, if any of them runs anything. A
                // member's approval of one only stores it.
                tx if !matches!(tx, Tx::ApproveMultisig { .. }) && tx.batch_steps().is_some_and(|steps| steps.iter().any(|step| matches!(step, Tx::ContractCall { .. } | Tx::EvmCall { .. } | Tx::Deploy { .. } | Tx::Upgrade { .. }))) => self.schedule.for_fuel(tx_fuel),
                _ => 0,
            };

            self.schedule.intrinsic(probe).saturating_add(fuel).min(self.config.block_gas_limit)
        });

        FeeOffer {
            gas_limit,
            max_price,
            tip,
        }
    }

    #[document(
        parent = "Fees",
        summary = "A gas limit for a transaction from what a dry run of it used: its intrinsic cost, plus a quarter more than the rest, for what it does by the time it lands. Never more than the limit offer leaves open, the fuel cap's worth, nor than a block holds.",
        params = {
            tx_fuel = "The chain's fuel cap per transaction.",
            probe = "The transaction, signed with any offer.",
            gas_used = "What the dry run used.",
        },
        see_also = "Fees::offer, dry_run",
    )]
    pub fn estimated(&self, tx_fuel: u64, probe: &SignedTx, gas_used: u64) -> u64 {
        let intrinsic = self.schedule.intrinsic(probe);
        let ran = gas_used.saturating_sub(intrinsic);
        let open = self.offer(u256::ZERO, tx_fuel, probe, OfferChoice::default()).gas_limit;

        intrinsic.saturating_add(ran).saturating_add(ran.div_ceil(4)).min(open)
    }

    #[document(
        parent = "Fees",
        summary = "Splits a fee once a transaction ran: the base price times the gas charged for is the base part, of which burn_permille burns and the rest goes to the producer, as does everything paid above it.",
        params = {
            base = "The block's base price.",
            price = "What the transaction paid per gas.",
            gas_used = "The gas it used.",
            charged = "The gas it is charged for.",
        },
    )]
    pub fn settle(&self, base: u256, price: u256, gas_used: u64, charged: u64) -> Charged {
        let fee = u256::from(charged) * price;
        let base_part = u256::from(charged) * base;
        // In two steps, so no part of it can overflow.
        let (thousand, permille) = (u256::from(1000u64), u256::from(self.config.burn_permille));
        let burned = base_part / thousand * permille + base_part % thousand * permille / thousand;

        Charged {
            gas_used,
            fee,
            to_producer: fee - burned,
            burned,
        }
    }
}
