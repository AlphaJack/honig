//! Formats whose library type reads more than the format allows. `jiff` reads `2024-06-19 08:30Z` as a `date-time`.
//! `uuid` reads `{...}` and `url` reads a space. Generated code decodes such a value through [`read`]. The text is
//! validated as [`check_format`](crate::validate::check_format) validates a string, then the library reads it. A
//! server decoding a request and a client reading a response decode alike.
//!
//! [`Holds`] says where the text sits in the type: the value itself ([`Text`]) or the items of a list, the values of a
//! map and so on. The types stay those of the library.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Unexpected, Visitor};

use crate::nullable::Nullable;

/// A string format that a library type holds.
pub trait Format {
    /// Its name in `format`.
    const NAME: &'static str;

    /// Returns the text that the library reads. It is `text` unless the library would refuse a valid one.
    fn text(text: &str) -> Cow<'_, str> {
        Cow::Borrowed(text)
    }
}

/// The digits of a fraction of a second that `jiff` reads: nanoseconds. RFC 3339 allows more.
const NANO_DIGITS: usize = 9;

/// Returns `text` with the fraction of its seconds cut to nanoseconds. `jiff` refuses more digits. `chrono` drops them.
fn nanoseconds(text: &str) -> Cow<'_, str> {
    let Some(dot) = text.find('.') else { return Cow::Borrowed(text) };
    let digits = text.get(dot + 1..).map_or(0, |f| f.bytes().take_while(u8::is_ascii_digit).count());
    if digits <= NANO_DIGITS {
        return Cow::Borrowed(text);
    }
    let keep = dot + 1 + NANO_DIGITS;
    let cut = dot + 1 + digits;
    Cow::Owned([text.get(..keep).unwrap_or(text), text.get(cut..).unwrap_or("")].concat())
}

/// The formats that a library type holds. Each names its `format`.
pub mod format {
    use std::borrow::Cow;

    use super::{Format, nanoseconds};

    macro_rules! marker {
        ($(#[$doc:meta])* $name:ident, $format:literal) => {
            $(#[$doc])*
            #[derive(Debug, Clone, Copy)]
            pub struct $name;

            impl Format for $name {
                const NAME: &'static str = $format;
            }
        };
        ($(#[$doc:meta])* $name:ident, $format:literal, nanoseconds) => {
            $(#[$doc])*
            #[derive(Debug, Clone, Copy)]
            pub struct $name;

            impl Format for $name {
                const NAME: &'static str = $format;

                fn text(text: &str) -> Cow<'_, str> {
                    nanoseconds(text)
                }
            }
        };
    }

    marker!(
        /// `date-time`: RFC 3339 with an offset.
        DateTime,
        "date-time",
        nanoseconds
    );
    marker!(
        /// `date-time-local`: a date and a time without an offset.
        DateTimeLocal,
        "date-time-local",
        nanoseconds
    );
    marker!(
        /// `time-local`: a time without an offset.
        TimeLocal,
        "time-local",
        nanoseconds
    );
    marker!(
        /// `date`: RFC 3339 `full-date`.
        Date,
        "date"
    );
    marker!(
        /// `uuid`: the hyphenated form of RFC 9562.
        Uuid,
        "uuid"
    );
    marker!(
        /// `uri`: an absolute RFC 3986 URI.
        Uri,
        "uri"
    );
    marker!(
        /// `ipv4-cidr`: an IPv4 address block.
        Ipv4Cidr,
        "ipv4-cidr"
    );
    marker!(
        /// `ipv6-cidr`: an IPv6 address block.
        Ipv6Cidr,
        "ipv6-cidr"
    );
}

/// Where the formatted text sits in `T`, and how to read `T` with it checked.
pub trait Holds<'de, T> {
    /// Reads a `T`. Its text is checked first.
    ///
    /// # Errors
    /// The text is not of its format, or `T` refuses the value.
    fn read<D: Deserializer<'de>>(d: D) -> Result<T, D::Error>;
}

/// Reads `T` checked as `S` says. It is the `deserialize_with` of generated fields.
///
/// # Errors
/// The text is not of its format, or `T` refuses the value.
pub fn read<'de, D: Deserializer<'de>, T, S: Holds<'de, T>>(d: D) -> Result<T, D::Error> {
    S::read(d)
}

/// `T` read through `H`: a branch of a union or a body that is a library type itself. Decode one and take the value
/// with [`Validated::into_inner`].
#[derive(Debug)]
pub struct Validated<T, H>(T, PhantomData<H>);

impl<T, H> Validated<T, H> {
    /// Returns the value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<'de, T, H: Holds<'de, T>> Deserialize<'de> for Validated<T, H> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        H::read(d).map(|v| Self(v, PhantomData))
    }
}

/// A value read as it is: a position of a tuple without a checked format.
#[derive(Debug, Clone, Copy)]
pub struct Plain;

impl<'de, T: Deserialize<'de>> Holds<'de, T> for Plain {
    fn read<D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        T::deserialize(d)
    }
}

/// The text of format `F` itself.
#[derive(Debug, Clone, Copy)]
pub struct Text<F>(PhantomData<F>);

impl<'de, T: Deserialize<'de>, F: Format> Holds<'de, T> for Text<F> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<T, D::Error> {
        d.deserialize_str(TextVisitor::<T, F>(PhantomData))
    }
}

/// Checks a string, then hands it to `T`.
struct TextVisitor<T, F>(PhantomData<(T, F)>);

impl<'de, T: Deserialize<'de>, F: Format> Visitor<'de> for TextVisitor<T, F> {
    type Value = T;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a valid {}", F::NAME)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<T, E> {
        if !crate::validate::is_format(v, F::NAME) {
            return Err(E::invalid_value(Unexpected::Str(v), &self));
        }
        T::deserialize(de::value::StrDeserializer::<E>::new(&F::text(v)))
    }
}

/// `S` as a [`DeserializeSeed`]: an item or a value of a container.
struct Seed<S, T>(PhantomData<(S, T)>);

impl<'de, S: Holds<'de, T>, T> DeserializeSeed<'de> for Seed<S, T> {
    type Value = T;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
        S::read(d)
    }
}

/// `S` or `null`: an `Option`.
#[derive(Debug, Clone, Copy)]
pub struct Maybe<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, Option<T>> for Maybe<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
        d.deserialize_option(OptionVisitor::<S, T>(PhantomData))
    }
}

/// `None` for `null`, `S` otherwise.
struct OptionVisitor<S, T>(PhantomData<(S, T)>);

impl<'de, S: Holds<'de, T>, T> Visitor<'de> for OptionVisitor<S, T> {
    type Value = Option<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a value or null")
    }

    fn visit_none<E: de::Error>(self) -> Result<Option<T>, E> {
        Ok(None)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Option<T>, E> {
        Ok(None)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Option<T>, D::Error> {
        Present::<S>::read(d)
    }
}

/// `S` that may not be `null`, in an `Option` that is `None` when the property is absent.
#[derive(Debug, Clone, Copy)]
pub struct Present<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, Option<T>> for Present<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<Option<T>, D::Error> {
        S::read(d).map(Some)
    }
}

/// `S` or `null` in a [`Nullable`]. Absent is its default.
#[derive(Debug, Clone, Copy)]
pub struct OrNull<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, Nullable<T>> for OrNull<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<Nullable<T>, D::Error> {
        Maybe::<S>::read(d).map(|v| v.map_or(Nullable::Null, Nullable::Value))
    }
}

/// `S` held by reference.
#[derive(Debug, Clone, Copy)]
pub struct Boxed<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, Box<T>> for Boxed<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<Box<T>, D::Error> {
        S::read(d).map(Box::new)
    }
}

/// A list of `S`.
#[derive(Debug, Clone, Copy)]
pub struct Many<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, Vec<T>> for Many<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<Vec<T>, D::Error> {
        d.deserialize_seq(SeqVisitor::<S, T>(PhantomData))
    }
}

/// Reads every item with `S`.
struct SeqVisitor<S, T>(PhantomData<(S, T)>);

impl<'de, S: Holds<'de, T>, T> Visitor<'de> for SeqVisitor<S, T> {
    type Value = Vec<T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a list")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(MAX_PREALLOCATED));
        while let Some(item) = seq.next_element_seed(Seed::<S, T>(PhantomData))? {
            items.push(item);
        }
        Ok(items)
    }
}

/// The most items a list reserves room for before it reads them. A size hint comes from the body.
const MAX_PREALLOCATED: usize = 4096;

/// A list of exactly `N` of `S`.
#[derive(Debug, Clone, Copy)]
pub struct Exactly<S, const N: usize>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T, const N: usize> Holds<'de, [T; N]> for Exactly<S, N> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<[T; N], D::Error> {
        let items = Many::<S>::read(d)?;
        items.try_into().map_err(|unfit: Vec<T>| de::Error::invalid_length(unfit.len(), &ExpectedLen(N)))
    }
}

/// "`N` items" in an error.
struct ExpectedLen(usize);

impl de::Expected for ExpectedLen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} items", self.0)
    }
}

/// A map from keys to `S`, in key order.
#[derive(Debug, Clone, Copy)]
pub struct Values<S>(PhantomData<S>);

impl<'de, S: Holds<'de, T>, T> Holds<'de, BTreeMap<String, T>> for Values<S> {
    fn read<D: Deserializer<'de>>(d: D) -> Result<BTreeMap<String, T>, D::Error> {
        d.deserialize_map(MapVisitor::<S, T>(PhantomData))
    }
}

/// Reads every value with `S`.
struct MapVisitor<S, T>(PhantomData<(S, T)>);

impl<'de, S: Holds<'de, T>, T> Visitor<'de> for MapVisitor<S, T> {
    type Value = BTreeMap<String, T>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<BTreeMap<String, T>, A::Error> {
        let mut out = BTreeMap::new();
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(Seed::<S, T>(PhantomData))?;
            out.insert(key, value);
        }
        Ok(out)
    }
}

/// Tuples whose positions each have a shape: `(Text<format::Date>, Plain)` reads a `(jiff::civil::Date, u8)`.
macro_rules! tuple {
    ($($s:ident $t:ident),+) => {
        impl<'de, $($s: Holds<'de, $t>, $t),+> Holds<'de, ($($t,)+)> for ($($s,)+) {
            fn read<D: Deserializer<'de>>(d: D) -> Result<($($t,)+), D::Error> {
                let len = [$(stringify!($t)),+].len();
                d.deserialize_tuple(len, TupleVisitor::<($($s,)+), ($($t,)+)>(PhantomData))
            }
        }

        impl<'de, $($s: Holds<'de, $t>, $t),+> Visitor<'de> for TupleVisitor<($($s,)+), ($($t,)+)> {
            type Value = ($($t,)+);

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{} items", [$(stringify!($t)),+].len())
            }

            #[expect(non_snake_case, reason = "each item is named by its type parameter")]
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<($($t,)+), A::Error> {
                let mut at = 0;
                $(
                    let $t = seq
                        .next_element_seed(Seed::<$s, $t>(PhantomData))?
                        .ok_or_else(|| de::Error::invalid_length(at, &self))?;
                    at += 1;
                )+
                let _ = at;
                Ok(($($t,)+))
            }
        }
    };
}

/// Reads the positions of a tuple.
struct TupleVisitor<S, T>(PhantomData<(S, T)>);

tuple!(S0 T0);
tuple!(S0 T0, S1 T1);
tuple!(S0 T0, S1 T1, S2 T2);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6, S7 T7);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6, S7 T7, S8 T8);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6, S7 T7, S8 T8, S9 T9);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6, S7 T7, S8 T8, S9 T9, S10 T10);
tuple!(S0 T0, S1 T1, S2 T2, S3 T3, S4 T4, S5 T5, S6 T6, S7 T7, S8 T8, S9 T9, S10 T10, S11 T11);

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::{Exactly, Many, Maybe, Plain, Text, Values, format, nanoseconds};

    /// A library type that reads any text: what the check lets through reaches it.
    #[derive(Debug, PartialEq, serde::Deserialize)]
    #[serde(transparent)]
    struct Lenient(String);

    /// Reads `json` as a request or a response is read.
    fn read<'de, T, S: super::Holds<'de, T>>(json: &'de str) -> Result<T, serde_json::Error> {
        super::read::<_, T, S>(&mut serde_json::Deserializer::from_str(json))
    }

    #[test]
    fn text_that_the_library_reads_but_the_format_refuses_is_refused() {
        let json = r#""2024-06-19 08:30:06Z""#;
        let read = super::read::<_, Lenient, Text<format::DateTime>>(&mut serde_json::Deserializer::from_str(json));
        assert!(read.is_err(), "a space for `T` is not RFC 3339");
    }

    #[test]
    fn the_text_is_checked_before_the_library_reads_it() {
        assert_eq!(
            read::<Lenient, Text<format::DateTime>>(r#""2024-06-19T08:30:06Z""#).unwrap(),
            Lenient("2024-06-19T08:30:06Z".to_owned())
        );
        for wrong in [r#""2024-06-19 08:30:06Z""#, r#""2024-06-19T08:30Z""#, r#""20240619T083006Z""#] {
            let e = read::<Lenient, Text<format::DateTime>>(wrong).unwrap_err().to_string();
            assert!(e.contains("a valid date-time"), "{wrong}: {e}");
        }
        assert!(read::<Lenient, Text<format::Uuid>>(r#""{01234567-89ab-4def-8123-456789abcdef}""#).is_err());
        assert!(read::<Lenient, Text<format::Uri>>(r#""https://example.com/a b""#).is_err());
    }

    #[test]
    fn containers_check_each_text() {
        let ok = r#"["2024-06-19", "2024-06-20"]"#;
        assert_eq!(read::<Vec<Lenient>, Many<Text<format::Date>>>(ok).unwrap().len(), 2);
        assert!(read::<Vec<Lenient>, Many<Text<format::Date>>>(r#"["2024-06-19", "20240620"]"#).is_err());
        assert_eq!(read::<Option<Lenient>, Maybe<Text<format::Date>>>("null").unwrap(), None);
        assert!(read::<Option<Lenient>, Maybe<Text<format::Date>>>(r#""2024-6-9""#).is_err());
        assert!(
            read::<std::collections::BTreeMap<String, Lenient>, Values<Text<format::Date>>>(r#"{"a": "x"}"#).is_err()
        );
        assert!(read::<[Lenient; 2], Exactly<Text<format::Date>, 2>>(r#"["2024-06-19"]"#).is_err());
        let pair = read::<(Lenient, u8), (Text<format::Date>, Plain)>(r#"["2024-06-19", 3]"#).unwrap();
        assert_eq!(pair, (Lenient("2024-06-19".to_owned()), 3));
        assert!(read::<(Lenient, u8), (Text<format::Date>, Plain)>(r#"["2024 06 19", 3]"#).is_err());
    }

    #[test]
    fn validated_reads_through_its_shape() {
        type Day = super::Validated<Lenient, Text<format::Date>>;
        assert_eq!(
            serde_json::from_str::<Day>(r#""2024-06-19""#).unwrap().into_inner(),
            Lenient("2024-06-19".to_owned())
        );
        assert!(serde_json::from_str::<Day>(r#""2024-6-19""#).is_err());
    }

    #[test]
    fn a_fraction_is_cut_to_nanoseconds() {
        assert_eq!(nanoseconds("08:30:06.1234567891Z"), Cow::<str>::Owned("08:30:06.123456789Z".to_owned()));
        assert_eq!(nanoseconds("2024-06-19T08:30:06.5+01:00"), "2024-06-19T08:30:06.5+01:00");
        assert_eq!(nanoseconds("08:30:06"), "08:30:06");
    }
}
