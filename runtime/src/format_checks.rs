//! RFC 3339 dates and the local forms, and IP addresses, checked by their grammar with no library. A `time` with its
//! offset is [`crate::full_time`]. JSON Schema 2020-12 section 7.2.1 lets a validator assert `format`.

/// Days of each month in a common year.
const DAYS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// A leap year is a multiple of 4, but not of 100 unless of 400 (proleptic Gregorian).
const LEAP: (u32, u32, u32) = (4, 100, 400);

/// Digits of a year and of every other field of a date or time.
const YEAR_DIGITS: usize = 4;
/// Digits of a month, day, hour, minute or second.
const FIELD_DIGITS: usize = 2;
/// The last hour, minute and second (`60` is the leap second).
const LAST: (u32, u32, u32) = (23, 59, 60);

/// Days in `month` (1 to 12) of `year`.
fn days_in(year: u32, month: u32) -> u32 {
    let (four, hundred, four_hundred) = LEAP;
    let leap = year.is_multiple_of(four) && (!year.is_multiple_of(hundred) || year.is_multiple_of(four_hundred));
    let days = month.checked_sub(1).and_then(|i| DAYS.get(usize::try_from(i).ok()?)).copied().unwrap_or(0);
    if leap && month == 2 { days + 1 } else { days }
}

/// Returns `n` ASCII digits at the start of `s` as a number, and the rest.
fn digits(s: &str, n: usize) -> Option<(u32, &str)> {
    let (head, rest) = (s.get(..n)?, s.get(n..)?);
    head.bytes().all(|b| b.is_ascii_digit()).then(|| (head.parse().ok(), rest)).and_then(|(v, r)| Some((v?, r)))
}

/// Returns RFC 3339 `full-date`: `YYYY-MM-DD` with a real day.
#[must_use]
pub(crate) fn is_date(s: &str) -> bool {
    let parsed = (|| {
        let (y, s) = digits(s, YEAR_DIGITS)?;
        let (m, s) = digits(s.strip_prefix('-')?, FIELD_DIGITS)?;
        let (d, s) = digits(s.strip_prefix('-')?, FIELD_DIGITS)?;
        Some(s.is_empty() && days_in(y, m) > 0 && (1..=days_in(y, m)).contains(&d))
    })();
    parsed == Some(true)
}

/// Returns `HH:MM:SS` and an optional `.fraction`: (hour, minute, second, rest).
fn partial_time(s: &str) -> Option<(u32, u32, u32, &str)> {
    let (h, s) = digits(s, FIELD_DIGITS)?;
    let (m, s) = digits(s.strip_prefix(':')?, FIELD_DIGITS)?;
    let (sec, mut s) = digits(s.strip_prefix(':')?, FIELD_DIGITS)?;
    if let Some(frac) = s.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = frac.get(n..)?;
    }
    let (last_hour, last_minute, leap_second) = LAST;
    (h <= last_hour && m <= last_minute && sec <= leap_second).then_some((h, m, sec, s))
}

/// Returns a time without an offset (`OpenAPI`'s `time-local`).
#[must_use]
pub(crate) fn is_time_local(s: &str) -> bool {
    let (_, last_minute, _) = LAST;
    partial_time(s).is_some_and(|(_, _, sec, rest)| rest.is_empty() && sec <= last_minute)
}

/// Returns RFC 3339 `date-time`: a date, `T` (or `t`) and a full time.
#[must_use]
pub(crate) fn is_date_time(s: &str) -> bool {
    s.split_once(['T', 't']).is_some_and(|(d, t)| is_date(d) && crate::full_time::is_full_time(t))
}

/// Returns a date-time without an offset (`OpenAPI`'s `date-time-local`).
#[must_use]
pub(crate) fn is_date_time_local(s: &str) -> bool {
    s.split_once(['T', 't']).is_some_and(|(d, t)| is_date(d) && is_time_local(t))
}

/// The octets of an IPv4 address.
const OCTETS: usize = 4;

/// Returns RFC 2673 dotted quad: four decimal octets without leading zeros.
#[must_use]
pub(crate) fn is_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    parts.len() == OCTETS
        && parts.iter().all(|p| {
            !p.is_empty()
                && p.bytes().all(|b| b.is_ascii_digit())
                && (p.len() == 1 || !p.starts_with('0'))
                && p.parse::<u8>().is_ok()
        })
}

/// Returns RFC 4291 section 2.2 text forms, with `::` and an IPv4 tail.
#[must_use]
pub(crate) fn is_ipv6(s: &str) -> bool {
    s.is_ascii() && s.parse::<std::net::Ipv6Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_addresses_follow_their_rfcs() {
        assert!(is_date("2020-02-29") && !is_date("2021-02-29") && !is_date("2020-13-01") && !is_date("20-01-01"));
        assert!(is_date_time("1963-06-19T08:30:06.283185Z") && !is_date_time("1963-06-19 08:30:06Z"));
        assert!(
            is_time_local("12:30:00.5") && !is_time_local("12:30:00Z") && is_date_time_local("2024-06-01t12:30:00")
        );
        assert!(is_ipv4("192.168.0.1") && !is_ipv4("087.10.0.1") && !is_ipv4("256.0.0.1"));
        assert!(is_ipv6("::ffff:192.168.0.1") && !is_ipv6("12345::"));
    }

    #[test]
    fn a_local_time_has_hours_and_minutes_in_range() {
        assert!(is_time_local("23:59:59"));
        assert!(!is_time_local("24:00:00") && !is_time_local("12:60:00"));
    }
}
