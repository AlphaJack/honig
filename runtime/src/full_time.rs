//! `format: time`: RFC 3339 `full-time`, with its offset and its leap second.

use crate::http_date::{DAY_HOURS, HOUR_MINUTES, MINUTE_SECONDS};

/// The last minute of a UTC day: the only one a leap second may end.
const LAST_MINUTE: i64 = DAY_HOURS * HOUR_MINUTES - 1;
/// Hour, minute and second.
const CLOCK_FIELDS: usize = 3;
/// Minutes in a day.
const DAY_MINUTES: i64 = DAY_HOURS * HOUR_MINUTES;

/// Is `text` `HH:MM:SS`, an optional fraction and then `Z` or `+HH:MM` or `-HH:MM`? Letters may be lowercase. A
/// second of `60` is a leap second: it is valid only where the time in UTC is `23:59:60`.
pub(crate) fn is_full_time(text: &str) -> bool {
    let Some((clock, offset)) = split_offset(text) else { return false };
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "1"));
    let fraction_ok = !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit());
    let Some([h, m, s]) = fields::<CLOCK_FIELDS>(clock) else { return false };
    let clock_ok = h < DAY_HOURS && m < HOUR_MINUTES && s <= MINUTE_SECONDS;
    let leap_ok = s < MINUTE_SECONDS || (h * HOUR_MINUTES + m - offset).rem_euclid(DAY_MINUTES) == LAST_MINUTE;
    fraction_ok && clock_ok && leap_ok
}

/// Returns the clock and the offset east of UTC in minutes.
fn split_offset(text: &str) -> Option<(&str, i64)> {
    if let Some(clock) = text.strip_suffix(['Z', 'z']) {
        return Some((clock, 0));
    }
    let at = text.rfind(['+', '-'])?;
    let (clock, offset) = text.split_at(at);
    let sign = if offset.starts_with('-') { -1 } else { 1 };
    let [h, m] = fields::<2>(offset.get(1..)?)?;
    (h < DAY_HOURS && m < HOUR_MINUTES).then_some((clock, sign * (h * HOUR_MINUTES + m)))
}

/// Returns `N` two-digit numbers between colons.
fn fields<const N: usize>(text: &str) -> Option<[i64; N]> {
    let parts: [&str; N] = text.split(':').collect::<Vec<_>>().try_into().ok()?;
    let mut out = [0; N];
    for (o, p) in out.iter_mut().zip(parts) {
        if p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *o = p.parse().ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_has_an_offset_and_a_leap_second_only_at_the_end_of_a_utc_day() {
        for good in [
            "08:30:06Z",
            "08:30:06.283185Z",
            "08:30:06+01:00",
            "08:30:06z",
            "23:59:60Z",
            "15:59:60-08:00",
            "01:29:60+01:30",
            "23:59:60+00:00",
            "00:00:00-00:00",
        ] {
            assert!(is_full_time(good), "{good}");
        }
        for bad in [
            "08:30:06",
            "8:30:06Z",
            "08:30Z",
            "24:00:00Z",
            "08:30:06+0100",
            "08:30:06 Z",
            "08:30:06.Z",
            "22:59:60Z",
            "23:59:60+01:00",
            "08:30:06+24:00",
            "08:30:06+01:60",
            "08:30:61Z",
            "08:30:06.5",
            "\u{09e8}8:30:06Z",
            "",
        ] {
            assert!(!is_full_time(bad), "{bad}");
        }
    }

    #[test]
    fn a_minute_ends_at_59() {
        assert!(is_full_time("10:59:00Z") && !is_full_time("10:60:00Z"));
    }
}
