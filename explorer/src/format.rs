//! How the explorer writes ids, amounts and times.

/// An id or address cut to its ends, keeping any kind in front: `ed25519:0x3b6a91…f2c0`.
pub fn short(text: &str) -> String {
    // The kind and 0x, or a bitcoin address's bc1q, stay whole.
    let (kind, hex) = match text.find("0x") {
        Some(at) => text.split_at(at + 2),
        None if text.starts_with("bc1q") => text.split_at(4),
        None => ("", text),
    };

    if hex.chars().count() <= 12 {
        return text.to_string();
    }

    let head: String = hex.chars().take(6).collect();
    let tail: String = hex.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();

    format!("{kind}{head}…{tail}")
}

/// Digits grouped in threes: 1,234,567.
pub fn thousands(digits: &str) -> String {
    let mut grouped = String::new();

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }

        grouped.push(digit);
    }

    grouped
}

/// A whole number of units shown with its decimals: 1500000000000000000 at 18 is 1.5. Any size,
/// since it only moves the point.
pub fn units(raw: &str, decimals: u8) -> String {
    let digits = raw.trim_start_matches('0');
    let decimals = usize::from(decimals);
    let padded = format!("{digits:0>width$}", width = decimals + 1);
    let (whole, fraction) = padded.split_at(padded.len() - decimals);
    let fraction = fraction.trim_end_matches('0');

    match fraction.is_empty() {
        true => thousands(whole),
        false => format!("{}.{fraction}", thousands(whole)),
    }
}

/// Units with a symbol after them.
pub fn amount(raw: &str, decimals: u8, symbol: &str) -> String {
    format!("{} {symbol}", units(raw, decimals))
}

/// What a typed amount, such as 1.5, comes to in whole units at these decimals, as decimal digits;
/// None for anything else, or for more decimals than there are.
pub fn parse_units(text: &str, decimals: u8) -> Option<String> {
    let text = text.trim().replace([',', '_'], "");
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let decimals = usize::from(decimals);

    if whole.is_empty() && fraction.is_empty()
        || !whole.chars().chain(fraction.chars()).all(|digit| digit.is_ascii_digit())
        || fraction.len() > decimals
    {
        return None;
    }

    let raw = format!("{whole}{fraction:0<decimals$}");
    let raw = raw.trim_start_matches('0');

    Some(match raw.is_empty() {
        true => "0".to_string(),
        false => raw.to_string(),
    })
}

/// A length of time, to the two biggest units: 3h 12m, 45s.
pub fn span(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (days, hours, minutes) = (seconds / 86_400, seconds / 3_600 % 24, seconds / 60 % 60);

    match (days, hours, minutes) {
        (0, 0, 0) if ms < 10_000 => format!("{:.1}s", ms as f64 / 1_000.0),
        (0, 0, 0) => format!("{seconds}s"),
        (0, 0, _) => format!("{minutes}m {}s", seconds % 60),
        (0, _, _) => format!("{hours}h {minutes}m"),
        _ => format!("{days}d {hours}h"),
    }
}

/// How long ago something happened: 12s ago, or "in 3s" for a clock ahead of ours.
pub fn ago(then: u64, now: u64) -> String {
    match now.checked_sub(then) {
        Some(past) => format!("{} ago", span(past)),
        None => format!("in {}", span(then - now)),
    }
}

/// A time in UTC: 2026-10-24 13:05:09.
pub fn utc(ms: u64) -> String {
    let seconds = ms / 1_000;
    let (date, time) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil(date as i64);

    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}", time / 3_600, time / 60 % 60, time % 60)
}

/// The calendar date a number of days after 1970-01-01 falls on (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    (year, month, day)
}

/// A hashrate, in the unit that keeps it short: 950 H/s, 205.8 MH/s, 1.25 TH/s.
pub fn hashrate(rate: f64) -> String {
    let units = ["H/s", "kH/s", "MH/s", "GH/s", "TH/s", "PH/s", "EH/s"];
    let mut scaled = rate.max(0.0);
    let mut unit = 0;

    while scaled >= 1_000.0 && unit < units.len() - 1 {
        scaled /= 1_000.0;
        unit += 1;
    }

    match unit {
        0 => format!("{scaled:.0} {}", units[0]),
        _ if scaled >= 100.0 => format!("{scaled:.1} {}", units[unit]),
        _ => format!("{scaled:.2} {}", units[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashrates_take_the_unit_that_keeps_them_short() {
        assert_eq!(hashrate(950.0), "950 H/s");
        assert_eq!(hashrate(205_812_000.0), "205.8 MH/s");
        assert_eq!(hashrate(1.25e12), "1.25 TH/s");
        assert_eq!(hashrate(0.0), "0 H/s");
        assert_eq!(hashrate(-3.0), "0 H/s");
        assert_eq!(hashrate(5e24), "5000000.0 EH/s");
    }

    #[test]
    fn ids_keep_their_kind_and_ends() {
        assert_eq!(short("ed25519:0x3b6a91aabbccddeeff00f2c0"), "ed25519:0x3b6a91…f2c0");
        assert_eq!(short("0x0011223344556677889900aabbccddeeff"), "0x001122…eeff");
        assert_eq!(short("0x1234"), "0x1234");
    }

    #[test]
    fn units_move_the_point_and_back() {
        assert_eq!(units("1500000000000000000", 18), "1.5");
        assert_eq!(units("1000000000000000000000000", 18), "1,000,000");
        assert_eq!(units("1", 18), "0.000000000000000001");
        assert_eq!(units("0", 18), "0");
        assert_eq!(units("1234567", 0), "1,234,567");
        assert_eq!(units("1050", 2), "10.5");
        assert_eq!(amount("2500", 3, "EXT"), "2.5 EXT");

        assert_eq!(parse_units("1.5", 18).as_deref(), Some("1500000000000000000"));
        assert_eq!(parse_units("1,000", 2).as_deref(), Some("100000"));
        assert_eq!(parse_units("0.000", 3).as_deref(), Some("0"));
        assert_eq!(parse_units(".5", 1).as_deref(), Some("5"));
        assert_eq!(parse_units("1.234", 2), None);
        assert_eq!(parse_units("-1", 18), None);
        assert_eq!(parse_units("", 18), None);

        for raw in ["1", "42", "1000000000000000000", "123456789012345678901234567890"] {
            for decimals in [0, 6, 18] {
                assert_eq!(parse_units(&units(raw, decimals), decimals).as_deref(), Some(raw));
            }
        }
    }

    #[test]
    fn times_read_as_people_write_them() {
        assert_eq!(utc(0), "1970-01-01 00:00:00");
        assert_eq!(utc(1_792_847_109_000), "2026-10-24 13:05:09");
        assert_eq!(utc(951_782_400_000), "2000-02-29 00:00:00");
        assert_eq!(span(2_500), "2.5s");
        assert_eq!(span(45_000), "45s");
        assert_eq!(span(125_000), "2m 5s");
        assert_eq!(span(3 * 3_600_000 + 12 * 60_000), "3h 12m");
        assert_eq!(span(26 * 3_600_000), "1d 2h");
        assert_eq!(ago(1_000, 13_000), "12s ago");
        assert_eq!(ago(5_000, 2_000), "in 3.0s");
        assert_eq!(thousands("1234567"), "1,234,567");
        assert_eq!(thousands("123"), "123");
    }
}
