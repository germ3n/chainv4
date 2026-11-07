//! The DEX's arithmetic, for the apps that quote it: contracts/lua/dex.lua's pools, and the EVM's
//! DexPair, keep the same constant product, so one formula prices both, each pool at its own fee
//! tier. A trade's route is the best of every pool that holds both assets and every path through
//! one middle pool, at any tiers; a split is the best way to divide an amount between the two DEXes
//! (contracts/lua/router.lua runs it); its least is what slippage allows; its price impact is how
//! far its rate falls below the pools' own.

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

#[document(summary = "The middle fee tier a chain offers by default, in hundredths of a percent: 30, Uniswap V2's 0.3%. The tiers in force are GET /status's dex_tiers; each pool charges its own.", category = "DEX", see_also = "DexSettings")]
pub const DEFAULT_FEE_BPS: u32 = 30;

#[document(summary = "Which DEX a pool is on: the native one, dex.lua, or the EVM's, Dex.sol.", category = "DEX", see_also = "DexPool")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Venue {
    #[document(summary = "contracts/lua/dex.lua.")]
    #[default]
    Native,
    #[document(summary = "Dex.sol's pairs, through its DexRouter.")]
    Evm,
}

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

#[document(summary = "A pool: its two assets, its fee tier, what it holds of each, its share token and which DEX it is on.", category = "DEX", see_also = "best_route")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DexPool {
    pub a: Address,
    pub b: Address,
    #[document(summary = "Its fee, in hundredths of a percent, fixed when it opened.")]
    pub fee_bps: u32,
    pub reserve_a: u256,
    pub reserve_b: u256,
    #[document(summary = "Its share token: a clone of lp_token.lua, or the EVM pair itself.")]
    pub share: Address,
    #[document(summary = "Which DEX it is on.")]
    pub venue: Venue,
}

impl DexPool {
    #[document(
        parent = "DexPool",
        summary = "A pool from what the native DEX's pool_at answers, as text: a, b, its fee tier, what it holds of each, and its share token.",
    )]
    pub fn from_values(values: &[String]) -> Result<DexPool, String> {
        let [a, b, fee, reserve_a, reserve_b, share] = values else {
            return Err("an answer the DEX does not give".to_string());
        };
        let address = |text: &str| text.parse::<Address>().map_err(|_| format!("{text} is not an address"));
        let number = |text: &str| u256::from_str_radix(text, 10).map_err(|_| format!("{text} is not an amount"));
        let fee_bps = fee.parse().map_err(|_| format!("{fee} is not a fee tier"))?;

        Ok(DexPool { a: address(a)?, b: address(b)?, fee_bps, reserve_a: number(reserve_a)?, reserve_b: number(reserve_b)?, share: address(share)?, venue: Venue::Native })
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

#[document(summary = "A trade's way through the pools of one DEX, and what it pays.", category = "DEX", see_also = "best_route")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TradeRoute {
    #[document(summary = "The first pool's fee tier, in hundredths of a percent.")]
    pub fee_bps: u32,
    #[document(summary = "The asset traded through on the way, and the second pool's tier, for a trade through two pools.")]
    pub middle: Option<(Address, u32)>,
    #[document(summary = "What comes out.")]
    pub out: u256,
    #[document(summary = "How far the trade's rate falls below the pools' own before it, in parts in 10,000: the fees and the trade's own weight on the price.")]
    pub impact: u32,
}

#[document(
    summary = "The best route for a trade through pools of one DEX: through any pool that holds both assets, at any tier, or through one middle asset, two pools at any tiers, whichever pays out more. None when no pools join them.",
    category = "DEX",
)]
pub fn best_route(pools: &[DexPool], from: &Address, to: &Address, amount: u256) -> Option<TradeRoute> {
    let step = |pool: &DexPool, x: &Address, amount: u256| {
        let (held_in, held_out) = pool.facing(x)?;

        Some((amount_out(amount, held_in, held_out, pool.fee_bps)?, held_in, held_out))
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

    for pool in pools.iter().filter(|pool| pool.pairs(from, to)) {
        if let Some((out, held_in, held_out)) = step(pool, from, amount) {
            consider(TradeRoute { fee_bps: pool.fee_bps, middle: None, out, impact: impact(out, spot(amount.as_f64(), held_in, held_out)) });
        }
    }

    for first in pools.iter().filter(|pool| pool.facing(from).is_some()) {
        let Some(middle) = first.other(from).filter(|middle| middle != to) else {
            continue;
        };
        let Some((between, first_in, first_out)) = step(first, from, amount) else {
            continue;
        };

        for second in pools.iter().filter(|pool| pool.pairs(&middle, to)) {
            let Some((out, second_in, second_out)) = step(second, &middle, between) else {
                continue;
            };
            let rate = spot(spot(amount.as_f64(), first_in, first_out), second_in, second_out);
            consider(TradeRoute { fee_bps: first.fee_bps, middle: Some((middle, second.fee_bps)), out, impact: impact(out, rate) });
        }
    }

    best
}

#[document(summary = "A trade divided between the two DEXes: how much goes each way, by which route, and what both pay together.", category = "DEX", see_also = "best_split")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    #[document(summary = "The part traded on the native DEX, and its route there.")]
    pub native: Option<(u256, TradeRoute)>,
    #[document(summary = "The part traded on the EVM's, and its route there, in its assets: WDEV for the coin.")]
    pub evm: Option<(u256, TradeRoute)>,
    #[document(summary = "What both pay together.")]
    pub out: u256,
}

#[document(
    summary = "The best way to trade an amount across both DEXes: all of it on one, or divided between them, in twentieths and then hundredths around the best twentieth, whichever pays out the most together. evm_of names each native asset's side on the EVM, WDEV for the coin, an ERC-20 itself, None for a token the EVM has not; with none for either end, the trade stays native. None when neither DEX joins them.",
    category = "DEX",
    see_also = "best_route, Split",
)]
pub fn best_split(pools: &[DexPool], from: &Address, to: &Address, amount: u256, evm_of: impl Fn(&Address) -> Option<Address>) -> Option<Split> {
    let (native, evm): (Vec<DexPool>, Vec<DexPool>) = pools.iter().cloned().partition(|pool| pool.venue == Venue::Native);
    let evm_ends = evm_of(from).zip(evm_of(to));
    let on_native = |part: u256| (part != u256::ZERO).then(|| best_route(&native, from, to, part)).flatten();
    let on_evm = |part: u256| (part != u256::ZERO).then(|| evm_ends.and_then(|(x, y)| best_route(&evm, &x, &y, part))).flatten();
    let at = |parts: u64, of: u64| {
        let native_part = amount.checked_mul(u256::from(parts)).map_or(u256::ZERO, |all| all / u256::from(of));
        let evm_part = amount - native_part;
        let (n, e) = (on_native(native_part), on_evm(evm_part));

        // Either part with no route there pays nothing, and the split is no split.
        match ((native_part == u256::ZERO) == n.is_none(), (evm_part == u256::ZERO) == e.is_none()) {
            (true, true) => {
                let out = n.as_ref().map_or(u256::ZERO, |route| route.out) + e.as_ref().map_or(u256::ZERO, |route| route.out);
                Some(Split { native: n.map(|route| (native_part, route)), evm: e.map(|route| (evm_part, route)), out })
            }
            _ => None,
        }
    };
    let better = |best: Option<Split>, next: Option<Split>| match (best, next) {
        (Some(best), Some(next)) => Some(if next.out > best.out { next } else { best }),
        (best, next) => best.or(next),
    };

    let mut best: Option<(u64, Split)> = None;

    for parts in 0..=20 {
        if let Some(split) = at(parts, 20) {
            best = match best {
                Some((_, ref kept)) if kept.out >= split.out => best,
                _ => Some((parts, split)),
            };
        }
    }

    let (twentieth, mut chosen) = best.map(|(parts, split)| (parts, Some(split)))?;

    // Finer, around it: a hundredth at a time between its neighbours.
    for parts in (twentieth * 5).saturating_sub(4)..=(twentieth * 5 + 4).min(100) {
        chosen = better(chosen, at(parts, 100));
    }

    chosen
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

#[document(
    summary = "A native pool's time-weighted price from two observations of one side's cumulative price, as pool() returns them: what that side was worth in the other on average between them, in raw units, the cumulative prices being 10^18 fixed point times milliseconds and wrapping, as Uniswap V2's do.",
    params = { then = "The earlier observation.", now = "The later one.", elapsed_ms = "The milliseconds between the two blocks they were read at." },
    returns = "The average, or None for no time between them.",
    category = "DEX",
)]
pub fn twap(then: u256, now: u256, elapsed_ms: u64) -> Option<f64> {
    (elapsed_ms > 0).then(|| now.wrapping_sub(then).as_f64() / elapsed_ms as f64 / 1e18)
}

#[document(
    summary = "An EVM pair's price0CumulativeLast as of a later time, as Uniswap V2's currentCumulativePrices works it out: the reserves it held since its last change, token1's in token0's as 112-bit fixed point, times the seconds since, added, wrapping. price1's is the same with the reserves swapped.",
    params = { last = "price0CumulativeLast.", reserve0 = "getReserves's first.", reserve1 = "Its second.", last_s = "Its blockTimestampLast, in seconds.", now_s = "The time to work it out for, in seconds." },
    category = "DEX",
    see_also = "evm_twap",
)]
pub fn evm_cumulative(last: u256, reserve0: u256, reserve1: u256, last_s: u32, now_s: u64) -> u256 {
    // Dex.sol counts in 32 bits, wrapping, as V2 does.
    let elapsed = (now_s as u32).wrapping_sub(last_s);

    match reserve0 == u256::ZERO || elapsed == 0 {
        true => last,
        false => {
            let price: u256 = (reserve1 << 112u32) / reserve0;

            last.wrapping_add(price.wrapping_mul(u256::from(elapsed)))
        }
    }
}

#[document(
    summary = "An EVM pair's time-weighted price from two observations of a cumulative price: what token0 was worth in token1 on average between them, in raw units.",
    params = { then = "The earlier cumulative price.", now = "The later.", elapsed_s = "The seconds between them." },
    returns = "The average, or None for no time between them.",
    category = "DEX",
    see_also = "evm_cumulative, twap",
)]
pub fn evm_twap(then: u256, now: u256, elapsed_s: u64) -> Option<f64> {
    (elapsed_s > 0).then(|| now.wrapping_sub(then).as_f64() / elapsed_s as f64 / 2f64.powi(112))
}

#[document(
    summary = "router.lua's route arguments for a split, as chainv4 dex swap --split and the wallet send it: from, to, the whole amount, the least out, the deadline in milliseconds, the native part with its fee tier and middle, and the EVM part with its path and tiers ABI-encoded.",
    params = {
        split = "What best_split worked out.",
        least = "The least both parts may pay together.",
        deadline = "Until when, in milliseconds.",
        evm_token = "Each asset's ERC-20 on the EVM, WDEV for the coin.",
    },
    returns = "The arguments, or None when an asset on the EVM's path has no ERC-20.",
    category = "DEX",
    see_also = "best_split",
)]
#[cfg(feature = "abi")]
pub fn split_args(split: &Split, from: Address, to: Address, amount: u256, least: u256, deadline: u64, evm_token: impl Fn(&Address) -> Option<[u8; 20]>) -> Option<Vec<crate::structs::tx::TxCallArgumentType>> {
    use crate::abi::{AbiType, AbiValue, encode_values};
    use crate::structs::tx::TxCallArgumentType as Arg;

    let zero = Arg::U256(u256::ZERO);
    let (native_amount, fee, middle, middle_fee) = match &split.native {
        Some((part, route)) => (
            Arg::U256(*part),
            Arg::I64(i64::from(route.fee_bps)),
            route.middle.map_or(Arg::Nil, |(middle, _)| Arg::Address(middle)),
            route.middle.map_or(Arg::Nil, |(_, fee)| Arg::I64(i64::from(fee))),
        ),
        None => (zero.clone(), Arg::I64(i64::from(DEFAULT_FEE_BPS)), Arg::Nil, Arg::Nil),
    };
    let (evm_amount, evm_route) = match &split.evm {
        Some((part, route)) => {
            let path = [Some(from), route.middle.map(|(middle, _)| middle), Some(to)].into_iter().flatten().map(|asset| evm_token(&asset).map(AbiValue::Address)).collect::<Option<Vec<_>>>()?;
            let fees = [Some(route.fee_bps), route.middle.map(|(_, fee)| fee)].into_iter().flatten().map(|fee| AbiValue::Uint(u256::from(fee))).collect();
            let encoded = encode_values(&[AbiType::Array(Box::new(AbiType::Address)), AbiType::Array(Box::new(AbiType::Uint(256)))], &[AbiValue::Array(path), AbiValue::Array(fees)]).ok()?;

            (Arg::U256(*part), Arg::Bytes(encoded))
        }
        None => (zero, Arg::Bytes(Vec::new())),
    };

    Some(vec![
        Arg::Address(from),
        Arg::Address(to),
        Arg::U256(amount),
        Arg::U256(least),
        Arg::I64(i64::try_from(deadline).unwrap_or(i64::MAX)),
        native_amount,
        fee,
        middle,
        middle_fee,
        evm_amount,
        evm_route,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(a: u8, b: u8, reserve_a: u128, reserve_b: u128) -> DexPool {
        DexPool { a: Address::Contract([a; 32]), b: Address::Contract([b; 32]), fee_bps: DEFAULT_FEE_BPS, reserve_a: u256::new(reserve_a), reserve_b: u256::new(reserve_b), share: Address::Contract([a ^ b ^ 0x40; 32]), venue: Venue::Native }
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
        let route = best_route(&pools, &at(1), &at(3), u256::new(10_000)).unwrap();
        assert_eq!(route.middle, Some((at(2), DEFAULT_FEE_BPS)));
        assert!(route.out > u256::new(9_900), "{route:?}");
        assert!(route.impact >= 59 && route.impact < 70, "two fees, about 0.6%: {}", route.impact);

        // A small trade in a deep pool goes direct.
        let pools = [pool(1, 3, 10u128.pow(15), 10u128.pow(15)), pool(1, 2, 1_000, 1_000), pool(2, 3, 1_000, 1_000)];
        let direct = super::best_route(&pools, &at(3), &at(1), u256::new(10u128.pow(6))).unwrap();
        assert_eq!((direct.middle, direct.out), (None, amount_out(u256::new(10u128.pow(6)), u256::new(10u128.pow(15)), u256::new(10u128.pow(15)), DEFAULT_FEE_BPS).unwrap()));
        assert_eq!(direct.impact, 30, "the fee alone");
        assert_eq!(super::best_route(&pools, &at(1), &at(9), u256::new(1)), None, "nothing joins them");
    }

    #[test]
    fn the_cheaper_tier_of_two_pools_wins_when_both_are_as_deep() {
        let cheap = DexPool { fee_bps: 5, ..pool(1, 2, 10u128.pow(12), 10u128.pow(12)) };
        let dear = DexPool { fee_bps: 100, ..pool(1, 2, 10u128.pow(12), 10u128.pow(12)) };
        let route = best_route(&[dear, cheap], &at(1), &at(2), u256::new(10u128.pow(6))).unwrap();

        assert_eq!((route.fee_bps, route.impact), (5, 5));
    }

    #[test]
    fn a_split_divides_a_large_trade_between_two_equal_dexes_and_a_small_one_goes_whole() {
        let native = pool(1, 2, 10u128.pow(9), 10u128.pow(9));
        let evm = DexPool { a: Address::Evm([1; 20]), b: Address::Evm([2; 20]), venue: Venue::Evm, ..native.clone() };
        let evm_of = |asset: &Address| match asset {
            Address::Contract(bytes) => Some(Address::Evm([bytes[0]; 20])),
            _ => None,
        };
        let pools = [native.clone(), evm];

        // A tenth of either pool: halved, each half moves its pool's price half as far.
        let split = best_split(&pools, &at(1), &at(2), u256::new(10u128.pow(8)), evm_of).unwrap();
        let (native_part, evm_part) = (split.native.as_ref().unwrap().0, split.evm.as_ref().unwrap().0);
        assert_eq!(native_part, evm_part, "equal pools take equal parts");
        assert!(split.out > best_route(std::slice::from_ref(&native), &at(1), &at(2), u256::new(10u128.pow(8))).unwrap().out);

        // Without the EVM's side of an asset, it stays native, whole.
        let whole = best_split(&pools, &at(1), &at(2), u256::new(10u128.pow(8)), |_| None).unwrap();
        assert_eq!((whole.native.map(|(part, _)| part), whole.evm), (Some(u256::new(10u128.pow(8))), None));
        assert_eq!(best_split(&pools, &at(1), &at(9), u256::new(1), evm_of), None);
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
        let read = DexPool::from_values(&[at(1).to_string(), at(2).to_string(), "100".into(), "5".into(), "6".into(), at(3).to_string()]).unwrap();
        assert_eq!((read.fee_bps, read.reserve_b), (100, u256::new(6)));
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

    #[test]
    fn an_evm_pairs_time_weighted_price_comes_from_its_cumulative_prices_as_v2s_does() {
        // 2 of token1 to 1 of token0 since second 100: 50 seconds on, the cumulative grew by 2 a
        // second, and the average between the two is 2.
        let at_100: u256 = u256::new(7) << 112u32;
        let at_150 = evm_cumulative(at_100, u256::new(1_000), u256::new(2_000), 100, 150);

        assert_eq!(at_150 - at_100, (u256::new(2) << 112u32) * u256::new(50));
        assert_eq!(evm_twap(at_100, at_150, 50), Some(2.0));
        assert_eq!(evm_cumulative(at_100, u256::ZERO, u256::new(2_000), 100, 150), at_100, "nothing held, nothing summed");
        assert_eq!(evm_twap(at_100, at_100, 0), None);
    }
}
