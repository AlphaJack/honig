//! The trait impls that every generated string or integer enum, and every checked newtype of one base, shares.
//! Generated code calls them as `runtime::str_enum_impls!(Name)`: the module contains no copy of them.

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! str_enum_impls {
    ($n:ident) => {
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
        impl std::str::FromStr for $n {
            type Err = $crate::BuildError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::try_from(s)
            }
        }
        impl ::serde::Serialize for $n {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <std::borrow::Cow<'de, str> as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(&*s).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! int_enum_impls {
    ($n:ident) => {
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.value())
            }
        }
        impl ::serde::Serialize for $n {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_i64(self.value())
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = <i64 as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(v).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_str_impls {
    ($n:ident) => {
        impl $n {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }
        impl TryFrom<&str> for $n {
            type Error = $crate::BuildError;
            fn try_from(value: &str) -> Result<Self, $crate::BuildError> {
                Self::try_from(value.to_owned())
            }
        }
        impl std::str::FromStr for $n {
            type Err = $crate::BuildError;
            fn from_str(s: &str) -> Result<Self, $crate::BuildError> {
                Self::try_from(s)
            }
        }
        impl std::ops::Deref for $n {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }
        impl AsRef<str> for $n {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl PartialEq<str> for $n {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }
        impl PartialEq<&str> for $n {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }
        impl From<$n> for String {
            fn from(value: $n) -> Self {
                value.0
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(s).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_num_impls {
    ($n:ident, $t:ty) => {
        impl $n {
            #[must_use]
            pub const fn get(self) -> $t {
                self.0
            }
        }
        impl std::str::FromStr for $n {
            type Err = $crate::BuildError;
            fn from_str(s: &str) -> Result<Self, $crate::BuildError> {
                let v: $t = s.parse().map_err(|e| $crate::BuildError::value(format!("{e}")))?;
                Self::try_from(v)
            }
        }
        impl std::ops::Deref for $n {
            type Target = $t;
            fn deref(&self) -> &$t {
                &self.0
            }
        }
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl PartialEq<$t> for $n {
            fn eq(&self, other: &$t) -> bool {
                self.0 == *other
            }
        }
        impl PartialOrd<$t> for $n {
            fn partial_cmp(&self, other: &$t) -> Option<std::cmp::Ordering> {
                self.0.partial_cmp(other)
            }
        }
        impl From<$n> for $t {
            fn from(value: $n) -> Self {
                value.0
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = <$t as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(v).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_impls {
    ($n:ident, $t:ty) => {
        impl $n {
            #[must_use]
            pub fn into_inner(self) -> $t {
                self.0
            }
        }
        impl std::ops::Deref for $n {
            type Target = $t;
            fn deref(&self) -> &$t {
                &self.0
            }
        }
        impl AsRef<$t> for $n {
            fn as_ref(&self) -> &$t {
                &self.0
            }
        }
        impl From<$n> for $t {
            fn from(value: $n) -> Self {
                value.0
            }
        }
    };
}

/// The impls that every generated type of one kind shares, written once here.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_decimal_impls {
    ($n:ident) => {
        impl $n {
            #[must_use]
            pub fn into_inner(self) -> $crate::Decimal {
                self.0
            }
        }
        impl std::str::FromStr for $n {
            type Err = $crate::BuildError;
            fn from_str(s: &str) -> Result<Self, $crate::BuildError> {
                let v: $crate::Decimal = s.parse().map_err(|e| $crate::BuildError::value(format!("{e}")))?;
                Self::try_from(v)
            }
        }
        impl std::ops::Deref for $n {
            type Target = $crate::Decimal;
            fn deref(&self) -> &$crate::Decimal {
                &self.0
            }
        }
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                std::fmt::Display::fmt(&self.0, f)
            }
        }
        impl From<$n> for $crate::Decimal {
            fn from(value: $n) -> Self {
                value.0
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = <$crate::Decimal as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(v).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// The impls that every constant shares: it serializes to its one value, `$n::value()` or `$n::VALUE`, and decodes
/// only it.
#[doc(hidden)]
#[macro_export]
macro_rules! const_impls {
    ($n:ident) => {
        impl ::serde::Serialize for $n {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                ::serde::Serialize::serialize(Self::value(), s)
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $crate::__private::expect_const(d, Self::value()).map(|()| Self)
            }
        }
    };
    ($n:ident, VALUE) => {
        impl ::serde::Serialize for $n {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                ::serde::Serialize::serialize(&Self::VALUE, s)
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $crate::__private::expect_const(d, &::serde_json::Value::from(Self::VALUE)).map(|()| Self)
            }
        }
    };
}

/// `Default` of a type whose `new()` takes no argument: that value.
#[doc(hidden)]
#[macro_export]
macro_rules! default_from_new {
    ($n:ident) => {
        impl Default for $n {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

/// `Display` and `Error` of an operation's response: each variant written as its text, so that `?` takes it into any
/// error type.
#[doc(hidden)]
#[macro_export]
macro_rules! response_error {
    ($n:ident { $($pat:pat => $text:expr,)* }) => {
        impl std::fmt::Display for $n {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    $($pat => std::fmt::Display::fmt(&$text, f),)*
                }
            }
        }
        impl std::error::Error for $n {}
    };
}

/// The impls of a value sent as JSON text inside a string: it writes its JSON and parses it back.
#[doc(hidden)]
#[macro_export]
macro_rules! json_text_impls {
    ($n:ident) => {
        impl ::serde::Serialize for $n {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let text = ::serde_json::to_string(&self.0).map_err(::serde::ser::Error::custom)?;
                s.serialize_str(&text)
            }
        }
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let text = <String as ::serde::Deserialize>::deserialize(d)?;
                $crate::__private::json_text(&text).map(Self).map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// `Deserialize` of a union told apart by a property: [`TaggedUnion`](crate::__private::TaggedUnion) decodes it.
#[doc(hidden)]
#[macro_export]
macro_rules! tagged_deserialize {
    ($n:ident) => {
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                $crate::__private::tagged::<D, Self>(d)
            }
        }
    };
}

/// `Deserialize` of a checked type of `$base`: the value decodes as its base, then its checks run. With `read $h`, the
/// base decodes the text of its library formats checked through `$h`. With `once`, JSON text is parsed once: the
/// value decodes from it and the checks use it.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_deserialize {
    ($n:ident, $base:ty) => {
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let value = <$base as ::serde::Deserialize>::deserialize(d)?;
                Self::try_from(value).map_err(::serde::de::Error::custom)
            }
        }
    };
    ($n:ident, $base:ty, read $h:ty) => {
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let value = $crate::__private::formats::read::<_, $base, $h>(d)?;
                Self::try_from(value).map_err(::serde::de::Error::custom)
            }
        }
    };
    ($n:ident, $base:ty, once) => {
        impl<'de> ::serde::Deserialize<'de> for $n {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let (value, text) = $crate::__private::read_once::<_, $base>(d)?;
                match text {
                    Some(text) => {
                        let json = $crate::__private::Content::parse(text).map_err(::serde::de::Error::custom)?;
                        Self::checked(value, &json)
                    }
                    None => Self::try_from(value),
                }
                .map_err(::serde::de::Error::custom)
            }
        }
    };
}

/// `TryFrom<$base>` of a checked type whose checks use the value as JSON: the value written out once, then
/// `$n::checked` on it.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_json {
    ($n:ident, $base:ty) => {
        impl TryFrom<$base> for $n {
            type Error = $crate::BuildError;
            fn try_from(value: $base) -> Result<Self, $crate::BuildError> {
                let json = $crate::__private::to_value(&value).map_err(|e| $crate::BuildError::value(e.to_string()))?;
                Self::checked(value, &$crate::__private::Content::of(&json))
            }
        }
    };
}

/// The checks of a checked type that has the base and the keywords of `$first`, an earlier one: `$first` runs them.
/// With `json`, its `checked` as well.
#[doc(hidden)]
#[macro_export]
macro_rules! checked_twin {
    ($n:ident, $first:ident, $base:ty) => {
        impl TryFrom<$base> for $n {
            type Error = $crate::BuildError;
            fn try_from(value: $base) -> Result<Self, $crate::BuildError> {
                $first::try_from(value).map(|v| Self(v.0))
            }
        }
    };
    ($n:ident, $first:ident, $base:ty, json) => {
        impl $n {
            fn checked(value: $base, json: &$crate::__private::Content<'_>) -> Result<Self, $crate::BuildError> {
                $first::checked(value, json).map(|v| Self(v.0))
            }
        }
        $crate::checked_json!($n, $base);
    };
}
