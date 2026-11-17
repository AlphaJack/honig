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

/// `format: time`: an RFC 3339 `full-time` such as `08:30:06.5+01:00`. It keeps its offset and its leap second.
///
/// A `jiff::civil::Time` has no offset and no second 60. So it holds the time and these two beside it.
#[cfg(feature = "jiff")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FullTime {
    /// The clock time where the offset applies. A leap second is 59 here.
    pub time: jiff::civil::Time,
    /// The offset east of UTC. `Z` and `-00:00` are zero.
    pub offset: jiff::tz::Offset,
    /// The second was 60: a leap second at the end of a UTC day.
    pub leap_second: bool,
}

/// Midnight in UTC: `00:00:00Z`.
#[cfg(feature = "jiff")]
impl Default for FullTime {
    fn default() -> Self {
        Self { time: jiff::civil::Time::midnight(), offset: jiff::tz::Offset::UTC, leap_second: false }
    }
}

/// Not an RFC 3339 `full-time`.
#[cfg(feature = "jiff")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotAFullTime(String);

#[cfg(feature = "jiff")]
impl std::fmt::Display for NotAFullTime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` is not a time with an offset (RFC 3339 `full-time`)", self.0)
    }
}

#[cfg(feature = "jiff")]
impl std::error::Error for NotAFullTime {}

/// Digits of a fraction that a time keeps: nanoseconds. More are cut off.
#[cfg(feature = "jiff")]
const NANO_DIGITS: usize = 9;

#[cfg(feature = "jiff")]
impl std::str::FromStr for FullTime {
    type Err = NotAFullTime;

    fn from_str(text: &str) -> Result<Self, NotAFullTime> {
        let wrong = || NotAFullTime(text.to_owned());
        if !is_full_time(text) {
            return Err(wrong());
        }
        let (clock, minutes) = split_offset(text).ok_or_else(wrong)?;
        let (clock, fraction) = clock.split_once('.').unwrap_or((clock, ""));
        let [h, m, s] = fields::<CLOCK_FIELDS>(clock).ok_or_else(wrong)?;
        let digits = fraction.get(..NANO_DIGITS).unwrap_or(fraction);
        let nanos = format!("{digits:0<NANO_DIGITS$}").parse::<i32>().ok().ok_or_else(wrong)?;
        let leap_second = s == MINUTE_SECONDS;
        let narrow = |n: i64| i8::try_from(n).ok().ok_or_else(wrong);
        let time = jiff::civil::Time::new(narrow(h)?, narrow(m)?, narrow(s.min(MINUTE_SECONDS - 1))?, nanos)
            .ok()
            .ok_or_else(wrong)?;
        let offset = i32::try_from(minutes * MINUTE_SECONDS)
            .ok()
            .and_then(|seconds| jiff::tz::Offset::from_seconds(seconds).ok())
            .ok_or_else(wrong)?;
        Ok(Self { time, offset, leap_second })
    }
}

#[cfg(feature = "jiff")]
impl std::fmt::Display for FullTime {
    /// Writes `HH:MM:SS`, the fraction when there is one and `Z` or `+HH:MM`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let t = self.time;
        let second = if self.leap_second { MINUTE_SECONDS } else { i64::from(t.second()) };
        write!(f, "{:02}:{:02}:{second:02}", t.hour(), t.minute())?;
        if t.subsec_nanosecond() != 0 {
            let fraction = format!("{:09}", t.subsec_nanosecond());
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        let seconds = self.offset.seconds();
        if seconds == 0 {
            return f.write_str("Z");
        }
        let minutes = i64::from(seconds).abs() / MINUTE_SECONDS;
        let sign = if seconds < 0 { '-' } else { '+' };
        write!(f, "{sign}{:02}:{:02}", minutes / HOUR_MINUTES, minutes % HOUR_MINUTES)
    }
}

#[cfg(feature = "jiff")]
impl serde::Serialize for FullTime {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "jiff")]
impl<'de> serde::Deserialize<'de> for FullTime {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
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

    #[cfg(feature = "jiff")]
    #[test]
    fn a_full_time_keeps_its_offset_fraction_and_leap_second() {
        let read = |text: &str| text.parse::<FullTime>().map(|t| t.to_string());
        assert_eq!(read("08:30:06Z").as_deref(), Ok("08:30:06Z"));
        assert_eq!(read("08:30:06.250+01:30").as_deref(), Ok("08:30:06.25+01:30"));
        assert_eq!(read("15:59:60-08:00").as_deref(), Ok("15:59:60-08:00"));
        assert_eq!(read("08:30:06z").as_deref(), Ok("08:30:06Z"));
        assert_eq!(read("08:30:06.1234567891Z").as_deref(), Ok("08:30:06.123456789Z"), "past nanoseconds: cut");
        let t: FullTime = "08:30:06+02:00".parse().unwrap();
        assert_eq!((t.time, t.offset.seconds(), t.leap_second), (jiff::civil::time(8, 30, 6, 0), 7200, false));
        assert_eq!(
            "08:30:06".parse::<FullTime>().unwrap_err().to_string(),
            "`08:30:06` is not a time with an offset (RFC 3339 `full-time`)"
        );
        assert!("22:59:60Z".parse::<FullTime>().is_err(), "a leap second ends a UTC day");
        assert_eq!(FullTime::default().to_string(), "00:00:00Z");
    }

    #[cfg(feature = "jiff")]
    #[test]
    fn a_full_time_is_json_text() {
        let t: FullTime = serde_json::from_str("\"23:59:60Z\"").unwrap();
        assert!(t.leap_second);
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"23:59:60Z\"");
        assert!(serde_json::from_str::<FullTime>("\"10:00:00\"").is_err());
    }
}
