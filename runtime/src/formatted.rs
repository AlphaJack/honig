//! The text of a string format that the crate has no library type for. Only text of that format is one: `try_from`,
//! `parse` and decoding check it. It reads as a `str`.

use std::borrow::Borrow;
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::Deref;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::BuildError;
use crate::formats::Format;
use crate::formats::format;

/// The text of the format `F`. Only valid text is one.
pub struct Formatted<F>(String, PhantomData<F>);

impl<F> Formatted<F> {
    /// Returns the text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the text as a `String`.
    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl<F: Format> TryFrom<String> for Formatted<F> {
    type Error = BuildError;

    /// Errors: the text is not of the format.
    fn try_from(value: String) -> Result<Self, BuildError> {
        if crate::checks::is_format(&value, F::NAME) {
            Ok(Self(value, PhantomData))
        } else {
            Err(BuildError::value(format!("`{value}` is not a valid {}", F::NAME)))
        }
    }
}

impl<F: Format> TryFrom<&str> for Formatted<F> {
    type Error = BuildError;

    /// Errors: the text is not of the format.
    fn try_from(value: &str) -> Result<Self, BuildError> {
        Self::try_from(value.to_owned())
    }
}

impl<F: Format> FromStr for Formatted<F> {
    type Err = BuildError;

    fn from_str(s: &str) -> Result<Self, BuildError> {
        Self::try_from(s)
    }
}

impl<F> Deref for Formatted<F> {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl<F> AsRef<str> for Formatted<F> {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<F> Borrow<str> for Formatted<F> {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl<F> fmt::Display for Formatted<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

impl<F> fmt::Debug for Formatted<F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<F> Clone for Formatted<F> {
    fn clone(&self) -> Self {
        Self(self.0.clone(), PhantomData)
    }
}

impl<F> PartialEq for Formatted<F> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<F> Eq for Formatted<F> {}

impl<F> PartialOrd for Formatted<F> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<F> Ord for Formatted<F> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<F> Hash for Formatted<F> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl<F> PartialEq<str> for Formatted<F> {
    fn eq(&self, other: &str) -> bool {
        self.0 == other
    }
}

impl<F> PartialEq<&str> for Formatted<F> {
    fn eq(&self, other: &&str) -> bool {
        self.0 == *other
    }
}

impl<F> From<Formatted<F>> for String {
    fn from(value: Formatted<F>) -> Self {
        value.0
    }
}

impl<F> Serialize for Formatted<F> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de, F: Format> Deserialize<'de> for Formatted<F> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::try_from(text).map_err(serde::de::Error::custom)
    }
}

/// `format: uuid` without the `uuid` crate.
pub type UuidText = Formatted<format::Uuid>;
/// `format: date-time` without a date library.
pub type DateTimeText = Formatted<format::DateTime>;
/// `format: date` without a date library.
pub type DateText = Formatted<format::Date>;
/// `format: date-time-local` without a date library.
pub type DateTimeLocalText = Formatted<format::DateTimeLocal>;
/// `format: time-local` without a date library.
pub type TimeLocalText = Formatted<format::TimeLocal>;
/// `format: uri` without the `url` crate.
pub type UriText = Formatted<format::Uri>;
/// `format: uri-reference`.
pub type UriReferenceText = Formatted<format::UriReference>;
/// `format: iri`.
pub type IriText = Formatted<format::Iri>;
/// `format: iri-reference`.
pub type IriReferenceText = Formatted<format::IriReference>;
/// `format: ipv4-cidr` as its text.
pub type Ipv4CidrText = Formatted<format::Ipv4Cidr>;
/// `format: ipv6-cidr` as its text.
pub type Ipv6CidrText = Formatted<format::Ipv6Cidr>;
/// `format: email`. Checking it needs the `idn` feature.
pub type EmailText = Formatted<format::Email>;
/// `format: idn-email`. Checking it needs the `idn` feature.
pub type IdnEmailText = Formatted<format::IdnEmail>;
/// `format: hostname`. Checking it needs the `idn` feature.
pub type HostnameText = Formatted<format::Hostname>;
/// `format: idn-hostname`. Checking it needs the `idn` feature.
pub type IdnHostnameText = Formatted<format::IdnHostname>;
/// `{type: string, format: decimal}`: every digit, past what a decimal type holds.
pub type DecimalText = Formatted<format::Decimal>;
/// `{type: string, format: decimal128}`: a decimal that IEEE 754 decimal128 holds exactly, at most 34 significant
/// digits within its exponent range.
pub type Decimal128Text = Formatted<format::Decimal128>;
/// `format: http-date`.
pub type HttpDateText = Formatted<format::HttpDate>;
/// `format: duration`: no std type holds `P1M`, a month has no fixed length.
pub type DurationText = Formatted<format::Duration>;
/// `format: json-pointer`.
pub type JsonPointerText = Formatted<format::JsonPointer>;
/// `format: relative-json-pointer`.
pub type RelativeJsonPointerText = Formatted<format::RelativeJsonPointer>;
/// `format: regex`. Checking it needs the `regress` feature.
pub type RegexText = Formatted<format::Regex>;
/// `format: uri-template`.
pub type UriTemplateText = Formatted<format::UriTemplate>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_text_of_the_format_is_one() {
        let id = UuidText::try_from("01234567-89ab-4def-8123-456789abcdef").expect("hyphenated");
        assert_eq!(id, "01234567-89ab-4def-8123-456789abcdef");
        assert_eq!(id.len(), 36, "reads as a `str`");
        assert_eq!(
            UuidText::try_from("{01234567-89ab-4def-8123-456789abcdef}"),
            Err(BuildError::value("`{01234567-89ab-4def-8123-456789abcdef}` is not a valid uuid".to_owned()))
        );
        assert!("P1M".parse::<DurationText>().is_ok());
        assert!("1 month".parse::<DurationText>().is_err());
    }

    #[test]
    fn decoding_checks_the_text_and_encoding_writes_it_back() {
        let day: DateText = serde_json::from_str(r#""2024-06-19""#).expect("a date");
        assert_eq!(serde_json::to_string(&day).expect("encodes"), r#""2024-06-19""#);
        let err = serde_json::from_str::<DateText>(r#""2024-6-19""#).expect_err("not a full-date");
        assert!(err.to_string().starts_with("`2024-6-19` is not a valid date"), "{err}");
    }
}
