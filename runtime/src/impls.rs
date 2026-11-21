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
