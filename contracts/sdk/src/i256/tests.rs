use super::*;
use alloc::{format, vec};

const MIN: &str = "-57896044618658097711785492504343953926634992332820282019728792003956564819968";
const MAX: &str = "57896044618658097711785492504343953926634992332820282019728792003956564819967";

fn int(value: i64) -> I256 {
    I256::from(value)
}

/// i64 values, so sums and products are exact in i128 to check against.
struct Numbers(u64);

impl Numbers {
    fn next(&mut self) -> i64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let bits = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d);

        // mostly small values, where division and remainder have something to round
        match bits % 4 {
            0 => (bits as i64) >> 40,
            1 => (bits as i64) >> 56,
            _ => bits as i64,
        }
    }
}

#[test]
fn constants_and_their_bytes() {
    assert_eq!(I256::MIN.to_decimal(), MIN);
    assert_eq!(I256::MAX.to_decimal(), MAX);
    assert_eq!(I256::ZERO.to_decimal(), "0");
    assert_eq!(int(-1).to_le_bytes(), [0xff; 32]);
    assert_eq!(int(-5).to_le_bytes(), [[0xfb].as_slice(), &[0xff; 31]].concat()[..]);
    assert_eq!(I256::MIN.to_le_bytes()[31], 0x80);
    assert!(I256::MIN.to_le_bytes()[..31].iter().all(|byte| *byte == 0));
    assert_eq!(I256::from_le_bytes(I256::MIN.to_le_bytes()), I256::MIN);
    assert!(I256::MIN.is_negative() && !I256::MAX.is_negative() && !I256::ZERO.is_negative());
    assert_eq!(I256::MIN.unsigned_abs(), U256::from(2u64).pow(255));
    assert_eq!(I256::MAX.unsigned_abs(), U256::from(2u64).pow(255) - 1);
}

#[test]
fn arithmetic_matches_i128_exactly() {
    let mut numbers = Numbers(3);

    for _ in 0..5000 {
        let (a, b) = (numbers.next(), numbers.next());
        let (wide_a, wide_b) = (i128::from(a), i128::from(b));
        let exact = |value: I256| i128::try_from(value).unwrap();

        assert_eq!(exact(int(a) + int(b)), wide_a + wide_b, "{a} + {b}");
        assert_eq!(exact(int(a) - int(b)), wide_a - wide_b, "{a} - {b}");
        assert_eq!(exact(int(a) * int(b)), wide_a * wide_b, "{a} * {b}");
        assert_eq!(exact(-int(a)), -wide_a, "-{a}");
        assert_eq!(exact(int(a).abs()), wide_a.abs(), "|{a}|");
        assert_eq!(int(a).cmp(&int(b)), a.cmp(&b), "{a} <=> {b}");

        if b != 0 {
            // Rust's: toward zero, and the dividend's sign
            assert_eq!(exact(int(a) / int(b)), wide_a / wide_b, "{a} / {b}");
            assert_eq!(exact(int(a) % int(b)), wide_a % wide_b, "{a} % {b}");
        }
    }
}

#[test]
fn checked_methods_stop_at_the_edges() {
    let minus_one = int(-1);

    assert_eq!(I256::MAX.checked_add(I256::ONE), None);
    assert_eq!(I256::MIN.checked_add(minus_one), None);
    assert_eq!(I256::MIN.checked_add(I256::MAX), Some(minus_one));
    assert_eq!(I256::MIN.checked_sub(I256::ONE), None);
    assert_eq!(I256::MAX.checked_sub(minus_one), None);
    assert_eq!(I256::ZERO.checked_sub(I256::MAX), Some(I256::MIN + 1));
    assert_eq!(I256::MIN.checked_mul(minus_one), None);
    assert_eq!(I256::MAX.checked_mul(minus_one), Some(I256::MIN + 1));
    assert_eq!(I256::MIN.checked_mul(I256::ONE), Some(I256::MIN));
    assert_eq!(I256::MAX.checked_mul(int(2)), None);
    assert_eq!(I256::MIN.checked_div(minus_one), None);
    assert_eq!(I256::MIN.checked_div(I256::ONE), Some(I256::MIN));
    assert_eq!(int(5).checked_div(I256::ZERO), None);
    assert_eq!(I256::MIN.checked_rem(minus_one), None);
    assert_eq!(int(5).checked_rem(I256::ZERO), None);
    assert_eq!(I256::MIN.checked_rem(int(3)), Some(int(-2)));
    assert_eq!(I256::MIN.checked_neg(), None);
    assert_eq!(I256::MAX.checked_neg(), Some(I256::MIN + 1));
    assert_eq!(I256::ZERO.checked_neg(), Some(I256::ZERO));
    assert_eq!(I256::MIN.checked_abs(), None);
    assert_eq!(int(-2).checked_pow(255), Some(I256::MIN));
    assert_eq!(int(2).checked_pow(255), None);
    assert_eq!(int(2).checked_pow(254).map(|value| value.unsigned_abs()), Some(U256::from(2u64).pow(254)));
    assert_eq!(int(-3).checked_pow(3), Some(int(-27)));
    assert_eq!(int(-3).checked_pow(4), Some(int(81)));
    assert_eq!(int(-7).checked_pow(0), Some(I256::ONE));
}

#[test]
fn saturating_methods_go_the_right_way() {
    assert_eq!(I256::MAX.saturating_add(I256::ONE), I256::MAX);
    assert_eq!(I256::MIN.saturating_add(int(-1)), I256::MIN);
    assert_eq!(I256::MIN.saturating_sub(I256::ONE), I256::MIN);
    assert_eq!(I256::MAX.saturating_sub(int(-1)), I256::MAX);
    assert_eq!(I256::MIN.saturating_mul(int(-1)), I256::MAX);
    assert_eq!(I256::MIN.saturating_mul(int(2)), I256::MIN);
    assert_eq!(I256::MAX.saturating_mul(int(-2)), I256::MIN);
    assert_eq!(int(-10).saturating_pow(255), I256::MIN);
    assert_eq!(int(-10).saturating_pow(256), I256::MAX);
    assert_eq!(int(5).saturating_add(int(-7)), int(-2));
}

#[test]
fn wrapping_methods_wrap_in_twos_complement() {
    assert_eq!(I256::MAX.wrapping_add(I256::ONE), I256::MIN);
    assert_eq!(I256::MIN.wrapping_sub(I256::ONE), I256::MAX);
    assert_eq!(I256::MIN.wrapping_mul(int(-1)), I256::MIN);
    assert_eq!(I256::MIN.wrapping_neg(), I256::MIN);
    assert_eq!(int(5).wrapping_neg(), int(-5));
    assert_eq!(I256::MIN.saturating_neg(), I256::MAX);
    assert_eq!(int(-5).saturating_neg(), int(5));
    assert_eq!(I256::MAX.wrapping_mul(int(2)), int(-2));
    assert_eq!(int(-2).wrapping_pow(256), I256::ZERO);
    assert_eq!(int(-2).wrapping_pow(255), I256::MIN);
    assert_eq!(int(-3).wrapping_mul(int(7)), int(-21));
}

#[test]
fn operators_take_i64_on_either_side() {
    let mut debt = int(-100);

    assert_eq!(debt * 3, int(-300));
    assert_eq!(3 * debt, int(-300));
    assert_eq!(debt + 1, int(-99));
    assert_eq!(1 - debt, int(101));
    assert_eq!(debt / 7, int(-14));
    assert_eq!(-700 / debt, int(7));
    assert_eq!(debt % 7, int(-2));
    assert_eq!(-debt, int(100));

    debt += 10;
    debt -= -5;
    debt *= -2;
    debt /= 3;
    debt %= 40;

    assert_eq!(debt, int(16));
}

#[test]
fn comparisons_take_i64_on_either_side() {
    let debt = int(-100);

    assert!(debt < 0 && debt <= -100 && debt > -101 && debt >= -100);
    assert!(0 > debt && -100 >= debt && -101 < debt);
    assert!(debt == -100 && -100 == debt && debt != 100);
    assert!(I256::MIN < i64::MIN && i64::MAX < I256::MAX);
    assert!(int(-1) < I256::ZERO && I256::ZERO < I256::ONE && I256::MIN < I256::MAX);
    assert_eq!(debt.max(int(5)), int(5));
    assert_eq!(debt.min(int(5)), debt);
}

#[test]
fn conversions_keep_the_value_or_say_it_does_not_fit() {
    assert_eq!(i64::try_from(int(i64::MIN)), Ok(i64::MIN));
    assert_eq!(i64::try_from(int(i64::MAX) + 1), Err(OutOfRange));
    assert_eq!(i64::try_from(int(i64::MIN) - 1), Err(OutOfRange));
    assert_eq!(i128::try_from(I256::from(i128::MIN)), Ok(i128::MIN));
    assert_eq!(i128::try_from(I256::from(i128::MAX)), Ok(i128::MAX));
    assert_eq!(i128::try_from(I256::from(i128::MIN) - 1), Err(OutOfRange));
    assert_eq!(i128::try_from(I256::MAX), Err(OutOfRange));
    assert_eq!(I256::from(u128::MAX).to_decimal(), u128::MAX.to_string());
    assert_eq!(I256::from(u64::MAX).to_decimal(), u64::MAX.to_string());
    assert_eq!(I256::try_from(U256::from(2u64).pow(255) - 1), Ok(I256::MAX));
    assert_eq!(I256::try_from(U256::from(2u64).pow(255)), Err(OutOfRange));
    assert_eq!(U256::try_from(I256::MAX), Ok(U256::from(2u64).pow(255) - 1));
    assert_eq!(U256::try_from(int(-1)), Err(OutOfRange));
}

#[test]
fn decimals_read_and_write_with_a_sign() {
    assert_eq!(I256::from_decimal(MIN), Some(I256::MIN));
    assert_eq!(I256::from_decimal(MAX), Some(I256::MAX));
    assert_eq!(I256::from_decimal("-0"), Some(I256::ZERO));
    assert_eq!(I256::from_decimal("-42"), Some(int(-42)));
    assert_eq!(I256::from_decimal("007"), Some(int(7)));
    assert_eq!(I256::from_decimal("-1_000"), Some(int(-1000)));
    assert_eq!(I256::from_decimal("-_1"), None);
    assert_eq!(I256::from_decimal("1_"), None);

    for bad in ["", "-", "--1", "+1", "1.5", "0x10", " 1", "-57896044618658097711785492504343953926634992332820282019728792003956564819969", "57896044618658097711785492504343953926634992332820282019728792003956564819968"] {
        assert_eq!(I256::from_decimal(bad), None, "{bad}");
    }

    assert_eq!(format!("{}", int(-42)), "-42");
    assert_eq!(format!("{:>5}", int(-42)), "  -42");
    assert_eq!(format!("{:05}", int(-42)), "-0042");
    assert_eq!(format!("{:?}", int(-42)), "I256(-42)");
}

#[test]
fn the_values_list_carries_them_on_tag_12() {
    let values = vec![crate::Value::from(int(-5))];
    let encoded = crate::value::encode(&values);

    // the count, then the node's tag for TxCallArgumentType::I256 and 32 bytes
    assert_eq!(encoded[..9], [1, 0, 0, 0, 0, 0, 0, 0, 12]);
    assert_eq!(encoded[9..], int(-5).to_le_bytes());
    assert_eq!(crate::value::decode(&encoded), Some(values));
}

#[test]
fn values_read_as_i256_from_any_integer_that_fits() {
    use crate::Value;

    assert_eq!(Value::I64(-5).as_i256(), Some(int(-5)));
    assert_eq!(Value::I128(i128::MIN).as_i256(), Some(I256::from(i128::MIN)));
    assert_eq!(Value::U64(5).as_i256(), Some(int(5)));
    assert_eq!(Value::U128(u128::MAX).as_i256(), Some(I256::from(u128::MAX)));
    assert_eq!(Value::Number((-3.0f64).to_bits()).as_i256(), Some(int(-3)));
    assert_eq!(Value::from(U256::MAX).as_i256(), None);
    assert_eq!(Value::from(I256::MIN).as_i256(), Some(I256::MIN));
    assert_eq!(Value::from(int(-5)).as_i64(), Some(-5));
    assert_eq!(Value::from(I256::MIN).as_i64(), None);
    assert_eq!(Value::from(int(7)).as_u256(), Some(U256::from(7u64)));
    assert_eq!(Value::from(int(-7)).as_u256(), None);
    assert_eq!(Value::String("5".into()).as_i256(), None);
}

#[test]
#[should_panic(expected = "i256 overflow")]
fn addition_past_max_aborts() {
    let _ = I256::MAX + 1;
}

#[test]
#[should_panic(expected = "i256 overflow")]
fn subtraction_past_min_aborts() {
    let _ = I256::MIN - 1;
}

#[test]
#[should_panic(expected = "i256 overflow")]
fn negating_min_aborts() {
    let _ = -I256::MIN;
}

#[test]
#[should_panic(expected = "i256 overflow")]
fn min_divided_by_minus_one_aborts() {
    let _ = I256::MIN / -1;
}

#[test]
#[should_panic(expected = "division by zero")]
fn division_by_zero_aborts() {
    let _ = int(5) / 0;
}

#[test]
#[should_panic(expected = "division by zero")]
fn remainder_by_zero_aborts() {
    let _ = int(5) % I256::ZERO;
}

#[test]
#[should_panic(expected = "i256 overflow")]
fn abs_of_min_aborts() {
    let _ = I256::MIN.abs();
}
