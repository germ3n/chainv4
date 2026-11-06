//! Numbers as people type them, with `_` between digits to group them: 1_000_000, 0xff_ff.

use std::borrow::Cow;

/// The text without its `_` separators, each of which must stand between two digits, hex digits
/// after 0x: None for one that does not, such as _1, 1_, 1__0, 1_.5 or 0x_ff, so a slip is
/// refused rather than read as some other number. Whatever else the text holds is left for the
/// parser after.
pub fn ungrouped(text: &str) -> Option<Cow<'_, str>> {
    if !text.contains('_') {
        return Some(Cow::Borrowed(text));
    }

    let bytes = text.as_bytes();
    let digit: fn(&u8) -> bool = match text.trim_start_matches('-').starts_with("0x") {
        true => u8::is_ascii_hexdigit,
        false => u8::is_ascii_digit,
    };
    let between = |index: usize| index > 0 && digit(&bytes[index - 1]) && bytes.get(index + 1).is_some_and(digit);

    (0..bytes.len()).filter(|&index| bytes[index] == b'_').all(between).then(|| Cow::Owned(text.replace('_', "")))
}

#[cfg(test)]
mod tests {
    use super::ungrouped;

    #[test]
    fn separators_stand_between_digits_alone() {
        for (text, plain) in [("1_000_000", "1000000"), ("0xff_ff", "0xffff"), ("-1_0", "-10"), ("1_000.5", "1000.5"), ("42", "42"), ("", "")] {
            assert_eq!(ungrouped(text).as_deref(), Some(plain), "{text}");
        }

        for bad in ["_1", "1_", "1__0", "1_.5", "1._5", "0x_ff", "-_1", "_", "1_d", "a_b"] {
            assert_eq!(ungrouped(bad), None, "{bad}");
        }
    }

    #[test]
    fn typed_numbers_take_separators_and_strings_keep_theirs() {
        use crate::structs::{tx::TxCallArgumentType as Arg, u256::u256};

        assert_eq!(Arg::parse_typed("u256:1_000_000"), Ok(Arg::U256(u256::from(1_000_000u64))));
        assert_eq!(Arg::parse_typed("i64:-1_000"), Ok(Arg::I64(-1_000)));
        assert_eq!(Arg::parse_typed("number:2_048"), Ok(Arg::Number(2_048.0)));
        assert_eq!(Arg::parse_typed("string:a_b"), Ok(Arg::String("a_b".to_string())));
        assert!(Arg::parse_typed("u64:1__0").unwrap_err().contains("between two digits"));
    }
}
