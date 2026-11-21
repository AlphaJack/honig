//! HTTP dates (RFC 9110 section 5.6.7): the calendar that conditional requests compare them with and the check of
//! `format: http-date`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Hours in a day.
pub(crate) const DAY_HOURS: i64 = 24;
/// Minutes in an hour.
pub(crate) const HOUR_MINUTES: i64 = 60;
/// Seconds in a minute and the highest second that an HTTP date may contain. A leap second is `60`.
pub(crate) const MINUTE_SECONDS: i64 = 60;
/// Seconds in an hour.
pub(crate) const HOUR_SECONDS: i64 = HOUR_MINUTES * MINUTE_SECONDS;
/// Seconds in a day.
pub(crate) const DAY_SECONDS: i64 = DAY_HOURS * HOUR_SECONDS;
/// How far ahead an RFC 850 date's two-digit year may be before it counts as the century before.
const RFC850_YEARS_AHEAD: i64 = 50;
/// Digits of a four-digit year.
pub(crate) const YEAR_DIGITS: usize = 4;
/// Hour, minute and second.
const CLOCK_FIELDS: usize = 3;
/// Day, month, year and clock of IMF-fixdate.
const IMF_FIELDS: usize = 4;
/// Day, month and two-digit year of the RFC 850 date.
const RFC850_DATE_FIELDS: usize = 3;
/// The day names of IMF-fixdate and asctime.
const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
/// The day names of the RFC 850 form.
const LONG_DAY_NAMES: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

/// Is `text` an HTTP date as its grammar writes it? That is IMF-fixdate, the obsolete RFC 850 form or asctime with a
/// day that its month has. A conditional request parses dates more loosely: RFC 9110 asks recipients to be robust.
pub(crate) fn is_http_date(text: &str) -> bool {
    let Some((day, month, year, clock)) = imf_fixdate(text).or_else(|| rfc850(text)).or_else(|| asctime(text)) else {
        return false;
    };
    let Some(month) = MONTHS.iter().position(|m| *m == month).and_then(|m| i64::try_from(m).ok()) else {
        return false;
    };
    let Some([h, m, s]) = parts::<CLOCK_FIELDS>(clock, ':').and_then(two_digits) else { return false };
    (1..=days_in_month(year, month + 1)).contains(&day) && h < DAY_HOURS && m < HOUR_MINUTES && s <= MINUTE_SECONDS
}

/// Returns `Sun, 06 Nov 1994 08:49:37 GMT`: day, month, year and clock.
fn imf_fixdate(text: &str) -> Option<(i64, &str, i64, &str)> {
    let (name, rest) = text.strip_suffix(" GMT")?.split_once(", ")?;
    let [day, month, year, clock] = parts::<IMF_FIELDS>(rest, ' ')?;
    DAY_NAMES.contains(&name).then_some((digits(day, 2)?, month, digits(year, YEAR_DIGITS)?, clock))
}

/// Returns `Sunday, 06-Nov-94 08:49:37 GMT`.
fn rfc850(text: &str) -> Option<(i64, &str, i64, &str)> {
    let (name, rest) = text.strip_suffix(" GMT")?.split_once(", ")?;
    let [date, clock] = parts::<2>(rest, ' ')?;
    let [day, month, yy] = parts::<RFC850_DATE_FIELDS>(date, '-')?;
    LONG_DAY_NAMES.contains(&name).then_some((digits(day, 2)?, month, two_digit_year(digits(yy, 2)?), clock))
}

/// Returns `Sun Nov  6 08:49:37 1994`: a day below 10 is a space and one digit.
fn asctime(text: &str) -> Option<(i64, &str, i64, &str)> {
    let (name, rest) = text.split_once(' ')?;
    let (month, rest) = rest.split_once(' ')?;
    let (day, rest) = match rest.strip_prefix(' ') {
        Some(rest) => (digits(rest.get(..1)?, 1)?, rest.get(1..)?),
        None => (digits(rest.get(..2)?, 2)?, rest.get(2..)?),
    };
    let [clock, year] = parts::<2>(rest.strip_prefix(' ')?, ' ')?;
    DAY_NAMES.contains(&name).then_some((day, month, digits(year, YEAR_DIGITS)?, clock))
}

/// Splits `text` at `sep` into exactly `N` pieces.
fn parts<const N: usize>(text: &str, sep: char) -> Option<[&str; N]> {
    text.split(sep).collect::<Vec<_>>().try_into().ok()
}

/// Returns two-digit pieces as numbers.
fn two_digits<const N: usize>(pieces: [&str; N]) -> Option<[i64; N]> {
    let mut out = [0; N];
    for (o, p) in out.iter_mut().zip(pieces) {
        *o = digits(p, 2)?;
    }
    Some(out)
}

/// Returns `text` as a number when it is `len` ASCII digits.
fn digits(text: &str, len: usize) -> Option<i64> {
    (text.len() == len && text.bytes().all(|b| b.is_ascii_digit())).then(|| text.parse().ok()).flatten()
}

/// Days in `month` (1 to 12) of `year`.
fn days_in_month(year: i64, month: i64) -> i64 {
    let (next_year, next_month) = if month == YEAR_MONTHS { (year + 1, 1) } else { (year, month + 1) };
    days_from_civil(next_year, next_month, 1) - days_from_civil(year, month, 1)
}

/// The month names of an HTTP date, January first.
pub(crate) const MONTHS: [&str; 12] =
    ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Returns the year of an RFC 850 date: one more than 50 years ahead is the most recent past year with those last two
/// digits (RFC 9110 section 5.6.7).
pub(crate) fn two_digit_year(yy: i64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or_default();
    let this_year = civil_year(now.div_euclid(DAY_SECONDS));
    let century = this_year - this_year.rem_euclid(100);
    let year = century + yy;
    if year > this_year + RFC850_YEARS_AHEAD { year - 100 } else { year }
}

/// Years after which the Gregorian calendar repeats: an era.
const ERA_YEARS: i64 = 400;
/// Days in an era.
const ERA_DAYS: i64 = 146_097;
/// Days in a common year.
const YEAR_DAYS: i64 = 365;
/// Years from one leap year to the next. A century that is not a whole era can come between.
const LEAP_EVERY: i64 = 4;
/// Days from 0000-03-01 to 1970-01-01. The computed years start at 0000-03-01.
const EPOCH_SHIFT: i64 = 719_468;
/// Months in a year.
const YEAR_MONTHS: i64 = 12;
/// The first month of a computed year. It is March so that a leap day is the last day of the year.
const MARCH: i64 = 3;
/// Months after March whose lengths repeat: 31, 30, 31, 30, 31.
const FIVE_MONTHS: i64 = 5;
/// Days in those months.
const FIVE_MONTH_DAYS: i64 = 153;

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month < MARCH { year - 1 } else { year };
    let era = y.div_euclid(ERA_YEARS);
    let yoe = y - era * ERA_YEARS;
    let mp = (month + YEAR_MONTHS - MARCH) % YEAR_MONTHS;
    let doy = (FIVE_MONTH_DAYS * mp + 2) / FIVE_MONTHS + day - 1;
    let doe = yoe * YEAR_DAYS + yoe / LEAP_EVERY - yoe / 100 + doy;
    era * ERA_DAYS + doe - EPOCH_SHIFT
}

/// Returns the year a count of days since 1970-01-01 falls in (Howard Hinnant's `civil_from_days`).
fn civil_year(days: i64) -> i64 {
    let z = days + EPOCH_SHIFT;
    let era = z.div_euclid(ERA_DAYS);
    let doe = z - era * ERA_DAYS;
    // Take out leap days. One goes out each 4 years and comes back each century. It goes out again for the last day of
    // the era.
    let leap_days =
        doe / (LEAP_EVERY * YEAR_DAYS) - doe / (100 * YEAR_DAYS + 100 / LEAP_EVERY - 1) + doe / (ERA_DAYS - 1);
    let yoe = (doe - leap_days) / YEAR_DAYS;
    let doy = doe - (YEAR_DAYS * yoe + yoe / LEAP_EVERY - yoe / 100);
    let mp = (FIVE_MONTHS * doy + 2) / FIVE_MONTH_DAYS;
    let month = (mp + MARCH - 1) % YEAR_MONTHS + 1;
    yoe + era * ERA_YEARS + i64::from(month < MARCH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_converts_to_days_and_back_to_its_year() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2024, 2, 29), 19_782);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        for year in [-401, -1, 0, 1, 1600, 1900, 1999, 2000, 2100, 2400] {
            for (month, day) in [(1, 1), (2, 28), (3, 1), (12, 31)] {
                assert_eq!(civil_year(days_from_civil(year, month, day)), year, "{year}-{month}-{day}");
            }
        }
    }

    /// Every day of two eras: a slip in the leap days moves some day into the wrong year.
    #[test]
    fn every_day_falls_in_its_year() {
        let mut year = 1600;
        let mut next = days_from_civil(year + 1, 1, 1);
        for day in days_from_civil(year, 1, 1)..days_from_civil(2401, 1, 1) {
            if day == next {
                year += 1;
                next = days_from_civil(year + 1, 1, 1);
            }
            assert_eq!(civil_year(day), year, "day {day}");
        }
    }

    #[test]
    fn a_two_digit_year_is_never_more_than_50_years_ahead() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
            .unwrap_or_default();
        let this_year = civil_year(now.div_euclid(86_400));
        assert_eq!(two_digit_year(this_year % 100), this_year);
        assert_eq!(two_digit_year((this_year + 51) % 100), this_year + 51 - 100);
        assert_eq!(two_digit_year((this_year + 50) % 100), this_year + 50);
    }

    #[test]
    fn a_date_checks_as_its_grammar_writes_it() {
        for good in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun Nov 16 08:49:37 1994",
            "Thu, 29 Feb 2024 00:00:00 GMT",
            "Sat, 31 Dec 2016 23:59:60 GMT",
        ] {
            assert!(is_http_date(good), "{good}");
        }
        for bad in [
            "",
            "yesterday",
            "Sun, 06 Nov 1994 08:49:37 CET",
            "Sun, 6 Nov 1994 08:49:37 GMT",
            "Sun,  06 Nov 1994 08:49:37 GMT",
            "Sun, 06 nov 1994 08:49:37 GMT",
            "Sunday, 06 Nov 1994 08:49:37 GMT",
            "Sun, 06-Nov-94 08:49:37 GMT",
            "Sun Nov 6 08:49:37 1994",
            "Thu, 29 Feb 2023 00:00:00 GMT",
            "Sun, 31 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Nov 1994 8:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:61 GMT",
            "Sun, 06 Nov 1994 08:60:00 GMT",
            "1994-11-06T08:49:37Z",
        ] {
            assert!(!is_http_date(bad), "{bad}");
        }
    }
}
