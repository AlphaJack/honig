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
/// It stores plain numbers, so no date library is part of it. With the feature `jiff`, its methods `time` and `offset`
/// return it as that library's types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct FullTime {
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
    /// 0 to 59. A leap second is 59 here and [`FullTime::leap_second`].
    pub second: u8,
    /// The fraction of the second: 0 to 999,999,999.
    pub nanosecond: u32,
    /// The offset east of UTC in seconds. `Z` and `-00:00` are zero.
    pub offset_seconds: i32,
    /// The second was 60: a leap second at the end of a UTC day.
    pub leap_second: bool,
}

/// Digits of a fraction that a time keeps: nanoseconds. More are cut off.
const NANO_DIGITS: usize = 9;

impl std::str::FromStr for FullTime {
    type Err = crate::BuildError;

    fn from_str(text: &str) -> Result<Self, crate::BuildError> {
        let wrong =
            || crate::BuildError::value(format!("`{text}` is not a time with an offset (RFC 3339 `full-time`)"));
        if !is_full_time(text) {
            return Err(wrong());
        }
        let (clock, minutes) = split_offset(text).ok_or_else(wrong)?;
        let (clock, fraction) = clock.split_once('.').unwrap_or((clock, ""));
        let [h, m, s] = fields::<CLOCK_FIELDS>(clock).ok_or_else(wrong)?;
        let digits = fraction.get(..NANO_DIGITS).unwrap_or(fraction);
        let nanosecond = format!("{digits:0<NANO_DIGITS$}").parse::<u32>().ok().ok_or_else(wrong)?;
        let narrow = |n: i64| u8::try_from(n).ok().ok_or_else(wrong);
        let offset_seconds = i32::try_from(minutes * MINUTE_SECONDS).ok().ok_or_else(wrong)?;
        Ok(Self {
            hour: narrow(h)?,
            minute: narrow(m)?,
            second: narrow(s.min(MINUTE_SECONDS - 1))?,
            nanosecond,
            offset_seconds,
            leap_second: s == MINUTE_SECONDS,
        })
    }
}

impl std::fmt::Display for FullTime {
    /// Writes `HH:MM:SS`, the fraction when there is one and `Z` or `+HH:MM`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let second = if self.leap_second { MINUTE_SECONDS } else { i64::from(self.second) };
        write!(f, "{:02}:{:02}:{second:02}", self.hour, self.minute)?;
        if self.nanosecond != 0 {
            let fraction = format!("{:09}", self.nanosecond);
            write!(f, ".{}", fraction.trim_end_matches('0'))?;
        }
        if self.offset_seconds == 0 {
            return f.write_str("Z");
        }
        let minutes = i64::from(self.offset_seconds).abs() / MINUTE_SECONDS;
        let sign = if self.offset_seconds < 0 { '-' } else { '+' };
        write!(f, "{sign}{:02}:{:02}", minutes / HOUR_MINUTES, minutes % HOUR_MINUTES)
    }
}

impl serde::Serialize for FullTime {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for FullTime {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "jiff")]
impl FullTime {
    /// Returns the clock time as `jiff` represents one. A leap second is its second 59.
    #[must_use]
    pub fn time(&self) -> jiff::civil::Time {
        let narrow = |n: u8| i8::try_from(n).unwrap_or(i8::MAX);
        let nanosecond = i32::try_from(self.nanosecond).unwrap_or_default();
        jiff::civil::Time::new(narrow(self.hour), narrow(self.minute), narrow(self.second), nanosecond)
            .unwrap_or_default()
    }

    /// Returns the offset as `jiff` represents one.
    #[must_use]
    pub fn offset(&self) -> jiff::tz::Offset {
        jiff::tz::Offset::from_seconds(self.offset_seconds).unwrap_or(jiff::tz::Offset::UTC)
    }
}

/// A time and its offset from `jiff`. No leap second: `jiff` has none.
#[cfg(feature = "jiff")]
impl From<(jiff::civil::Time, jiff::tz::Offset)> for FullTime {
    fn from((time, offset): (jiff::civil::Time, jiff::tz::Offset)) -> Self {
        let narrow = |n: i8| u8::try_from(n).unwrap_or_default();
        Self {
            hour: narrow(time.hour()),
            minute: narrow(time.minute()),
            second: narrow(time.second()),
            nanosecond: u32::try_from(time.subsec_nanosecond()).unwrap_or_default(),
            offset_seconds: offset.seconds(),
            leap_second: false,
        }
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
    let mut parts = text.split(':');
    let mut out = [0; N];
    for o in &mut out {
        let p = parts.next()?;
        if p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *o = p.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
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

    /// A time with its offset is plain numbers: no library type, with or without a feature.
    #[test]
    fn a_full_time_is_plain_numbers() {
        let t = FullTime { hour: 8, minute: 30, second: 6, nanosecond: 0, offset_seconds: 7200, leap_second: false };
        assert_eq!(t.to_string(), "08:30:06+02:00");
        assert_eq!("08:30:06+02:00".parse::<FullTime>(), Ok(t));
    }

    #[test]
    fn a_full_time_keeps_its_offset_fraction_and_leap_second() {
        let read = |text: &str| text.parse::<FullTime>().map(|t| t.to_string());
        assert_eq!(read("08:30:06Z").as_deref(), Ok("08:30:06Z"));
        assert_eq!(read("08:30:06.250+01:30").as_deref(), Ok("08:30:06.25+01:30"));
        assert_eq!(read("15:59:60-08:00").as_deref(), Ok("15:59:60-08:00"));
        assert_eq!(read("08:30:06z").as_deref(), Ok("08:30:06Z"));
        assert_eq!(read("08:30:06.1234567891Z").as_deref(), Ok("08:30:06.123456789Z"), "past nanoseconds: cut");
        let t: FullTime = "15:59:60-08:00".parse().unwrap();
        assert_eq!((t.hour, t.minute, t.second, t.offset_seconds, t.leap_second), (15, 59, 59, -28_800, true));
        assert_eq!(
            "08:30:06".parse::<FullTime>().unwrap_err().to_string(),
            "`08:30:06` is not a time with an offset (RFC 3339 `full-time`)"
        );
        assert!("22:59:60Z".parse::<FullTime>().is_err(), "a leap second ends a UTC day");
        assert_eq!(FullTime::default().to_string(), "00:00:00Z");
    }

    #[test]
    fn a_full_time_is_json_text() {
        let t: FullTime = serde_json::from_str("\"23:59:60Z\"").unwrap();
        assert!(t.leap_second);
        assert_eq!(serde_json::to_string(&t).unwrap(), "\"23:59:60Z\"");
        assert!(serde_json::from_str::<FullTime>("\"10:00:00\"").is_err());
    }

    /// With `jiff`, the time and the offset convert to its types and build one back.
    #[cfg(feature = "jiff")]
    #[test]
    fn a_full_time_reads_as_jiff() {
        let t: FullTime = "08:30:06.5+01:30".parse().unwrap();
        assert_eq!(t.time(), jiff::civil::time(8, 30, 6, 500_000_000));
        assert_eq!(t.offset().seconds(), 5400);
        assert_eq!(FullTime::from((t.time(), t.offset())), t);
    }
}
