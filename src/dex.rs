//! The DEX's arithmetic, for the apps that quote it: contracts/lua/dex.lua's pools, and the EVM's
//! DexPair, keep the same constant product, so one formula prices both. A trade's route is the
//! best of the pool that holds both assets and every path through one middle pool; its least is
//! what slippage allows; its price impact is how far its rate falls below the pools' own.

use {
    crate::structs::address::Address,
    crate::structs::u256::u256,
    r#macro::document,
};

#[document(
    summary = "The chain's coin as a side of a pool: the zero contract address, contract:0x00…00, where no contract lives, and which a transaction may name, as it may not the zero key.",
    category = "DEX",
)]
pub const COIN: Address = Address::Contract([0; 32]);

#[document(summary = "What a trade leaves in the pool on a chain whose governance never set the DEX's fee, in hundredths of a percent: 30, Uniswap V2's 0.3%. The fee in force is GET /status's dex_fee_bps, which both DEXes charge.", category = "DEX", see_also = "DexSettings")]
pub const DEFAULT_FEE_BPS: u32 = 30;

#[document(
    summary = "What a trade of amount_in pays out of a pool holding reserve_in and reserve_out, as Uniswap V2's getAmountOut: the fee off what goes in, then the product kept, as both DEXes reckon it. None for nothing in, or a pool holding nothing.",
    category = "DEX",
    params = { fee_bps = "The chain's DEX fee, in hundredths of a percent." },
)]
pub fn amount_out(amount_in: u256, reserve_in: u256, reserve_out: u256, fee_bps: u32) -> Option<u256> {
    if amount_in == u256::ZERO || reserve_in == u256::ZERO || reserve_out == u256::ZERO || fee_bps > 10_000 {
        return None;
    }

    let with_fee = amount_in.checked_mul(u256::from(10_000 - u64::from(fee_bps)))?;

    with_fee.checked_mul(reserve_out)?.checked_div(reserve_in.checked_mul(u256::from(10_000u64))?.checked_add(with_fee)?)
}

#[document(summary = "What a trade of amount pays the pool it goes into, at a fee in hundredths of a percent.", category = "DEX")]
pub fn fee_of(amount: u256, fee_bps: u32) -> u256 {
    amount.checked_mul(u256::from(fee_bps)).map_or(u256::ZERO, |all| all / u256::from(10_000u64))
}

#[document(summary = "A pool: its two assets, what it holds of each, and its share token.", category = "DEX", see_also = "best_route")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DexPool {
    pub a: Address,
    pub b: Address,
    pub reserve_a: u256,
    pub reserve_b: u256,
    #[document(summary = "Its share token: a clone of lp_token.lua, or the EVM pair itself.")]
    pub share: Address,
}

impl DexPool {
    #[document(
        parent = "DexPool",
        summary = "A pool from what the DEX's pool_at answers, as text: a, b, what it holds of each, and its share token.",
    )]
    pub fn from_values(values: &[String]) -> Result<DexPool, String> {
        let [a, b, reserve_a, reserve_b, share] = values else {
            return Err("an answer the DEX does not give".to_string());
        };
        let address = |text: &str| text.parse::<Address>().map_err(|_| format!("{text} is not an address"));
        let number = |text: &str| u256::from_str_radix(text, 10).map_err(|_| format!("{text} is not an amount"));

        Ok(DexPool { a: address(a)?, b: address(b)?, reserve_a: number(reserve_a)?, reserve_b: number(reserve_b)?, share: address(share)? })
    }

    #[document(parent = "DexPool", summary = "What it holds of from, then of the other asset: None when it does not hold from.")]
    pub fn facing(&self, from: &Address) -> Option<(u256, u256)> {
        match from {
            from if *from == self.a => Some((self.reserve_a, self.reserve_b)),
            from if *from == self.b => Some((self.reserve_b, self.reserve_a)),
            _ => None,
        }
    }

    #[document(parent = "DexPool", summary = "Its other asset than one it holds.")]
    pub fn other(&self, one: &Address) -> Option<Address> {
        match one {
            one if *one == self.a => Some(self.b),
            one if *one == self.b => Some(self.a),
            _ => None,
        }
    }

    #[document(parent = "DexPool", summary = "Whether it trades these two, in either order.")]
    pub fn pairs(&self, x: &Address, y: &Address) -> bool {
        (self.a == *x && self.b == *y) || (self.a == *y && self.b == *x)
    }

    #[document(
        parent = "DexPool",
        summary = "How much of the other asset goes in with amount of one, at the pool's ratio: what adding to it takes. None for an empty pool, which takes any ratio, the price it opens at.",
    )]
    pub fn matching(&self, one: &Address, amount: u256) -> Option<u256> {
        let (held, other) = self.facing(one)?;

        (held != u256::ZERO).then(|| amount * other / held)
    }

    #[document(
        parent = "DexPool",
        summary = "The price of base in the other asset, as a float, for charts: what the pool holds of the other per base, scaled by both assets' decimals.",
    )]
    pub fn price(&self, base: &Address, base_decimals: u8, other_decimals: u8) -> Option<f64> {
        let (held, other) = self.facing(base)?;
        let float = |amount: u256, decimals: u8| amount.as_f64() / 10f64.powi(i32::from(decimals));

        (held != u256::ZERO).then(|| float(other, other_decimals) / float(held, base_decimals))
    }
}

#[document(summary = "A trade's way through the pools, and what it pays.", category = "DEX", see_also = "best_route")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeRoute {
    #[document(summary = "The asset traded through on the way, for a trade through two pools.")]
    pub middle: Option<Address>,
    #[document(summary = "What comes out.")]
    pub out: u256,
    #[document(summary = "How far the trade's rate falls below the pools' own before it, in parts in 10,000: the fee and the trade's own weight on the price.")]
    pub impact: u32,
}

#[document(
    summary = "The best route for a trade: through the pool that holds both assets, or through one middle pool, whichever pays out more. None when no pools join them.",
    category = "DEX",
)]
pub fn best_route(pools: &[DexPool], from: &Address, to: &Address, amount: u256, fee_bps: u32) -> Option<TradeRoute> {
    let pool = |x: &Address, y: &Address| pools.iter().find(|pool| pool.pairs(x, y));
    let step = |x: &Address, y: &Address, amount: u256| {
        let (held_in, held_out) = pool(x, y)?.facing(x)?;

        Some((amount_out(amount, held_in, held_out, fee_bps)?, held_in, held_out))
    };
    // The pools' own rate along a path, before the trade: what amount would buy with no fee and
    // no weight of its own.
    let spot = |amount: f64, held_in: u256, held_out: u256| amount * held_out.as_f64() / held_in.as_f64();
    let impact = |out: u256, spot: f64| match spot > 0.0 {
        true => ((1.0 - out.as_f64() / spot) * 10_000.0).clamp(0.0, 10_000.0).round() as u32,
        false => 10_000,
    };
    let mut best: Option<TradeRoute> = None;
    let mut consider = |route: TradeRoute| {
        if best.as_ref().is_none_or(|best| route.out > best.out) {
            best = Some(route);
        }
    };

    if let Some((out, held_in, held_out)) = step(from, to, amount) {
        consider(TradeRoute { middle: None, out, impact: impact(out, spot(amount.as_f64(), held_in, held_out)) });
    }

    let middles = pools.iter().filter_map(|pool| pool.other(from)).filter(|middle| middle != to);

    for middle in middles {
        let Some((between, first_in, first_out)) = step(from, &middle, amount) else {
            continue;
        };
        let Some((out, second_in, second_out)) = step(&middle, to, between) else {
            continue;
        };
        let rate = spot(spot(amount.as_f64(), first_in, first_out), second_in, second_out);
        consider(TradeRoute { middle: Some(middle), out, impact: impact(out, rate) });
    }

    best
}

#[document(
    summary = "The least a trade takes out, with slippage allowed in parts in 10,000: 50 for 0.5%. What the swap is sent with, so it fails rather than pay less.",
    category = "DEX",
)]
pub fn least_out(out: u256, slippage: u32) -> u256 {
    out - out * u256::from(slippage.min(10_000)) / u256::from(10_000u32)
}

#[document(
    summary = "What adding at most desired_a and desired_b to a pool holding reserve_a and reserve_b puts in: the largest pair at its ratio that fits both, rounded down as both DEXes round it (dex.lua's add_liquidity, the router's _amounts, Uniswap V2's _addLiquidity); for a pool holding nothing, the amounts as given, its opening price. None for a pool holding one side alone, or amounts too large to reckon.",
    category = "DEX",
    see_also = "least_out",
)]
pub fn add_amounts(reserve_a: u256, reserve_b: u256, desired_a: u256, desired_b: u256) -> Option<(u256, u256)> {
    if reserve_a == u256::ZERO && reserve_b == u256::ZERO {
        return Some((desired_a, desired_b));
    }

    // The same two steps as the contracts, in the same order, so the pair is what they take.
    let optimal_b = desired_a.checked_mul(reserve_b)?.checked_div(reserve_a)?;

    if optimal_b <= desired_b {
        return Some((desired_a, optimal_b));
    }

    let optimal_a = desired_b.checked_mul(reserve_a)?.checked_div(reserve_b)?;

    (optimal_a <= desired_a).then_some((optimal_a, desired_b))
}

#[document(
    summary = "What shares of a pool are worth: their part of what it holds of each asset, as removing them pays.",
    category = "DEX",
)]
pub fn shares_worth(shares: u256, supply: u256, reserve_a: u256, reserve_b: u256) -> Option<(u256, u256)> {
    (supply != u256::ZERO).then(|| (shares * reserve_a / supply, shares * reserve_b / supply))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(a: u8, b: u8, reserve_a: u128, reserve_b: u128) -> DexPool {
        DexPool { a: Address::Contract([a; 32]), b: Address::Contract([b; 32]), reserve_a: u256::new(reserve_a), reserve_b: u256::new(reserve_b), share: Address::Contract([a ^ b ^ 0x40; 32]) }
    }

    fn at(byte: u8) -> Address {
        Address::Contract([byte; 32])
    }

    #[test]
    fn a_trade_pays_as_uniswap_v2_does() {
        // 997/1000 of 100,000 into 10,000,000 against 20,000,000.
        assert_eq!(amount_out(u256::new(100_000), u256::new(10_000_000), u256::new(20_000_000), DEFAULT_FEE_BPS), Some(u256::new(197_431)));
        assert_eq!(amount_out(u256::ZERO, u256::new(1), u256::new(1), DEFAULT_FEE_BPS), None);
        assert_eq!(amount_out(u256::new(1), u256::ZERO, u256::new(1), DEFAULT_FEE_BPS), None);
        // A dearer fee pays less out; none pays the product alone.
        assert!(amount_out(u256::new(100_000), u256::new(10_000_000), u256::new(20_000_000), 100).unwrap() < u256::new(197_431));
        assert_eq!(amount_out(u256::new(100_000), u256::new(10_000_000), u256::new(20_000_000), 0), Some(u256::new(198_019)));
        assert_eq!(fee_of(u256::new(10_000), 30), u256::new(30));
    }

    #[test]
    fn the_route_is_the_one_that_pays_more_direct_or_through_a_middle() {
        // 1 to 3 direct is thin; through 2 it is deep.
        let pools = [pool(1, 3, 1_000, 1_000), pool(1, 2, 10u128.pow(9), 10u128.pow(9)), pool(2, 3, 10u128.pow(9), 10u128.pow(9))];
        let route = best_route(&pools, &at(1), &at(3), u256::new(10_000), DEFAULT_FEE_BPS).unwrap();
        assert_eq!(route.middle, Some(at(2)));
        assert!(route.out > u256::new(9_900), "{route:?}");
        assert!(route.impact >= 59 && route.impact < 70, "two fees, about 0.6%: {}", route.impact);

        // A small trade in a deep pool goes direct.
        let pools = [pool(1, 3, 10u128.pow(15), 10u128.pow(15)), pool(1, 2, 1_000, 1_000), pool(2, 3, 1_000, 1_000)];
        let direct = super::best_route(&pools, &at(3), &at(1), u256::new(10u128.pow(6)), DEFAULT_FEE_BPS).unwrap();
        assert_eq!((direct.middle, direct.out), (None, amount_out(u256::new(10u128.pow(6)), u256::new(10u128.pow(15)), u256::new(10u128.pow(15)), DEFAULT_FEE_BPS).unwrap()));
        assert_eq!(direct.impact, 30, "the fee alone");
        assert_eq!(super::best_route(&pools, &at(1), &at(9), u256::new(1), DEFAULT_FEE_BPS), None, "nothing joins them");
    }

    #[test]
    fn slippage_shares_and_prices_read_off_a_pool() {
        assert_eq!(least_out(u256::new(10_000), 50), u256::new(9_950));
        assert_eq!(least_out(u256::new(10_000), 20_000), u256::ZERO);
        assert_eq!(shares_worth(u256::new(10), u256::new(100), u256::new(500), u256::new(2_000)), Some((u256::new(50), u256::new(200))));

        let pool = pool(1, 2, 2 * 10u128.pow(18), 6_000_000);
        assert_eq!(pool.price(&at(1), 18, 6), Some(3.0), "3 of the six-decimal asset per one of the other");
        assert_eq!(pool.matching(&at(1), u256::new(10u128.pow(18))), Some(u256::new(3_000_000)));
        assert_eq!(pool.other(&at(2)), Some(at(1)));
        assert_eq!(DexPool::from_values(&[at(1).to_string(), at(2).to_string(), "5".into(), "6".into(), at(3).to_string()]).unwrap().reserve_b, u256::new(6));
    }

    #[test]
    fn adding_takes_the_largest_pair_at_the_pools_ratio_rounded_as_the_contracts_round() {
        let n = u256::new;

        // A pool at 1 to 4: too much b offered keeps all of a, too much a keeps all of b.
        assert_eq!(add_amounts(n(1_000_000), n(4_000_000), n(10_000), n(50_000)), Some((n(10_000), n(40_000))));
        assert_eq!(add_amounts(n(1_000_000), n(4_000_000), n(20_000), n(40_000)), Some((n(10_000), n(40_000))));
        // Off any round ratio, each step rounds down, as dex.lua and the router do.
        assert_eq!(add_amounts(n(7_000_003), n(3_000_017), n(1_000), n(1_000)), Some((n(1_000), n(1_000 * 3_000_017 / 7_000_003))));
        assert_eq!(add_amounts(n(3_000_017), n(7_000_003), n(1_000), n(1_000)), Some((n(1_000 * 3_000_017 / 7_000_003), n(1_000))));
        // A new pool opens at the amounts given; one holding a single side cannot be added to.
        assert_eq!(add_amounts(u256::ZERO, u256::ZERO, n(5), n(7)), Some((n(5), n(7))));
        assert_eq!(add_amounts(u256::ZERO, n(5), n(5), n(7)), None);
        assert_eq!(add_amounts(n(2), n(2), u256::MAX, n(1)), None, "too large to reckon");
    }
}
