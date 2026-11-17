//! A property that is optional and nullable. Absent, `null` and a value are kept apart.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An optional and nullable property. Absent and `null` mean different things on the wire. `PATCH` leaves an absent
/// field alone and clears a `null` one. So each has its own variant.
///
/// Fields of this type are `#[serde(default, skip_serializing_if = "Nullable::is_absent")]`: absent decodes as
/// [`Nullable::Absent`] and is not written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Nullable<T> {
    /// Not sent.
    #[default]
    Absent,
    /// Sent as `null`.
    Null,
    /// Sent with a value.
    Value(T),
}

impl<T> Nullable<T> {
    /// Was the property left out?
    #[must_use]
    pub const fn is_absent(&self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Returns a reference to the value that keeps absent and `null` apart.
    #[must_use]
    pub const fn as_ref(&self) -> Nullable<&T> {
        match self {
            Self::Value(v) => Nullable::Value(v),
            Self::Absent => Nullable::Absent,
            Self::Null => Nullable::Null,
        }
    }

    /// Returns the value when there is one.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        self.as_ref().into_value()
    }

    /// Returns the value when there is one. Absent and `null` both give `None`.
    #[must_use]
    pub fn into_value(self) -> Option<T> {
        match self {
            Self::Value(v) => Some(v),
            Self::Absent | Self::Null => None,
        }
    }
}

/// `None` is `null`: a value that is sent either way.
impl<T> From<Option<T>> for Nullable<T> {
    fn from(v: Option<T>) -> Self {
        v.map_or(Self::Null, Self::Value)
    }
}

impl<T: Serialize> Serialize for Nullable<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Value(v) => v.serialize(s),
            Self::Absent | Self::Null => s.serialize_none(),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(d).map(Self::from)
    }
}

/// Returns `deserialize_with` of a required property that may be `null`.
///
/// serde reads a missing `Option` field as `None` unless the field has its own deserializer: with this one,
/// leaving the property out is an error and `null` is `None`.
///
/// # Errors
/// The value is neither `null` nor a `T`.
pub fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// Returns `deserialize_with` of an optional property that may not be `null`.
///
/// `#[serde(default)]` reads it absent as `None`; `null` is refused with `T`'s own error.
///
/// # Errors
/// The value is not a `T`; `null` is none.
pub fn not_null<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Deserialize)]
    struct Strict {
        #[serde(deserialize_with = "required_nullable")]
        must: Option<u8>,
        #[serde(default, deserialize_with = "not_null")]
        may: Option<u8>,
    }

    #[test]
    fn a_required_nullable_property_must_be_sent_and_an_optional_one_may_not_be_null() {
        let ok = |json: &str| serde_json::from_str::<Strict>(json).ok();
        assert_eq!(ok(r#"{"must":null}"#), Some(Strict { must: None, may: None }));
        assert_eq!(ok(r#"{"must":1,"may":2}"#), Some(Strict { must: Some(1), may: Some(2) }));
        assert_eq!(ok("{}"), None, "a required property left out");
        assert_eq!(ok(r#"{"must":1,"may":null}"#), None, "null for a property that is not nullable");
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Patch {
        #[serde(default, skip_serializing_if = "Nullable::is_absent")]
        name: Nullable<String>,
    }

    #[test]
    fn absent_null_and_a_value_stay_apart_both_ways() {
        for (json, name) in [
            ("{}", Nullable::Absent),
            (r#"{"name":null}"#, Nullable::Null),
            (r#"{"name":"a"}"#, Nullable::Value("a".to_owned())),
        ] {
            let p = Patch { name };
            assert_eq!(serde_json::to_string(&p).unwrap(), json);
            assert_eq!(serde_json::from_str::<Patch>(json).unwrap(), p);
        }
    }

    #[test]
    fn only_a_value_has_one() {
        assert_eq!((Nullable::Value(1).value(), Nullable::Value(1).into_value()), (Some(&1), Some(1)));
        assert_eq!((Nullable::<i32>::Null.value(), Nullable::<i32>::Absent.into_value()), (None, None));
    }
}
