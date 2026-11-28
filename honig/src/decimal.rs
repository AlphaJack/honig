//! `type: number, format: decimal` (and `decimal128`) exact from end to end.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize, Serializer};

/// The raw value of `serde_json` asked for by name. It gives the number as written and not as an `f64`.
use crate::raw_value::RAW_VALUE as RAW;

/// A JSON number kept as its digits (`type: number, format: decimal`). `12345678901234567.89` arrives and leaves
/// as written. An `f64` rounds it.
///
/// Exact in JSON bodies, parameters, form and multipart fields and `oneOf` branches with or without the
/// discriminator first. It rounds only in the `Other` of a union. That is a value that no branch accepts. It is kept
/// as a `serde_json::Value`. For arithmetic parse [`Decimal::as_str`] into the decimal type of your choice. Parse
/// its text back with `parse`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Decimal(String);

impl Decimal {
    /// Returns the digits as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the nearest `f64` for comparisons that do not need every digit.
    #[must_use]
    pub fn as_f64(&self) -> f64 {
        self.0.parse().unwrap_or(f64::NAN)
    }

    /// Creates the decimal nearest an `f64`. `None` for `NaN` and the infinities because they are no JSON number.
    #[must_use]
    pub fn from_f64(f: f64) -> Option<Self> {
        serde_json::Number::from_f64(f).map(|n| Self(n.to_string()))
    }
}

impl Default for Decimal {
    /// Returns zero.
    fn default() -> Self {
        Self("0".into())
    }
}

/// Returns JSON number syntax (RFC 8259 section 6): `-`, digits without a leading zero, a fraction and an exponent.
pub(crate) fn is_json_number(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(b.first() == Some(&b'-'));
    let digits = |i: &mut usize| {
        let start = *i;
        while b.get(*i).is_some_and(u8::is_ascii_digit) {
            *i += 1;
        }
        *i - start
    };
    let int_start = i;
    let int = digits(&mut i);
    if int == 0 || (int > 1 && b[int_start] == b'0') {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

impl FromStr for Decimal {
    type Err = crate::BuildError;
    fn from_str(s: &str) -> Result<Self, crate::BuildError> {
        if is_json_number(s) {
            Ok(Self(s.to_owned()))
        } else {
            Err(crate::BuildError::value(format!("`{s}` is not a decimal number")))
        }
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<i64> for Decimal {
    fn from(n: i64) -> Self {
        Self(n.to_string())
    }
}

impl From<u64> for Decimal {
    fn from(n: u64) -> Self {
        Self(n.to_string())
    }
}

impl Serialize for Decimal {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // serde_json writes a raw value as it is with the digits unrounded. Other serializers see a string.
        serde_json::value::RawValue::from_string(self.0.clone()).map_err(serde::ser::Error::custom)?.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Decimal {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_newtype_struct(RAW, DecimalVisitor)
    }
}

struct DecimalVisitor;

impl<'de> Visitor<'de> for DecimalVisitor {
    type Value = Decimal;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a decimal number")
    }

    // serde_json asked for the raw text gives a one-entry map that contains the number as written.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Decimal, A::Error> {
        let Some((_, raw)) = map.next_entry::<String, String>()? else { return Err(de::Error::custom("no number")) };
        // From a `serde_json::Value` that contains text (a parameter parsed from a query string) the raw JSON is a
        // string.
        let text =
            if raw.starts_with('"') { serde_json::from_str::<String>(&raw).map_err(de::Error::custom)? } else { raw };
        text.parse().map_err(de::Error::custom)
    }

    // Any other deserializer: what it decoded.
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Decimal, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_i64<E: de::Error>(self, n: i64) -> Result<Decimal, E> {
        Ok(Decimal::from(n))
    }

    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Decimal, E> {
        Ok(Decimal::from(n))
    }

    fn visit_f64<E: de::Error>(self, n: f64) -> Result<Decimal, E> {
        Decimal::from_f64(n).ok_or_else(|| de::Error::custom(format!("{n} is not a JSON number")))
    }

    // A number written as text such as a query, path, header or form value.
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Decimal, E> {
        s.parse().map_err(E::custom)
    }
}

/// A JSON number that IEEE 754 decimal128 represents exactly (`type: number, format: decimal128`): at most 34
/// significant digits within its exponent range.
///
/// It keeps its digits as written, as [`Decimal`] does. Building one with `parse` or `try_from` and decoding one check
/// the limits, so every value of it is in range.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Decimal128(Decimal);

impl std::ops::Deref for Decimal128 {
    type Target = Decimal;
    fn deref(&self) -> &Decimal {
        &self.0
    }
}

impl TryFrom<Decimal> for Decimal128 {
    type Error = crate::BuildError;
    fn try_from(d: Decimal) -> Result<Self, crate::BuildError> {
        match crate::checks::decimal128(d.as_str()) {
            Ok(()) => Ok(Self(d)),
            Err(why) => Err(crate::BuildError::value(format!("the number {why}"))),
        }
    }
}

impl FromStr for Decimal128 {
    type Err = crate::BuildError;
    fn from_str(s: &str) -> Result<Self, crate::BuildError> {
        Self::try_from(s.parse::<Decimal>()?)
    }
}

impl From<Decimal128> for Decimal {
    fn from(d: Decimal128) -> Self {
        d.0
    }
}

impl From<i64> for Decimal128 {
    /// An `i64` has at most 19 digits: decimal128 represents each.
    fn from(n: i64) -> Self {
        Self(n.into())
    }
}

impl From<u64> for Decimal128 {
    /// A `u64` has at most 20 digits: decimal128 represents each.
    fn from(n: u64) -> Self {
        Self(n.into())
    }
}

impl fmt::Display for Decimal128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Serialize for Decimal128 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Decimal128 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::try_from(Decimal::deserialize(d)?).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parsing fails with the error that every way of building a value fails with.
    #[test]
    fn a_decimal_that_does_not_parse_is_a_build_error() {
        let parse = |s: &str| -> Result<Decimal, crate::BuildError> { s.parse() };
        assert!(parse("1.50").is_ok());
        assert_eq!(parse("x").unwrap_err().to_string(), "`x` is not a decimal number");
    }

    const WIDE: &str = "12345678901234567.89";

    /// A decimal128 number keeps every digit and refuses one that decimal128 does not represent, however it is built.
    #[test]
    fn a_decimal128_holds_to_its_limits() {
        let at_most = "1234567890123456789012345678901234";
        let past = "123456789012345678901234567890123456789";
        let decoded: Decimal128 = serde_json::from_str(at_most).unwrap();
        assert_eq!((decoded.as_str(), serde_json::to_string(&decoded).unwrap().as_str()), (at_most, at_most));
        let refused = serde_json::from_str::<Decimal128>(past).unwrap_err().to_string();
        assert!(refused.contains("39 significant digits") && !refused.contains(past), "{refused}");
        assert!(past.parse::<Decimal128>().is_err());
        assert!(Decimal128::try_from(past.parse::<Decimal>().unwrap()).is_err());
        assert!(at_most.parse::<Decimal128>().is_ok());
    }

    #[test]
    fn every_digit_survives_a_json_body() {
        #[derive(Debug, PartialEq, Serialize, Deserialize)]
        struct Price {
            amount: Decimal,
            all: Vec<Decimal>,
        }
        let text = format!(r#"{{"amount":{WIDE},"all":[0.1,1e400,{WIDE}]}}"#);
        let p: Price = serde_json::from_str(&text).unwrap();
        assert_eq!(p.amount.as_str(), WIDE);
        assert_eq!(p.all.iter().map(Decimal::as_str).collect::<Vec<_>>(), ["0.1", "1e400", WIDE]);
        assert_eq!(serde_json::to_string(&p).unwrap(), text, "written back as it came");
    }

    #[test]
    fn text_is_json_number_syntax_only() {
        let from_text: Decimal = serde_json::from_value(serde_json::json!(WIDE)).unwrap();
        assert_eq!(from_text.as_str(), WIDE);
        for good in ["0", "-0.5", "1E+3", "12.000"] {
            assert!(good.parse::<Decimal>().is_ok(), "{good}");
        }
        for bad in ["", "abc", "+1", "01", "1.", ".5", "NaN", "inf", "1e", "1.2.3", " 1", "1 "] {
            assert!(bad.parse::<Decimal>().is_err(), "{bad}");
        }
    }

    #[test]
    fn integers_are_decimals_as_written() {
        use serde::de::IntoDeserializer;
        assert_eq!((Decimal::from(-5_i64).to_string(), Decimal::from(5_u64).to_string()), ("-5".into(), "5".into()));
        let read = Decimal::deserialize(IntoDeserializer::<de::value::Error>::into_deserializer(-7_i64));
        assert_eq!(read.map(|d| d.to_string()), Ok("-7".to_owned()));
        assert_eq!("x".parse::<Decimal>().unwrap_err().to_string(), "`x` is not a decimal number");
    }
}
