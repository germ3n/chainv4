use super::*;
use crate::vm::lvm::libs::address::register_address_lib;

const LIBRARIES: [&str; 6] = ["u64", "u128", "u256", "i64", "i128", "i256"];

fn setup() -> Lua {
    let lua = Lua::new();
    lua.set_fuel(u64::MAX).unwrap();
    register_integer_libs(&lua);
    register_address_lib(&lua);
    // an address, to compare integers with something that is neither
    lua.load("A = address.from_hex(string.rep('00', 32))").exec().unwrap();

    lua
}

/// What an expression gives, through tostring.
fn show(lua: &Lua, expression: &str) -> String {
    lua.load(format!("return tostring({expression})"))
        .eval::<String>()
        .unwrap_or_else(|err| panic!("{expression}: {err}"))
}

/// Each expression is true.
fn holds(lua: &Lua, expressions: &[&str]) {
    for expression in expressions {
        let result = lua.load(format!("return {expression}")).eval::<bool>();

        assert_eq!(result.as_ref().ok(), Some(&true), "{expression}: {result:?}");
    }
}

/// The code raises an error that says this.
fn fails(lua: &Lua, code: &str, expected: &str) {
    match lua.load(code).exec() {
        Ok(()) => panic!("{code}: ran without the error {expected:?}"),
        Err(err) => assert!(err.to_string().contains(expected), "{code}: {err}, not {expected:?}"),
    }
}

#[test]
fn each_type_is_its_own_lua_type() {
    let lua = setup();

    for library in LIBRARIES {
        let name = library.to_uppercase();

        assert_eq!(show(&lua, &format!("typeof({library}(1))")), name);
        assert_eq!(show(&lua, &format!("type({library}(1))")), "userdata");
        assert_eq!(show(&lua, &format!("typeof({library}.new(1))")), name);
        assert_eq!(show(&lua, &format!("typeof({library}.zero)")), name);
    }
}

#[test]
fn constants_are_each_type_s_limits() {
    let lua = setup();
    let limits = [
        ("u64", "0", "18446744073709551615"),
        ("u128", "0", "340282366920938463463374607431768211455"),
        ("u256", "0", "115792089237316195423570985008687907853269984665640564039457584007913129639935"),
        ("i64", "-9223372036854775808", "9223372036854775807"),
        ("i128", "-170141183460469231731687303715884105728", "170141183460469231731687303715884105727"),
        (
            "i256",
            "-57896044618658097711785492504343953926634992332820282019728792003956564819968",
            "57896044618658097711785492504343953926634992332820282019728792003956564819967",
        ),
    ];

    for (library, min, max) in limits {
        assert_eq!(show(&lua, &format!("{library}.min")), min);
        assert_eq!(show(&lua, &format!("{library}.max")), max);
        assert_eq!(show(&lua, &format!("{library}.zero")), "0");
        // the text reads back as the same value
        holds(&lua, &[&format!("{library}('{min}') == {library}.min"), &format!("{library}('{max}') == {library}.max")]);
    }
}

#[test]
fn values_are_made_from_numbers_text_and_other_integers() {
    let lua = setup();

    assert_eq!(show(&lua, "u256('1000000000000000000000')"), "1000000000000000000000");
    assert_eq!(show(&lua, "u64('0xff')"), "255");
    assert_eq!(show(&lua, "i64('-0x10')"), "-16");
    assert_eq!(show(&lua, "i128('-42')"), "-42");
    assert_eq!(show(&lua, "i256(-9007199254740991)"), "-9007199254740991");
    assert_eq!(show(&lua, "u64(9007199254740991)"), "9007199254740991");
    assert_eq!(show(&lua, "u128('007')"), "7");
    // from another integer, when the type holds it
    assert_eq!(show(&lua, "typeof(u64(u256(5)))"), "U64");
    assert_eq!(show(&lua, "i256(u256.max // 2)"), show(&lua, "i256.max"));
    assert_eq!(show(&lua, "u256(i256.max)"), show(&lua, "i256.max"));
    assert_eq!(show(&lua, "u128(i128(5))"), "5");
    assert_eq!(show(&lua, "i64(u64('9223372036854775807'))"), "9223372036854775807");
}

#[test]
fn bad_values_are_refused() {
    let lua = setup();

    for (code, expected) in [
        ("local _ = u256(-1)", "non-negative whole number"),
        // Numbers are whole: a fraction does not compile, and a number past 2^53 or a division by
        // zero is refused where it is made, before any type sees it.
        ("local _ = u256(1.5)", "Numbers are whole, within 2^53 either way: 1.5 is not"),
        ("local _ = u256(2 ^ 60)", "numbers are whole"),
        ("local _ = u256(0 / 0)", "numbers are whole"),
        ("local _ = i64(2 ^ 60)", "numbers are whole"),
        ("local _ = i64(-0.5)", "Numbers are whole"),
        ("local _ = u256('12abc')", "not a u256: 12abc"),
        ("local _ = u256('')", "not a u256"),
        ("local _ = u256('0x')", "not a u256"),
        ("local _ = u256('+5')", "not a u256: +5"),
        ("local _ = u256('-5')", "not a u256: -5"),
        ("local _ = i64('--5')", "not an i64: --5"),
        ("local _ = i64('-+5')", "not an i64"),
        ("local _ = i64('1.5')", "not an i64"),
        ("local _ = u64('18446744073709551616')", "not a u64"),
        ("local _ = i64('9223372036854775808')", "not an i64"),
        ("local _ = i64('-9223372036854775809')", "not an i64"),
        ("local _ = u256('115792089237316195423570985008687907853269984665640564039457584007913129639936')", "not a u256"),
        ("local _ = u64(i64(-1))", "not a u64: -1"),
        ("local _ = i64(u64.max)", "not an i64: 18446744073709551615"),
        ("local _ = i256(u256.max)", "not an i256"),
        ("local _ = u128(u256.max)", "not a u128"),
        ("local _ = u256({})", "non-negative whole number"),
        ("local _ = u256(true)", "non-negative whole number"),
    ] {
        fails(&lua, code, expected);
    }
}

#[test]
fn arithmetic_is_exact_and_keeps_the_type() {
    let lua = setup();

    assert_eq!(show(&lua, "u256('9007199254740993') + 1"), "9007199254740994");
    assert_eq!(show(&lua, "u256(2) ^ 200 / u256(2) ^ 199"), "2");
    assert_eq!(show(&lua, "u256(17) % 5"), "2");
    assert_eq!(show(&lua, "10 - u256(4)"), "6");
    assert_eq!(show(&lua, "3 * u64(5)"), "15");
    assert_eq!(show(&lua, "2 ^ u64(10)"), "1024");
    assert_eq!(show(&lua, "typeof(2 ^ u64(10))"), "U64");
    assert_eq!(show(&lua, "i64(5) - 8"), "-3");
    assert_eq!(show(&lua, "-i64(5)"), "-5");
    assert_eq!(show(&lua, "-i256.max"), show(&lua, "i256.min + 1"));
    assert_eq!(show(&lua, "-u64(0)"), "0");
    assert_eq!(show(&lua, "i128(-3) * -4"), "12");
    assert_eq!(show(&lua, "i64(-2) ^ 3"), "-8");
    assert_eq!(show(&lua, "(u128(2) ^ 127 - 1) * 2 + 1"), show(&lua, "u128.max"));
    assert_eq!(show(&lua, "i256('-1000000000000000000000000000000') / 1000000000000000"), "-1000000000000000");

    for library in LIBRARIES {
        for operator in ["+", "-", "*", "/", "//", "%", "^"] {
            let code = format!("typeof({library}(6) {operator} 2) .. typeof(6 {operator} {library}(2))");

            assert_eq!(show(&lua, &code), library.to_uppercase().repeat(2), "{library} {operator}");
        }
    }
}

#[test]
fn division_rounds_as_rust_does_and_floor_division_as_lua_does() {
    let lua = setup();

    for library in ["i64", "i128", "i256"] {
        let cases = [
            ("-7 / 2", "-3"),
            ("7 / -2", "-3"),
            ("-7 / -2", "3"),
            ("-7 % 2", "-1"),
            ("7 % -2", "1"),
            ("-7 % -2", "-1"),
            ("-7 // 2", "-4"),
            ("7 // -2", "-4"),
            ("-7 // -2", "3"),
            ("-8 // 2", "-4"),
            ("7 // 2", "3"),
            ("0 // -3", "0"),
        ];

        for (expression, expected) in cases {
            let (left, rest) = expression.split_once(' ').unwrap();

            assert_eq!(show(&lua, &format!("{library}({left}) {rest}")), expected, "{library}: {expression}");
        }
    }

    // unsigned types round down either way
    assert_eq!(show(&lua, "u64(7) / 2"), "3");
    assert_eq!(show(&lua, "u64(7) // 2"), "3");
    assert_eq!(show(&lua, "u256(7) % 2"), "1");
}

#[test]
fn operators_never_wrap() {
    let lua = setup();

    for library in LIBRARIES {
        let unsigned = library.starts_with('u');

        fails(&lua, &format!("local _ = {library}.max + 1"), &format!("{library} overflow"));
        fails(&lua, &format!("local _ = {library}.max * 2"), &format!("{library} overflow"));
        fails(&lua, &format!("local _ = {library}(2) ^ {}", if unsigned { 300 } else { 299 }), &format!("{library} overflow"));
        fails(&lua, &format!("local _ = {library}(1) / 0"), "division by zero");
        fails(&lua, &format!("local _ = {library}(1) // 0"), "division by zero");
        fails(&lua, &format!("local _ = {library}(1) % {library}.zero"), "division by zero");

        if unsigned {
            fails(&lua, &format!("local _ = {library}(1) - 2"), &format!("{library} underflow"));
            fails(&lua, &format!("local _ = -{library}(1)"), &format!("{library} underflow"));
        } else {
            fails(&lua, &format!("local _ = {library}.min - 1"), &format!("{library} overflow"));
            fails(&lua, &format!("local _ = -{library}.min"), &format!("{library} overflow"));
            fails(&lua, &format!("local _ = {library}.min / -1"), &format!("{library} overflow"));
            fails(&lua, &format!("local _ = {library}.min // -1"), &format!("{library} overflow"));
            fails(&lua, &format!("local _ = {library}.min % -1"), &format!("{library} overflow"));
            fails(&lua, &format!("local _ = {library}.min * -1"), &format!("{library} overflow"));
        }
    }

    // each width overflows at its own edge, not at 2^256
    fails(&lua, "local _ = u64(2) ^ 64", "u64 overflow");
    assert_eq!(show(&lua, "u128(2) ^ 64"), "18446744073709551616");
    fails(&lua, "local _ = u128(2) ^ 64 * u128(2) ^ 64", "u128 overflow");
    assert_eq!(show(&lua, "u256(2) ^ 64 * u256(2) ^ 64"), "340282366920938463463374607431768211456");
}

#[test]
fn one_signedness_widens_and_the_two_do_not_mix() {
    let lua = setup();

    assert_eq!(show(&lua, "typeof(u64(1) + u128(2))"), "U128");
    assert_eq!(show(&lua, "typeof(u128(2) * u64(1))"), "U128");
    assert_eq!(show(&lua, "typeof(u64(1) + u256(2))"), "U256");
    assert_eq!(show(&lua, "typeof(i64(1) - i256(2))"), "I256");
    assert_eq!(show(&lua, "typeof(i128(1) // i64(2))"), "I128");
    assert_eq!(show(&lua, "u64.max + u128(1)"), "18446744073709551616");
    assert_eq!(show(&lua, "i64.min - i128(1)"), "-9223372036854775809");
    assert_eq!(show(&lua, "typeof(u64(3):min(u256(2)))"), "U256");
    assert_eq!(show(&lua, "typeof(u64(3):checked_add(u128(2)))"), "U128");
    assert_eq!(show(&lua, "i256(u128(5)) + i128(-3)"), "2");
    assert_eq!(show(&lua, "u256(i64(5)) + u64(3)"), "8");

    for code in [
        "local _ = u128(5) + i128(-3)",
        "local _ = i64(5) * u64(1)",
        "local _ = u256(1) - i256(1)",
        "local _ = u64(1) // i64(1)",
        "local _ = u64(1) % i64(1)",
        "local _ = u64(5):checked_add(i64(1))",
        "local _ = u64(5):wrapping_add(i64(1))",
        "local _ = i64(5):saturating_sub(u64(1))",
        "local _ = i64(5):max(u64(1))",
    ] {
        fails(&lua, code, "do not mix: convert one side, such as i256(x) or u256(x)");
    }

    fails(&lua, "local _ = u128(5) + i128(-3)", "U128 and I128 do not mix");
}

#[test]
fn plain_numbers_take_the_integer_s_type() {
    let lua = setup();

    assert_eq!(show(&lua, "i64(5) + -1"), "4");
    assert_eq!(show(&lua, "-1 + i256(5)"), "4");
    assert_eq!(show(&lua, "u64(5) - 5"), "0");
    assert_eq!(show(&lua, "u64(1) + 9007199254740991"), "9007199254740992");

    // Fractions, numbers past 2^53 and NaN, as only the host can make them now.
    let globals = lua.globals();
    for (name, value) in [("f15", 1.5), ("f05", 0.5), ("p60", 2f64.powi(60)), ("nan", f64::NAN)] {
        globals.set(name, value).unwrap();
    }

    for (code, expected) in [
        ("local _ = u256(1) + 1.5", "Numbers are whole"),
        ("local _ = u64(5) + -1", "u64 arithmetic needs a U64 or a non-negative whole number up to 2^53"),
        ("local _ = u256(1) + f15", "u256 arithmetic needs a U256 or a non-negative whole number"),
        ("local _ = u256(1) + p60", "non-negative whole number"),
        ("local _ = i64(1) + p60", "i64 arithmetic needs an I64 or a whole number within 2^53 either way"),
        ("local _ = i64(1) * f05", "whole number within 2^53"),
        ("local _ = u256(1) + 'x'", "non-negative whole number"),
        ("local _ = u256(1) + '5'", "non-negative whole number"),
        ("local _ = u256(1) + {}", "non-negative whole number"),
        ("local _ = u256(1) + nil", "non-negative whole number"),
        ("local _ = u256(1) + nan", "non-negative whole number"),
        ("local _ = u64(2) ^ -1", "a negative exponent would make a fraction"),
        ("local _ = u64(2) ^ 4294967296", "an exponent is at most 2^32 - 1"),
        ("local _ = u64(2) ^ f05", "an exponent is a whole number"),
        ("local _ = u64(2) ^ i64(-1)", "a negative exponent would make a fraction"),
    ] {
        fails(&lua, code, expected);
    }
}

#[test]
fn checked_methods_give_nil_instead_of_an_error() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u64.max:checked_add(1) == nil",
            "u64(5):checked_add(1) == 6",
            "u64(0):checked_sub(1) == nil",
            "u64(5):checked_sub(u64(5)) == 0",
            "u128.max:checked_mul(2) == nil",
            "u128(3):checked_mul(4) == 12",
            "u256(1):checked_div(0) == nil",
            "u256(7):checked_div(2) == 3",
            "u256(7):checked_rem(0) == nil",
            "u256(7):checked_rem(4) == 3",
            "u64(2):checked_pow(64) == nil",
            "u64(2):checked_pow(63) == u64('9223372036854775808')",
            "i64.min:checked_sub(1) == nil",
            "i64(-5):checked_sub(1) == -6",
            "i64.min:checked_div(-1) == nil",
            "i64(-7):checked_div(2) == -3",
            "i64.min:checked_rem(-1) == nil",
            "i64(-7):checked_rem(2) == -1",
            "i128.max:checked_add(i128(1)) == nil",
            "i256.min:checked_mul(-1) == nil",
            "i256(-2):checked_pow(255) == i256.min",
            "i256(2):checked_pow(255) == nil",
            "typeof(u64(1):checked_add(u256(1))) == 'U256'",
        ],
    );
}

#[test]
fn saturating_methods_stop_at_the_limits() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u64.max:saturating_add(1) == u64.max",
            "u64(1):saturating_add(1) == 2",
            "u64(1):saturating_sub(5) == 0",
            "u64(9):saturating_sub(5) == 4",
            "u128.max:saturating_mul(2) == u128.max",
            "u128(3):saturating_mul(2) == 6",
            "u256(10):saturating_pow(300) == u256.max",
            "u256(10):saturating_pow(3) == 1000",
            "i64.max:saturating_add(1) == i64.max",
            "i64.min:saturating_sub(1) == i64.min",
            "i64.min:saturating_add(-1) == i64.min",
            "i128.min:saturating_mul(-1) == i128.max",
            "i128.min:saturating_mul(2) == i128.min",
            "i256(-10):saturating_pow(255) == i256.min",
            "i256(-10):saturating_pow(256) == i256.max",
        ],
    );
}

#[test]
fn wrapping_methods_wrap_at_each_width() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u64.max:wrapping_add(1) == 0",
            "u64(0):wrapping_sub(1) == u64.max",
            "u64(4294967296):wrapping_mul(4294967296) == 0",
            "u64(4294967296):wrapping_mul(4294967297) == 4294967296",
            "u128.max:wrapping_add(2) == 1",
            "u128(2):wrapping_pow(128) == 0",
            "u256.max:wrapping_add(1) == 0",
            "u256(0):wrapping_sub(1) == u256.max",
            "u256(2):wrapping_pow(256) == 0",
            "u256(2):wrapping_pow(255) == u256(2) ^ 255",
            "i64.max:wrapping_add(1) == i64.min",
            "i64.min:wrapping_sub(1) == i64.max",
            "i64.min:wrapping_mul(-1) == i64.min",
            "i64(-2):wrapping_pow(63) == i64.min",
            "i64(-2):wrapping_pow(64) == 0",
            "i128.max:wrapping_mul(2) == -2",
            "i256.min:wrapping_sub(i256(1)) == i256.max",
            "typeof(u64.max:wrapping_add(u128(1))) == 'U128'",
            "u64.max:wrapping_add(u128(1)) == u128('18446744073709551616')",
        ],
    );

    fails(&lua, "local _ = u64(2):wrapping_pow(-1)", "a negative exponent would make a fraction");
    fails(&lua, "local _ = u64(2):wrapping_pow(4294967296)", "an exponent is at most 2^32 - 1");
}

#[test]
fn min_max_abs_tonumber_and_hex() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u256(5):min(3) == 3",
            "u256(5):max(30) == 30",
            "i64(-5):min(i64(3)) == -5",
            "i64(-5):max(-7) == -5",
            "i64(-5):abs() == 5",
            "i256.max:abs() == i256.max",
            "typeof(i64(-5):abs()) == 'I64'",
            "u64(1).abs == nil",
            "u256(42):tonumber() == 42",
            "i64(-42):tonumber() == -42",
            "u64(9007199254740991):tonumber() == 9007199254740991",
            "type(u64(1):tonumber()) == 'number'",
        ],
    );

    holds(
        &lua,
        &[
            "i64(5):checked_neg() == -5",
            "i64.min:checked_neg() == nil",
            "u64(0):checked_neg() == 0",
            "u64(1):checked_neg() == nil",
            "u64(1):wrapping_neg() == u64.max",
            "u256(0):wrapping_neg() == 0",
            "i128.min:wrapping_neg() == i128.min",
            "i256(7):wrapping_neg() == -7",
            "i64.min:saturating_neg() == i64.max",
            "i64(-3):saturating_neg() == 3",
            "u64(1).saturating_neg == nil",
        ],
    );
    fails(&lua, "local _ = i64.min:abs()", "i64 overflow");
    fails(&lua, "local _ = u64(1):abs()", "attempt to call");
    fails(&lua, "local _ = u64('9007199254740992'):tonumber()", "is past 2^53, where numbers round");
    fails(&lua, "local _ = i64('-9007199254740993'):tonumber()", "is past 2^53");

    assert_eq!(show(&lua, "u64.max:hex()"), "0xffffffffffffffff");
    assert_eq!(show(&lua, "u256('0xff'):hex()"), "0xff");
    assert_eq!(show(&lua, "u64(0):hex()"), "0x0");
    assert_eq!(show(&lua, "i64(-16):hex()"), "-0x10");
    assert_eq!(show(&lua, "i256.min:hex()"), format!("-0x8{}", "0".repeat(63)));
    assert_eq!(show(&lua, "u256.max:hex()"), format!("0x{}", "f".repeat(64)));
}

#[test]
fn text_and_concatenation() {
    let lua = setup();

    assert_eq!(show(&lua, "'total: ' .. u256(5)"), "total: 5");
    assert_eq!(show(&lua, "i64(-5) .. ' left'"), "-5 left");
    assert_eq!(show(&lua, "u64(1) .. u128(2)"), "12");
    assert_eq!(show(&lua, "u64(1) .. 2"), "12");
    assert_eq!(show(&lua, "i256.min"), "-57896044618658097711785492504343953926634992332820282019728792003956564819968");
    fails(&lua, "local _ = u64(1) .. {}", "attempt to concatenate");
}

#[test]
fn comparisons_with_numbers_go_by_the_true_value() {
    let lua = setup();

    for library in LIBRARIES {
        // constants, register values from tonumber, and either side, through every comparison
        let cases = [
            "x == 5", "5 == x", "x ~= 6", "not (x ~= 5)", "x == n", "n == x", "not (x ~= n)", "x ~= m",
            "x < 6", "6 > x", "x <= 5", "x >= 5", "x > 4", "4 < x", "x < m", "m > x", "x <= n", "n >= x",
            "not (x < 5)", "not (x > 5)", "not (x == m)",
        ];

        for case in cases {
            let code = format!("local x, n, m = {library}(5), tonumber('5'), tonumber('6') return {case}");

            assert_eq!(lua.load(&code).eval::<bool>().ok(), Some(true), "{library}: {case}");
        }

        // and as conditions, which compile to the jumps rather than to a value
        let code = format!(
            "local x, n = {library}(5), tonumber('5')
            local hits = 0
            if x == 5 then hits += 1 end
            if x ~= 5 then hits += 100 end
            if x == n then hits += 1 end
            if x ~= n then hits += 100 end
            if n == x then hits += 1 end
            if x < 6 then hits += 1 end
            if x >= n then hits += 1 end
            while x > n do hits += 100 break end
            return hits"
        );

        assert_eq!(lua.load(&code).eval::<u32>().unwrap(), 5, "{library}");
    }
}

#[test]
fn comparisons_are_exact_past_2_to_the_53_and_with_fractions() {
    let lua = setup();

    // Scripts cannot make a fraction, a number past 2^53, an infinity or NaN any more, but the
    // host can hand them one, so they arrive here as globals the host set.
    let globals = lua.globals();
    for (name, value) in [
        ("p60", 2f64.powi(60)),
        ("p200", 2f64.powi(200)),
        ("p255", 2f64.powi(255)),
        ("p256", 2f64.powi(256)),
        ("n200", -(2f64.powi(200))),
        ("n255", -(2f64.powi(255))),
        ("n256", -(2f64.powi(256))),
        ("f25", 2.5),
        ("fm05", -0.5),
        ("fm15", -1.5),
        ("mzero", -0.0),
        ("inf", f64::INFINITY),
        ("ninf", f64::NEG_INFINITY),
        ("nan", f64::NAN),
    ] {
        globals.set(name, value).unwrap();
    }

    holds(
        &lua,
        &[
            "u256(2) ^ 60 == p60",
            "u256(2) ^ 60 + 1 > p60",
            "u256(2) ^ 60 - 1 < p60",
            "p60 < u256(2) ^ 60 + 1",
            "u256(2) ^ 255 == p255",
            "u256.max < p256",
            "u256.max > p255",
            "i256(2) ^ 200 * -1 == n200",
            "i256.min == n255",
            "i256.min > n256",
            "i256.max < p255",
            "u256(2) < f25",
            "u256(3) > f25",
            "u256(2) ~= f25",
            "u256(2) <= f25",
            "not (u256(2) >= f25)",
            "u256(0) > -1",
            "u256(0) > fm05",
            "u64(0) >= mzero",
            "u64(0) == mzero",
            "i64(-1) < fm05",
            "i64(-1) > fm15",
            "i64(-2) < fm15",
            "i64(0) > fm05",
            "fm05 < i64(0)",
            "u256.max < inf",
            "i256.min > ninf",
            "u64(0) > ninf",
            "not (u64(0) == nan)",
            "u64(0) ~= nan",
            "not (u64(0) < nan)",
            "not (u64(0) > nan)",
            "not (u64(0) <= nan)",
            "not (nan >= u64(0))",
        ],
    );
}

#[test]
fn comparisons_across_types_are_exact() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u64(5) == u256(5)",
            "u256(5) == u64(5)",
            "u128(5) == i64(5)",
            "u64(5) ~= u256(6)",
            "u128(5) < u256(6)",
            "u256(6) > u128(5)",
            "u64.max < u128.max",
            "u64.max == u128(u64.max)",
            "i64(-1) < u64(0)",
            "u64(0) > i64(-1)",
            "i256.min < i64.min",
            "i64.min > i256.min",
            "u256.max > i256.max",
            "i256(-1) < u256.zero",
            "u256(2) ^ 255 > i256.max",
            "u256(2) ^ 255 - 1 == i256.max",
            "i128(-5) <= i64(-5)",
            "i128(-5) >= i64(-5)",
        ],
    );
}

#[test]
fn integers_compare_with_nothing_else() {
    let lua = setup();

    holds(
        &lua,
        &[
            "u256(5) ~= '5'",
            "not (u256(5) == '5')",
            "u256(5) ~= true",
            "u256(5) ~= nil",
            "u256(5) ~= {}",
            "u256(1) ~= A",
            "A ~= u256(1)",
            "A ~= 0",
            "0 ~= A",
            "not rawequal(u256(5), 5)",
            "not rawequal(u256(5), u256(5))",
        ],
    );

    fails(&lua, "local _ = u256(5) < '6'", "attempt to compare");
    fails(&lua, "local _ = u256(5) < {}", "attempt to compare");
    fails(&lua, "local _ = u256(5) < A", "attempt to compare");
    fails(&lua, "local _ = A < u256(5)", "attempt to compare");
    // tables and plain numbers keep standard Lua: no metamethod is asked
    fails(&lua, "local _ = {} < 1", "attempt to compare");
    holds(&lua, &["setmetatable({}, { __eq = function() return true end }) ~= 1"]);
}

#[test]
fn the_table_library_compares_them_too() {
    let lua = setup();

    holds(
        &lua,
        &[
            "table.find({ u256(7), 3 }, 7) == 1",
            "table.find({ 1, i64(-2) }, -2) == 2",
            "table.find({ u64(1) }, 2) == nil",
        ],
    );

    // a fraction, as only the host can make one
    lua.globals().set("f25", 2.5).unwrap();
    let sorted = lua
        .load("local t = { u256(3), 1, i64(-2), u64(2), f25, i256(-7) } table.sort(t) local out = {} for _, v in t do table.insert(out, tostring(v)) end return table.concat(out, ' ')");

    assert_eq!(sorted.eval::<String>().unwrap(), "-7 -2 1 2 2.5 3");
}

#[test]
fn integers_are_still_refused_as_table_keys() {
    let lua = setup();

    fails(&lua, "local t = {} t[u256(1)] = true", "");
}

#[test]
fn exact_as_takes_any_integer_or_whole_number_that_fits() {
    let lua = setup();
    let value = |code: &str| lua.load(code).eval::<Value>().unwrap();

    assert_eq!(exact_as::<u64>(&value("u256(5)")).unwrap(), 5);
    assert_eq!(exact_as::<u64>(&value("5")).unwrap(), 5);
    assert_eq!(exact_as::<i128>(&value("i64(-5)")).unwrap(), -5);
    assert_eq!(exact_as::<u256>(&value("u128.max")).unwrap(), u256::from(u128::MAX));
    assert_eq!(exact_as::<i256>(&value("i256.min")).unwrap(), i256::MIN);
    assert!(exact_as::<u64>(&value("u128.max")).unwrap_err().to_string().contains("is outside what a U64 holds, 0 to 18446744073709551615"));
    assert!(exact_as::<u64>(&value("i64(-1)")).unwrap_err().to_string().contains("outside what a U64 holds"));
    assert!(exact_as::<i64>(&value("u64.max")).unwrap_err().to_string().contains("outside what an I64 holds"));
    assert!(exact_as::<u64>(&value("-1")).unwrap_err().to_string().contains("non-negative whole number"));
    assert!(exact_as::<u64>(&value("'5'")).unwrap_err().to_string().contains("non-negative whole number"));
}

#[test]
fn exact_conversions_hold_every_value_at_the_edges() {
    let edges = [
        Int::U64(0),
        Int::U64(u64::MAX),
        Int::U128(u128::MAX),
        Int::U256(u256::MAX),
        Int::I64(i64::MIN),
        Int::I64(i64::MAX),
        Int::I128(i128::MIN),
        Int::I128(i128::MAX),
        Int::I256(i256::MIN),
        Int::I256(i256::MAX),
        Int::I256(-i256::ONE),
    ];

    for value in edges {
        for kind in IntKind::ALL {
            // a conversion that succeeds keeps the value, and converts back
            if let Some(converted) = value.to(kind) {
                assert_eq!(converted.to_string(), value.to_string(), "{value:?} as {kind:?}");
                assert_eq!(converted.kind(), kind);
                assert_eq!(converted.to(value.kind()), Some(value));
            }
        }

        assert_eq!(value.to(value.kind()), Some(value));
    }

    assert_eq!(Int::U64(u64::MAX).to(IntKind::I64), None);
    assert_eq!(Int::U64(u64::MAX).to(IntKind::I128), Some(Int::I128(i128::from(u64::MAX))));
    assert_eq!(Int::I64(-1).to(IntKind::U256), None);
    assert_eq!(Int::I64(i64::MIN).to(IntKind::I256), Some(Int::I256(i256::from(i64::MIN))));
    assert_eq!(Int::I256(i256::MIN).to(IntKind::I128), None);
    assert_eq!(Int::U256(u256::ONE << 255).to(IntKind::I256), None);
    assert_eq!(Int::U256((u256::ONE << 255) - u256::ONE).to(IntKind::I256), Some(Int::I256(i256::MAX)));
    assert_eq!(Int::I128(i128::MIN).to(IntKind::I64), None);
    assert_eq!(Int::I128(i128::from(i64::MIN)).to(IntKind::I64), Some(Int::I64(i64::MIN)));
}

#[test]
fn whole_floats_convert_without_rounding() {
    assert_eq!(whole_u256(0.0), Some(u256::ZERO));
    assert_eq!(whole_u256(9007199254740992.0), Some(u256::from(1u64 << 53)));
    assert_eq!(whole_u256(2f64.powi(60)), Some(u256::ONE << 60));
    assert_eq!(whole_u256(2f64.powi(255)), Some(u256::ONE << 255));
    assert_eq!(whole_u256(2f64.powi(256)), None);
    assert_eq!(whole_u256(f64::INFINITY), None);
    // the largest f64 below 2^256: 53 ones, then zeros
    let below = 2f64.powi(256) - 2f64.powi(203);

    assert_eq!(whole_u256(below), Some(((u256::ONE << 53) - u256::ONE) << 203));
}
