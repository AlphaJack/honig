//! Deserializing tagged unions from a buffered value.

use std::borrow::Cow;
use std::fmt;

use serde::Deserialize;
use serde::de::{self, DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

/// Items reserved up front from a size hint. A hint is a claim of the input and not a promise worth memory.
const PREALLOCATE: usize = 64;

/// A buffered JSON-shaped value whose strings borrow from the input when they can.
#[derive(Debug, Clone, PartialEq)]
pub enum Content<'de> {
    /// `null`.
    Null,
    /// A boolean.
    Bool(bool),
    /// A non-negative integer.
    U64(u64),
    /// A negative integer.
    I64(i64),
    /// Any other number.
    F64(f64),
    /// A string.
    Str(Cow<'de, str>),
    /// An array.
    Seq(Vec<Self>),
    /// An object with its members in input order.
    Map(Vec<(Cow<'de, str>, Self)>),
    /// A JSON value as written from [`Content::exact`]. Its numbers keep their digits.
    Raw(&'de str),
}

impl<'de> Content<'de> {
    /// Returns the member `key` of an object.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Map(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            Self::Null
            | Self::Bool(_)
            | Self::U64(_)
            | Self::I64(_)
            | Self::F64(_)
            | Self::Str(_)
            | Self::Seq(_)
            | Self::Raw(_) => None,
        }
    }

    /// Returns the string value of the object's member `property`: a tagged union's discriminator. From raw text it is
    /// read without buffering the object.
    #[must_use]
    pub fn tag(&self, property: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::Raw(text) => {
                TagSeed(property).deserialize(&mut serde_json::Deserializer::from_str(text)).ok().flatten()
            }
            other @ (Self::Null
            | Self::Bool(_)
            | Self::U64(_)
            | Self::I64(_)
            | Self::F64(_)
            | Self::Str(_)
            | Self::Seq(_)
            | Self::Map(_)) => other.get(property).and_then(Self::as_str).map(Cow::Borrowed),
        }
    }

    /// Returns the string if this is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        if let Self::Str(s) = self { Some(s) } else { None }
    }

    /// Returns the JSON kind for unions told apart by kind.
    #[must_use]
    pub fn kind(&self) -> Kind {
        match self {
            Self::Raw(text) => raw_kind(text),
            Self::Null => Kind::Null,
            Self::Bool(_) => Kind::Bool,
            Self::U64(_) | Self::I64(_) => Kind::Integer,
            Self::F64(_) => Kind::Number,
            Self::Str(_) => Kind::String,
            Self::Seq(_) => Kind::Array,
            Self::Map(_) => Kind::Object,
        }
    }

    /// Converts the value into a `serde_json::Value` (for `Other` variants).
    #[must_use]
    pub fn into_value(self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(b),
            Self::U64(n) => Value::from(n),
            Self::I64(n) => Value::from(n),
            Self::F64(n) => serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number),
            Self::Str(s) => Value::String(s.into_owned()),
            Self::Seq(a) => Value::Array(a.into_iter().map(Self::into_value).collect()),
            Self::Map(m) => Value::Object(m.into_iter().map(|(k, v)| (k.into_owned(), v.into_value())).collect()),
            Self::Raw(text) => serde_json::from_str(text).unwrap_or(Value::Null),
        }
    }

    /// Returns the value as written when `d` is `serde_json` that reads a slice or a string. Otherwise it is buffered.
    /// A union whose branches hold a `Decimal` decodes from it. So the decimal keeps its digits. Errors: `d` fails.
    pub fn exact<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_newtype_struct(crate::raw_value::RAW_VALUE, ExactVisitor)
    }

    /// Deserializes `T` from the buffered value.
    /// Errors: the value does not fit `T`.
    pub fn decode<T: Deserialize<'de>>(self) -> Result<T, serde_json::Error> {
        // Raw text is read in place; a buffer hands its strings over.
        if matches!(self, Self::Raw(_)) { self.decode_ref() } else { T::deserialize(self) }
    }

    /// Returns [`Content::decode`] wrapped by `wrap` (a variant such as `Self::Text`) with the error type that the
    /// caller returns. Errors: the value does not fit `T`.
    pub fn decode_into<T: Deserialize<'de>, U, E: de::Error>(self, wrap: impl FnOnce(T) -> U) -> Result<U, E> {
        self.decode().map(wrap).map_err(E::custom)
    }

    /// Returns [`Content::decode`] without consuming the value.
    /// Errors: the value does not fit `T`.
    pub fn decode_ref<T: Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        let Self::Raw(text) = self else { return T::deserialize(self) };
        // Integral floats are integers here as in a whole body.
        let mut d = serde_json::Deserializer::from_str(text);
        let v = T::deserialize(crate::integral::Ints(&mut d))?;
        d.end().map(|()| v)
    }
}

/// Whether an object can be a struct whose fields are `(name, present, kinds)`.
///
/// Each field that must be present is, and each one present has one of its kinds (any kind when empty). An untagged
/// union checks a branch with it before decoding the branch. A branch that cannot fit is skipped, and its nested values are not decoded only to fail at a
/// field after them. That retry at every level of a recursive union made decoding exponential in its depth. A value
/// that is not an object (raw text) is left to the decoder.
#[must_use]
pub fn fits(v: &Content<'_>, fields: &[(&str, bool, &[Kind])]) -> bool {
    let Content::Map(_) = v else { return true };
    fields.iter().all(|(name, present, kinds)| match v.get(name) {
        None => !present,
        Some(x) => kinds.is_empty() || kinds.contains(&x.kind()),
    })
}

/// The JSON kind of a buffered value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// `null`.
    Null,
    /// A boolean.
    Bool,
    /// An integer.
    Integer,
    /// A non-integer number.
    Number,
    /// A string.
    String,
    /// An array.
    Array,
    /// An object.
    Object,
}

struct ContentVisitor;

impl<'de> Visitor<'de> for ContentVisitor {
    type Value = Content<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Content<'de>, E> {
        Ok(Content::Bool(v))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Content<'de>, E> {
        Ok(Content::U64(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Content<'de>, E> {
        Ok(u64::try_from(v).map_or(Content::I64(v), Content::U64))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Content<'de>, E> {
        Ok(Content::F64(v))
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Content<'de>, E> {
        Ok(Content::Str(Cow::Borrowed(v)))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Content<'de>, E> {
        Ok(Content::Str(Cow::Owned(v.to_owned())))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Content<'de>, E> {
        Ok(Content::Str(Cow::Owned(v)))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Content<'de>, E> {
        Ok(Content::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Content<'de>, E> {
        Ok(Content::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Content<'de>, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Content<'de>, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Content<'de>, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(PREALLOCATE));
        while let Some(v) = seq.next_element()? {
            out.push(v);
        }
        Ok(Content::Seq(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Content<'de>, A::Error> {
        let mut out = Vec::with_capacity(map.size_hint().unwrap_or(0).min(PREALLOCATE));
        while let Some(Key(k)) = map.next_key()? {
            out.push((k, map.next_value()?));
        }
        Ok(Content::Map(out))
    }
}

/// Returns the JSON kind of `text`, a value as written. Its first character tells. For a number it also matters whether
/// it has a fraction or an exponent.
fn raw_kind(text: &str) -> Kind {
    let text = text.trim_start();
    match text.as_bytes().first() {
        Some(b'{') => Kind::Object,
        Some(b'[') => Kind::Array,
        Some(b'"') => Kind::String,
        Some(b't' | b'f') => Kind::Bool,
        Some(b'n') | None => Kind::Null,
        Some(_) if text.contains(['.', 'e', 'E']) => Kind::Number,
        Some(_) => Kind::Integer,
    }
}

/// [`Content::tag`] over raw text. It gives the string member that it names and skips every other member.
struct TagSeed<'p>(&'p str);

impl<'de> DeserializeSeed<'de> for TagSeed<'_> {
    type Value = Option<Cow<'de, str>>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for TagSeed<'_> {
    type Value = Option<Cow<'de, str>>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        let mut tag = None;
        // Every member is read with the tag too. `serde_json` refuses an object left half read.
        while let Some(Key(k)) = map.next_key()? {
            if tag.is_none() && k == self.0 {
                if let Content::Str(s) = map.next_value()? {
                    tag = Some(s);
                }
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(tag)
    }
}

/// [`Content::exact`]: `serde_json` asked for the raw text answers with a one-entry map that holds it. Any other
/// deserializer gives the value buffered as usual.
struct ExactVisitor;

impl<'de> Visitor<'de> for ExactVisitor {
    type Value = Content<'de>;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Content<'de>, A::Error> {
        let Some(Key(first)) = map.next_key()? else { return Ok(Content::Map(vec![])) };
        if first == crate::raw_value::RAW_VALUE {
            // Text `serde_json` had to copy (a reader) does not live long enough to borrow: buffered instead.
            let Key(text) = map.next_value()?;
            return match text {
                Cow::Borrowed(text) => Ok(Content::Raw(text)),
                Cow::Owned(text) => serde_json::from_str::<Value>(&text)
                    .map_err(de::Error::custom)?
                    .deserialize_any(ContentVisitor)
                    .map_err(de::Error::custom),
            };
        }
        let mut out = vec![(first, map.next_value()?)];
        while let Some(Key(k)) = map.next_key()? {
            out.push((k, map.next_value()?));
        }
        Ok(Content::Map(out))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Content<'de>, D::Error> {
        Content::deserialize(d)
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Content<'de>, E> {
        ContentVisitor.visit_bool(v)
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Content<'de>, E> {
        ContentVisitor.visit_u64(v)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Content<'de>, E> {
        ContentVisitor.visit_i64(v)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Content<'de>, E> {
        ContentVisitor.visit_f64(v)
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Content<'de>, E> {
        ContentVisitor.visit_borrowed_str(v)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Content<'de>, E> {
        ContentVisitor.visit_str(v)
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Content<'de>, E> {
        ContentVisitor.visit_string(v)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Content<'de>, E> {
        ContentVisitor.visit_unit()
    }

    fn visit_none<E: de::Error>(self) -> Result<Content<'de>, E> {
        ContentVisitor.visit_none()
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Content<'de>, D::Error> {
        ContentVisitor.visit_some(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Content<'de>, A::Error> {
        ContentVisitor.visit_seq(seq)
    }
}

/// A union told apart by a discriminator property. Generated for tagged
/// `oneOf`/`anyOf`; [`tagged`] decodes it.
pub trait TaggedUnion<'de>: Sized {
    /// The discriminator property.
    const TAG: &'static str;
    /// Is `tag` a value of a known variant?
    fn knows(tag: &str) -> bool;
    /// Decodes the variant for a known `tag` straight from `d`.
    /// Errors: the value does not fit the variant.
    fn from_tag<D: Deserializer<'de>>(tag: &str, d: D) -> Result<Self, D::Error>;
    /// Decodes from a buffered value (tag not first or unknown).
    /// Errors: the value fits no variant.
    fn from_content(v: Content<'de>) -> Result<Self, serde_json::Error>;
}

/// Decodes a tagged union. A leading discriminator streams straight into the
/// variant. Otherwise the object is buffered in [`Content`].
/// Errors: the value is not an object or fits no variant.
pub fn tagged<'de, D: Deserializer<'de>, U: TaggedUnion<'de>>(d: D) -> Result<U, D::Error> {
    d.deserialize_any(TagVisitor(std::marker::PhantomData))
}

struct TagVisitor<U>(std::marker::PhantomData<U>);

impl<'de, U: TaggedUnion<'de>> Visitor<'de> for TagVisitor<U> {
    type Value = U;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "an object with `{}`", U::TAG)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<U, A::Error> {
        let Some(key) = map.next_key::<Content<'de>>()? else {
            return U::from_content(Content::Map(vec![])).map_err(de::Error::custom);
        };
        let key = match key {
            Content::Str(k) => k,
            other @ (Content::Null
            | Content::Bool(_)
            | Content::U64(_)
            | Content::I64(_)
            | Content::F64(_)
            | Content::Seq(_)
            | Content::Map(_)
            | Content::Raw(_)) => Cow::Owned(other.into_value().to_string()),
        };
        let value: Content<'de> = map.next_value()?;
        if key == U::TAG
            && let Content::Str(tag) = &value
            && U::knows(tag)
        {
            let tag = tag.clone();
            return U::from_tag(
                &tag,
                de::value::MapAccessDeserializer::new(Replay { first: Some((key, tag.clone())), map }),
            );
        }
        let mut members = vec![(key, value)];
        while let Some((k, v)) = map.next_entry::<Content<'de>, Content<'de>>()? {
            let k = if let Content::Str(k) = k { k } else { Cow::Owned(k.into_value().to_string()) };
            members.push((k, v));
        }
        U::from_content(Content::Map(members)).map_err(de::Error::custom)
    }

    // Not an object: let the buffered path report it.
    fn visit_bool<E: de::Error>(self, v: bool) -> Result<U, E> {
        U::from_content(Content::Bool(v)).map_err(E::custom)
    }
    fn visit_i64<E: de::Error>(self, v: i64) -> Result<U, E> {
        U::from_content(Content::I64(v)).map_err(E::custom)
    }
    fn visit_u64<E: de::Error>(self, v: u64) -> Result<U, E> {
        U::from_content(Content::U64(v)).map_err(E::custom)
    }
    fn visit_f64<E: de::Error>(self, v: f64) -> Result<U, E> {
        U::from_content(Content::F64(v)).map_err(E::custom)
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<U, E> {
        U::from_content(Content::Str(Cow::Owned(v.to_owned()))).map_err(E::custom)
    }
    fn visit_unit<E: de::Error>(self) -> Result<U, E> {
        U::from_content(Content::Null).map_err(E::custom)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<U, A::Error> {
        let mut items = vec![];
        while let Some(x) = seq.next_element::<Content<'de>>()? {
            items.push(x);
        }
        U::from_content(Content::Seq(items)).map_err(de::Error::custom)
    }
}

/// The rest of an object with the already-read discriminator replayed first.
struct Replay<'de, A> {
    first: Option<(Cow<'de, str>, Cow<'de, str>)>,
    map: A,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Replay<'de, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, A::Error> {
        match &self.first {
            Some((k, _)) => seed.deserialize(de::value::CowStrDeserializer::<A::Error>::new(k.clone())).map(Some),
            None => self.map.next_key_seed(seed),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        match self.first.take() {
            Some((_, v)) => seed.deserialize(TagValue::<A::Error>(v, std::marker::PhantomData)),
            None => self.map.next_value_seed(seed),
        }
    }
}

/// The replayed tag value: a string that also fills an `Option` field (a
/// variant whose tag property is not required).
struct TagValue<'de, E>(Cow<'de, str>, std::marker::PhantomData<E>);

impl<'de, E: de::Error> Deserializer<'de> for TagValue<'de, E> {
    type Error = E;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, E> {
        de::value::CowStrDeserializer::<E>::new(self.0).deserialize_any(v)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, E> {
        v.visit_some(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, E> {
        de::value::CowStrDeserializer::<E>::new(self.0).deserialize_enum(name, variants, v)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf unit unit_struct
        newtype_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

impl<'de> Deserialize<'de> for Content<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(ContentVisitor)
    }
}

/// An object key that is borrowed when possible.
struct Key<'de>(Cow<'de, str>);

impl<'de> Deserialize<'de> for Key<'de> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Key<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an object key")
            }
            fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Borrowed(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v.to_owned())))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Key<'de>, E> {
                Ok(Key(Cow::Owned(v)))
            }
        }
        d.deserialize_str(V)
    }
}

impl<'de> Deserializer<'de> for Content<'de> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, serde_json::Error> {
        match self {
            Self::Null => v.visit_unit(),
            Self::Bool(b) => v.visit_bool(b),
            Self::U64(n) => v.visit_u64(n),
            Self::I64(n) => v.visit_i64(n),
            Self::F64(n) => v.visit_f64(n),
            Self::Str(Cow::Borrowed(s)) => v.visit_borrowed_str(s),
            Self::Str(Cow::Owned(s)) => v.visit_string(s),
            Self::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_any(v),
            Self::Seq(a) => {
                let mut seq = de::value::SeqDeserializer::new(a.into_iter());
                let out = v.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(out)
            }
            Self::Map(m) => {
                let entries = m.into_iter().map(|(k, val)| (Content::Str(k), val));
                let mut map = de::value::MapDeserializer::new(entries);
                let out = v.visit_map(&mut map)?;
                map.end()?;
                Ok(out)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, serde_json::Error> {
        match self {
            Self::Null => v.visit_none(),
            Self::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_option(v),
            other @ (Self::Bool(_)
            | Self::U64(_)
            | Self::I64(_)
            | Self::F64(_)
            | Self::Str(_)
            | Self::Seq(_)
            | Self::Map(_)) => v.visit_some(other),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        v: V,
    ) -> Result<V::Value, serde_json::Error> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, serde_json::Error> {
        match self {
            Self::Str(s) => v.visit_enum(de::value::StringDeserializer::new(s.into_owned())),
            Self::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_enum(name, variants, v),
            other @ (Self::Null
            | Self::Bool(_)
            | Self::U64(_)
            | Self::I64(_)
            | Self::F64(_)
            | Self::Seq(_)
            | Self::Map(_)) => other.into_value().deserialize_enum(name, variants, v),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

impl<'de> de::IntoDeserializer<'de, serde_json::Error> for Content<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// Borrowing view: untagged unions try each variant on the same buffer.
impl<'de> Deserializer<'de> for &Content<'de> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, serde_json::Error> {
        match self {
            Content::Null => v.visit_unit(),
            Content::Bool(b) => v.visit_bool(*b),
            Content::U64(n) => v.visit_u64(*n),
            Content::I64(n) => v.visit_i64(*n),
            Content::F64(n) => v.visit_f64(*n),
            Content::Str(s) => KeyRef(s).deserialize_any(v),
            Content::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_any(v),
            Content::Seq(a) => {
                let mut seq = de::value::SeqDeserializer::new(a.iter());
                let out = v.visit_seq(&mut seq)?;
                seq.end()?;
                Ok(out)
            }
            Content::Map(m) => {
                let mut map = de::value::MapDeserializer::new(m.iter().map(|(k, val)| (KeyRef(k), val)));
                let out = v.visit_map(&mut map)?;
                map.end()?;
                Ok(out)
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, serde_json::Error> {
        if matches!(self, Content::Null) { v.visit_none() } else { v.visit_some(self) }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        v: V,
    ) -> Result<V::Value, serde_json::Error> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, serde_json::Error> {
        match self {
            Content::Str(s) => v.visit_enum(de::value::StrDeserializer::new(s)),
            Content::Raw(text) => serde_json::Deserializer::from_str(text).deserialize_enum(name, variants, v),
            other @ (Content::Null
            | Content::Bool(_)
            | Content::U64(_)
            | Content::I64(_)
            | Content::F64(_)
            | Content::Seq(_)
            | Content::Map(_)) => other.clone().deserialize_enum(name, variants, v),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

impl<'de> de::IntoDeserializer<'de, serde_json::Error> for &Content<'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// A buffered string. It is borrowed from the input when it was. Otherwise it is copied.
#[derive(Clone, Copy)]
struct KeyRef<'a, 'de>(&'a Cow<'de, str>);

impl<'de> Deserializer<'de> for KeyRef<'_, 'de> {
    type Error = serde_json::Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, serde_json::Error> {
        match self.0 {
            Cow::Borrowed(s) => v.visit_borrowed_str(s),
            Cow::Owned(s) => v.visit_str(s),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any
    }
}

impl<'de> de::IntoDeserializer<'de, serde_json::Error> for KeyRef<'_, 'de> {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct Price {
        amount: crate::Decimal,
    }

    const BODY: &str = r#"{"amount": 12345678901234567.89}"#;

    #[test]
    fn exact_keeps_a_decimal_from_json_text_and_buffers_any_other_input() {
        let v = Content::exact(&mut serde_json::Deserializer::from_str(BODY)).unwrap();
        assert_matches!(v, Content::Raw(_));
        assert_eq!(v.kind(), Kind::Object);
        assert_eq!(v.decode_ref::<Price>().unwrap().amount.as_str(), "12345678901234567.89");
        let value: Value = serde_json::from_str(BODY).unwrap();
        let buffered = Content::exact(&value).unwrap();
        assert_matches!(buffered, Content::Map(_), "{buffered:?}");
    }

    #[test]
    fn a_tag_is_read_from_raw_text_wherever_it_sits() {
        let late = r#"{"amount": 12345678901234567.89, "method": "card"}"#;
        let v = Content::exact(&mut serde_json::Deserializer::from_str(late)).unwrap();
        assert_eq!(v.tag("method").as_deref(), Some("card"));
        assert_eq!((v.tag("absent"), v.decode_ref::<Price>().unwrap().amount.as_str()), (None, "12345678901234567.89"));
        let buffered: Content<'_> = serde_json::from_str(late).unwrap();
        assert_eq!(buffered.tag("method").as_deref(), Some("card"));
    }

    #[test]
    fn the_kind_of_raw_text_is_read_from_its_start() {
        let kinds = [("1", Kind::Integer), ("-1.5", Kind::Number), ("2e3", Kind::Number), (" [1]", Kind::Array)];
        for (text, kind) in kinds {
            assert_eq!(raw_kind(text), kind, "{text}");
        }
        assert_eq!((raw_kind("\"x\""), raw_kind("true"), raw_kind("null")), (Kind::String, Kind::Bool, Kind::Null));
    }

    /// Which way a union was read.
    #[derive(Debug, PartialEq)]
    enum Read {
        Streamed(String),
        Buffered(Value),
    }

    impl<'de> TaggedUnion<'de> for Read {
        const TAG: &'static str = "kind";
        fn knows(tag: &str) -> bool {
            tag == "cat"
        }
        fn from_tag<D: Deserializer<'de>>(tag: &str, d: D) -> Result<Self, D::Error> {
            IgnoredAny::deserialize(d)?;
            Ok(Self::Streamed(tag.to_owned()))
        }
        fn from_content(v: Content<'de>) -> Result<Self, serde_json::Error> {
            Ok(Self::Buffered(v.into_value()))
        }
    }

    /// A known tag first streams into its variant. A known tag value under another key does not.
    #[test]
    fn only_a_leading_tag_streams() {
        let read = |text: &str| tagged::<_, Read>(&mut serde_json::Deserializer::from_str(text)).unwrap();
        assert_eq!(read(r#"{"kind": "cat", "a": 1}"#), Read::Streamed("cat".into()));
        let other = r#"{"name": "cat", "kind": "dog"}"#;
        assert_eq!(read(other), Read::Buffered(serde_json::from_str(other).unwrap()));
    }
}
