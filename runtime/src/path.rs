//! Where a value failed to deserialize: the JSON pointer (`/attachments/0/url`) of the innermost value whose
//! decoding failed. A server answers which field is wrong; serde's own errors only say what.

use std::cell::RefCell;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, Visitor};

/// Deserializes `T` from `de`. On failure it also gives the pointer of the failing value (`""` for the root).
pub(crate) fn deserialize<'de, D, T>(de: D) -> Result<T, (String, D::Error)>
where
    D: Deserializer<'de>,
    T: de::Deserialize<'de>,
{
    NESTED.with_borrow_mut(|n| *n = None);
    let failed = RefCell::new(None);
    let tracked = Tracked { de, at: &Chain::Root, failed: &failed };
    T::deserialize(tracked).map_err(|e| (failed.into_inner().unwrap_or_default(), e))
}

/// The path to the value being read with the innermost last.
enum Chain<'a> {
    Root,
    Key(&'a Self, String),
    Index(&'a Self, usize),
}

impl Chain<'_> {
    /// Returns the key escaped as RFC 6901 says: `~` and `/` become `~0` and `~1`.
    fn pointer(&self) -> String {
        match self {
            Self::Root => String::new(),
            Self::Key(parent, k) => format!("{}/{}", parent.pointer(), k.replace('~', "~0").replace('/', "~1")),
            Self::Index(parent, i) => format!("{}/{i}", parent.pointer()),
        }
    }
}

thread_local! {
    /// Where a value decoded from its own text ([`crate::tagged::read_once`]) failed, relative to that value. The tracker
    /// around the value adds it to the value's own pointer.
    static NESTED: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Records where a value decoded from its own text failed, relative to it: `/parcels/0/fragile`.
pub(crate) fn nested_failure(pointer: String) {
    NESTED.with_borrow_mut(|n| *n = Some(pointer));
}

/// Keeps the innermost failure. That is the first one recorded because errors propagate outwards. A value decoded from
/// its own text adds where inside it the decode failed.
fn record(failed: &RefCell<Option<String>>, at: &Chain<'_>) {
    let inside = NESTED.with_borrow_mut(Option::take).unwrap_or_default();
    let mut f = failed.borrow_mut();
    if f.is_none() {
        *f = Some(at.pointer() + &inside);
    }
}

struct Tracked<'a, 'b, D> {
    de: D,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
}

/// Forwards each `deserialize_*` with the visitor wrapped. It records the path when it fails.
macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, D::Error> {
            let (at, failed) = (self.at, self.failed);
            let wrapped = Wrap { inner: visitor, at, failed };
            self.de.$method($($arg,)* wrapped).inspect_err(|_| record(failed, at))
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Tracked<'_, '_, D> {
    type Error = D::Error;

    forward!(
        deserialize_any(),
        deserialize_bool(),
        deserialize_i8(),
        deserialize_i16(),
        deserialize_i32(),
        deserialize_i64(),
        deserialize_i128(),
        deserialize_u8(),
        deserialize_u16(),
        deserialize_u32(),
        deserialize_u64(),
        deserialize_u128(),
        deserialize_f32(),
        deserialize_f64(),
        deserialize_char(),
        deserialize_str(),
        deserialize_string(),
        deserialize_bytes(),
        deserialize_byte_buf(),
        deserialize_option(),
        deserialize_unit(),
        deserialize_unit_struct(name: &'static str),
        deserialize_newtype_struct(name: &'static str),
        deserialize_seq(),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map(),
        deserialize_struct(name: &'static str, fields: &'static [&'static str]),
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier(),
        deserialize_ignored_any(),
    );

    fn is_human_readable(&self) -> bool {
        self.de.is_human_readable()
    }
}

/// A visitor whose maps, sequences and nested values keep tracking.
/// `Visitor::expecting` of a visitor that wraps `inner`. That is what the wrapped visitor expects.
macro_rules! expecting {
    () => {
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.inner.expecting(f)
        }
    };
}

struct Wrap<'a, 'b, V> {
    inner: V,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
}

/// Forwards each scalar `visit_*` unchanged.
macro_rules! visit {
    ($($method:ident($ty:ty)),* $(,)?) => {$(
        fn $method<E: de::Error>(self, v: $ty) -> Result<Self::Value, E> {
            self.inner.$method(v)
        }
    )*};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Wrap<'_, '_, V> {
    type Value = V::Value;

    expecting!();

    visit!(
        visit_bool(bool),
        visit_i8(i8),
        visit_i16(i16),
        visit_i32(i32),
        visit_i64(i64),
        visit_i128(i128),
        visit_u8(u8),
        visit_u16(u16),
        visit_u32(u32),
        visit_u64(u64),
        visit_u128(u128),
        visit_f32(f32),
        visit_f64(f64),
        visit_char(char),
        visit_str(&str),
        visit_borrowed_str(&'de str),
        visit_string(String),
        visit_bytes(&[u8]),
        visit_borrowed_bytes(&'de [u8]),
        visit_byte_buf(Vec<u8>),
    );

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_some(Tracked { de, at: self.at, failed: self.failed })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, de: D) -> Result<Self::Value, D::Error> {
        self.inner.visit_newtype_struct(Tracked { de, at: self.at, failed: self.failed })
    }

    fn visit_seq<A: de::SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_seq(Seq { inner: seq, at: self.at, failed: self.failed, index: 0 })
    }

    fn visit_map<A: de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_map(Map { inner: map, at: self.at, failed: self.failed, key: None })
    }

    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        self.inner.visit_enum(Enum { inner: data, at: self.at, failed: self.failed })
    }
}

/// A seed that deserializes its value tracked at `at`.
struct Seed<'a, 'b, S> {
    inner: S,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Seed<'_, '_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<S::Value, D::Error> {
        self.inner.deserialize(Tracked { de, at: self.at, failed: self.failed })
    }
}

struct Seq<'a, 'b, A> {
    inner: A,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
    index: usize,
}

impl<'de, A: de::SeqAccess<'de>> de::SeqAccess<'de> for Seq<'_, '_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, A::Error> {
        let at = Chain::Index(self.at, self.index);
        self.index += 1;
        let failed = self.failed;
        self.inner.next_element_seed(Seed { inner: seed, at: &at, failed }).inspect_err(|_| record(failed, &at))
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

struct Map<'a, 'b, A> {
    inner: A,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
    /// The key just read when it is text.
    key: Option<String>,
}

impl<'de, A: de::MapAccess<'de>> de::MapAccess<'de> for Map<'_, '_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, A::Error> {
        let mut key = None;
        let found = self.inner.next_key_seed(KeySeed { inner: seed, key: &mut key });
        self.key = key;
        found.inspect_err(|_| record(self.failed, self.at))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        let at = Chain::Key(self.at, self.key.take().unwrap_or_default());
        let failed = self.failed;
        self.inner.next_value_seed(Seed { inner: seed, at: &at, failed }).inspect_err(|_| record(failed, &at))
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

/// Reads a map key and keeps its text for the path.
struct KeySeed<'k, S> {
    inner: S,
    key: &'k mut Option<String>,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for KeySeed<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, de: D) -> Result<S::Value, D::Error> {
        self.inner.deserialize(KeyDeserializer { de, key: self.key })
    }
}

/// A map key's deserializer: string keys are noted on their way to the visitor.
struct KeyDeserializer<'k, D> {
    de: D,
    key: &'k mut Option<String>,
}

macro_rules! key_forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, D::Error> {
            self.de.$method($($arg,)* KeyVisitor { inner: visitor, key: self.key })
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for KeyDeserializer<'_, D> {
    type Error = D::Error;

    key_forward!(
        deserialize_any(),
        deserialize_bool(),
        deserialize_i8(),
        deserialize_i16(),
        deserialize_i32(),
        deserialize_i64(),
        deserialize_i128(),
        deserialize_u8(),
        deserialize_u16(),
        deserialize_u32(),
        deserialize_u64(),
        deserialize_u128(),
        deserialize_f32(),
        deserialize_f64(),
        deserialize_char(),
        deserialize_str(),
        deserialize_string(),
        deserialize_bytes(),
        deserialize_byte_buf(),
        deserialize_option(),
        deserialize_unit(),
        deserialize_unit_struct(name: &'static str),
        deserialize_newtype_struct(name: &'static str),
        deserialize_seq(),
        deserialize_tuple(len: usize),
        deserialize_tuple_struct(name: &'static str, len: usize),
        deserialize_map(),
        deserialize_struct(name: &'static str, fields: &'static [&'static str]),
        deserialize_enum(name: &'static str, variants: &'static [&'static str]),
        deserialize_identifier(),
        deserialize_ignored_any(),
    );

    fn is_human_readable(&self) -> bool {
        self.de.is_human_readable()
    }
}

struct KeyVisitor<'k, V> {
    inner: V,
    key: &'k mut Option<String>,
}

/// Forwards each `visit_*` unchanged; the text ones note the key first.
macro_rules! key_visit {
    (text: $($tmethod:ident($tty:ty)),*; other: $($method:ident($ty:ty)),* $(,)?) => {
        $(
            fn $tmethod<E: de::Error>(self, v: $tty) -> Result<Self::Value, E> {
                *self.key = Some(v.to_string());
                self.inner.$tmethod(v)
            }
        )*
        $(
            fn $method<E: de::Error>(self, v: $ty) -> Result<Self::Value, E> {
                self.inner.$method(v)
            }
        )*
    };
}

impl<'de, V: Visitor<'de>> Visitor<'de> for KeyVisitor<'_, V> {
    type Value = V::Value;

    expecting!();

    key_visit!(
        text: visit_str(&str), visit_borrowed_str(&'de str), visit_string(String), visit_u64(u64), visit_i64(i64);
        other: visit_bool(bool), visit_u8(u8), visit_u16(u16), visit_u32(u32), visit_i8(i8), visit_i16(i16),
            visit_i32(i32), visit_f64(f64), visit_bytes(&[u8]), visit_byte_buf(Vec<u8>),
    );

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.inner.visit_unit()
    }
}

struct Enum<'a, 'b, A> {
    inner: A,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
}

impl<'a, 'b, 'de, A: de::EnumAccess<'de>> de::EnumAccess<'de> for Enum<'a, 'b, A> {
    type Error = A::Error;
    type Variant = Variant<'a, 'b, A::Variant>;

    fn variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<(S::Value, Self::Variant), A::Error> {
        let (at, failed) = (self.at, self.failed);
        let (value, variant) = self.inner.variant_seed(seed).inspect_err(|_| record(failed, at))?;
        Ok((value, Variant { inner: variant, at, failed }))
    }
}

struct Variant<'a, 'b, A> {
    inner: A,
    at: &'a Chain<'a>,
    failed: &'b RefCell<Option<String>>,
}

impl<'de, A: de::VariantAccess<'de>> de::VariantAccess<'de> for Variant<'_, '_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.inner.unit_variant()
    }

    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, seed: S) -> Result<S::Value, A::Error> {
        self.inner.newtype_variant_seed(Seed { inner: seed, at: self.at, failed: self.failed })
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.inner.tuple_variant(len, Wrap { inner: visitor, at: self.at, failed: self.failed })
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.inner.struct_variant(fields, Wrap { inner: visitor, at: self.at, failed: self.failed })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "only decoding is tested")]
    struct Payload {
        body: Option<String>,
        attachments: Option<Vec<Attachment>>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[expect(dead_code, reason = "only decoding is tested")]
    struct Attachment {
        url: String,
    }

    fn pointer(json: &str) -> String {
        let mut de = serde_json::Deserializer::from_str(json);
        deserialize::<_, Payload>(&mut de).err().map_or_else(|| "ok".to_owned(), |(p, _)| p)
    }

    #[test]
    fn points_at_the_failing_value() {
        assert_eq!(pointer(r#"{"body": "hi"}"#), "ok");
        assert_eq!(pointer(r#"{"body": 5}"#), "/body");
        assert_eq!(pointer(r#"{"attachments": [{"url": "a"}, {"url": 3}]}"#), "/attachments/1/url");
        assert_eq!(pointer(r#"{"attachments": [{}]}"#), "/attachments/0");
        // A struct read from a sequence: its first field is element 0.
        assert_eq!(pointer("[1]"), "/0");
        assert_eq!(pointer("5"), "");
    }
}
