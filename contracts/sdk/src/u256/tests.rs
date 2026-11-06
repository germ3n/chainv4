use super::*;
use alloc::{format, vec};

/// 2^n.
fn bit(n: u32) -> U256 {
    U256::ONE.shl(n)
}

/// A small deterministic generator, so the cross-checks below see many values.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;

        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A value with some of its top limbs zero, so every width of divisor turns up.
    fn u256(&mut self) -> U256 {
        let limbs = (self.next() % 4 + 1) as usize;
        let mut out = [0; 4];

        for limb in out.iter_mut().take(limbs) {
            *limb = self.next();
        }

        U256(out)
    }
}

#[test]
fn arithmetic_carries_across_limbs_and_stops_at_the_edges() {
    let big = U256::from(u128::MAX);
    let one = U256::from(1u64);
    let sum = big.checked_add(one).unwrap();

    assert_eq!(sum, U256([0, 0, 1, 0]));
    assert_eq!(sum.checked_sub(one), Some(big));
    assert_eq!(U256::MAX.checked_add(one), None);
    assert_eq!(U256::ZERO.checked_sub(one), None);
    assert!(sum > big && big > one && U256::MAX > sum);
    assert_eq!(U256::from_le_bytes(sum.to_le_bytes()), sum);
}

#[test]
fn decimals_read_and_write_as_lua_writes_them() {
    let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";

    assert_eq!(U256::ZERO.to_decimal(), "0");
    assert_eq!(U256::from(1_000_000_000_000_000_000u64).to_decimal(), "1000000000000000000");
    assert_eq!(U256::MAX.to_decimal(), max);
    assert_eq!(U256::from_decimal(max), Some(U256::MAX));
    assert_eq!(U256::from_decimal("340282366920938463463374607431768211456"), Some(U256([0, 0, 1, 0])));
    assert_eq!(U256::from_decimal("007"), Some(U256::from(7u64)));

    assert_eq!(U256::from_decimal("1_000_000"), Some(U256::from(1_000_000u64)), "grouped as Lua's u256 takes them");
    assert_eq!(U256::from_decimal("1_2_3"), Some(U256::from(123u64)));

    for bad in ["", "-1", "1.5", "0x10", " 1", "115792089237316195423570985008687907853269984665640564039457584007913129639936", "_1", "1_", "1__0", "_", "1_.5"] {
        assert_eq!(U256::from_decimal(bad), None, "{bad}");
    }
}

#[test]
fn multiplication_matches_u128_and_keeps_the_low_bits() {
    let mut numbers = Numbers(7);

    for _ in 0..1000 {
        let (a, b) = (numbers.next(), numbers.next());
        let expected = u128::from(a) * u128::from(b);

        assert_eq!(U256::from(a) * U256::from(b), U256::from(expected));
    }

    let high = U256::from(u128::MAX) * U256::from(u128::MAX);

    // (2^128 - 1)^2 = 2^256 - 2^129 + 1
    assert_eq!(high, U256([1, 0, u64::MAX - 1, u64::MAX]));
    assert_eq!(bit(128) * bit(127), bit(255));
    assert_eq!(bit(128).checked_mul(bit(128)), None);
    assert_eq!(U256::MAX.checked_mul(U256::from(2u64)), None);
    assert_eq!(U256::MAX.checked_mul(U256::ONE), Some(U256::MAX));
    assert_eq!(U256::MAX.checked_mul(U256::ZERO), Some(U256::ZERO));
    // (2^256 - 1)^2 is 1 modulo 2^256
    assert_eq!(U256::MAX.wrapping_mul(U256::MAX), U256::ONE);
    assert_eq!(U256::MAX.overflowing_mul(U256::MAX), (U256::ONE, true));
}

#[test]
fn division_matches_u128_and_reassembles_at_every_width() {
    let mut numbers = Numbers(11);

    for _ in 0..1000 {
        let a = u128::from(numbers.next()) << 64 | u128::from(numbers.next());
        let b = (u128::from(numbers.next()) << (numbers.next() % 64)).max(1);

        assert_eq!(U256::from(a) / U256::from(b), U256::from(a / b), "{a} / {b}");
        assert_eq!(U256::from(a) % U256::from(b), U256::from(a % b), "{a} % {b}");
    }

    for _ in 0..2000 {
        let (dividend, divisor) = (numbers.u256(), numbers.u256());

        if divisor.is_zero() {
            continue;
        }

        let (quotient, remainder) = (dividend / divisor, dividend % divisor);

        assert!(remainder < divisor, "{dividend:?} % {divisor:?}");
        assert_eq!(quotient * divisor + remainder, dividend, "{dividend:?} / {divisor:?}");
    }

    assert_eq!(U256::MAX / U256::MAX, U256::ONE);
    assert_eq!(U256::MAX / bit(255), U256::ONE);
    assert_eq!(U256::MAX % bit(255), bit(255) - 1);
    assert_eq!(U256::from(5u64) / U256::from(7u64), U256::ZERO);
    assert_eq!(U256::from(5u64).checked_div(U256::ZERO), None);
    assert_eq!(U256::from(5u64).checked_rem(U256::ZERO), None);
}

#[test]
fn powers_stop_at_the_edges() {
    let ten = U256::from(10u64);

    assert_eq!(U256::from(2u64).pow(255), bit(255));
    assert_eq!(U256::from(2u64).checked_pow(256), None);
    // 10^77 fits, 10^78 does not
    assert_eq!(ten.checked_pow(77).map(U256::to_decimal), Some(format!("1{}", "0".repeat(77))));
    assert_eq!(ten.checked_pow(78), None);
    assert_eq!(U256::ZERO.pow(0), U256::ONE);
    assert_eq!(U256::ZERO.pow(5), U256::ZERO);
    assert_eq!(U256::ONE.pow(u32::MAX), U256::ONE);
    assert_eq!(U256::from(3u64).pow(5), U256::from(243u64));
    assert_eq!(U256::from(2u64).wrapping_pow(256), U256::ZERO);
    assert_eq!(U256::from(3u64).wrapping_pow(300), U256::from(3u64).pow(150).wrapping_mul(U256::from(3u64).pow(150)));
    assert_eq!(ten.saturating_pow(78), U256::MAX);
    assert_eq!(ten.saturating_pow(2), U256::from(100u64));
}

#[test]
fn saturating_and_wrapping_stop_or_wrap() {
    let two = U256::from(2u64);

    assert_eq!(U256::MAX.saturating_add(U256::ONE), U256::MAX);
    assert_eq!(U256::ONE.saturating_add(U256::ONE), two);
    assert_eq!(U256::ONE.saturating_sub(two), U256::ZERO);
    assert_eq!(two.saturating_sub(U256::ONE), U256::ONE);
    assert_eq!(U256::MAX.saturating_mul(two), U256::MAX);
    assert_eq!(U256::MAX.wrapping_add(U256::ONE), U256::ZERO);
    assert_eq!(U256::ZERO.wrapping_sub(U256::ONE), U256::MAX);
    assert_eq!(bit(255).wrapping_mul(two), U256::ZERO);
    assert_eq!(U256::MAX.overflowing_add(two), (U256::ONE, true));
    assert_eq!(U256::ONE.wrapping_neg(), U256::MAX);
    assert_eq!(U256::ZERO.wrapping_neg(), U256::ZERO);
    assert_eq!(U256::ZERO.checked_neg(), Some(U256::ZERO));
    assert_eq!(U256::ONE.checked_neg(), None);
    assert_eq!(U256::ZERO.overflowing_sub(two), (U256::MAX - 1, true));
}

#[test]
fn operators_take_u64_on_either_side() {
    let mut amount = U256::from(1000u64);

    assert_eq!(amount * 3, U256::from(3000u64));
    assert_eq!(3 * amount, U256::from(3000u64));
    assert_eq!(amount + 1, U256::from(1001u64));
    assert_eq!(1 + amount, U256::from(1001u64));
    assert_eq!(amount - 1, U256::from(999u64));
    assert_eq!(5000 - amount, U256::from(4000u64));
    assert_eq!(amount / 3, U256::from(333u64));
    assert_eq!(10_000 / amount, U256::from(10u64));
    assert_eq!(amount % 7, U256::from(6u64));
    assert_eq!(7 % amount, U256::from(7u64));

    amount += 5;
    amount -= 1;
    amount *= 2;
    amount /= 4;
    amount %= 300;
    amount += U256::ONE;

    assert_eq!(amount, U256::from(203u64));
}

#[test]
fn comparisons_take_u64_on_either_side() {
    let amount = U256::from(1000u64);

    assert!(amount > 999 && amount >= 1000 && amount <= 1000 && amount < 1001);
    assert!(999 < amount && 1000 <= amount && 1001 > amount);
    assert!(amount == 1000 && 1000 == amount && amount != 1001);
    assert!(U256::MAX > u64::MAX && u64::MAX < U256::MAX);
    assert_eq!(amount.max(U256::from(5u64)), amount);
    assert_eq!(amount.min(U256::from(5u64)), U256::from(5u64));
}

#[test]
fn conversions_to_smaller_types_check_the_range() {
    assert_eq!(u64::try_from(U256::from(u64::MAX)), Ok(u64::MAX));
    assert_eq!(u64::try_from(U256::from(u64::MAX) + 1), Err(OutOfRange));
    assert_eq!(u128::try_from(U256::from(u128::MAX)), Ok(u128::MAX));
    assert_eq!(u128::try_from(U256::from(u128::MAX) + 1), Err(OutOfRange));
    assert_eq!(u128::try_from(U256::MAX), Err(OutOfRange));
}

#[test]
fn shifts_and_leading_zeros() {
    assert_eq!(U256::ZERO.leading_zeros(), 256);
    assert_eq!(U256::ONE.leading_zeros(), 255);
    assert_eq!(U256::MAX.leading_zeros(), 0);
    assert_eq!(U256::from(u128::MAX).leading_zeros(), 128);
    assert_eq!(U256::ONE.shl(200).shr(200), U256::ONE);
    assert_eq!(U256::MAX.shl(64), U256([0, u64::MAX, u64::MAX, u64::MAX]));
    assert_eq!(U256::MAX.shr(70), U256([u64::MAX, u64::MAX, u64::MAX >> 6, 0]));
    assert_eq!(U256::MAX.shr(255), U256::ONE);
    assert_eq!(U256::from(0b1011u64).shl(130), U256([0, 0, 0b1011 << 2, 0]));
}

#[test]
fn text_formats_as_decimal() {
    assert_eq!(format!("{}", U256::from(42u64)), "42");
    assert_eq!(format!("{:>5}", U256::from(42u64)), "   42");
    assert_eq!(format!("{:05}", U256::from(42u64)), "00042");
    assert_eq!(format!("{:?}", U256::from(42u64)), "U256(42)");
    assert_eq!(format!("{}", U256::MAX), U256::MAX.to_decimal());
}

#[test]
#[should_panic(expected = "u256 overflow")]
fn addition_past_max_aborts() {
    let _ = U256::MAX + 1;
}

#[test]
#[should_panic(expected = "u256 underflow")]
fn subtraction_below_zero_aborts() {
    let _ = U256::ONE - 2;
}

#[test]
#[should_panic(expected = "u256 overflow")]
fn multiplication_past_max_aborts() {
    let _ = U256::MAX * U256::from(2u64);
}

#[test]
#[should_panic(expected = "division by zero")]
fn division_by_zero_aborts() {
    let _ = U256::ONE / 0;
}

#[test]
#[should_panic(expected = "division by zero")]
fn remainder_by_zero_aborts() {
    let _ = U256::ONE % U256::ZERO;
}

#[test]
#[should_panic(expected = "u256 overflow")]
fn powers_past_max_abort() {
    let _ = U256::from(2u64).pow(256);
}

#[test]
fn the_values_list_carries_them() {
    let values = vec![crate::Value::from(U256::MAX)];

    assert_eq!(crate::value::decode(&crate::value::encode(&values)), Some(values));
}
