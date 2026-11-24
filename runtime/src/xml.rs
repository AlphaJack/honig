//! XML bodies shaped by the `xml` objects of the schemas: decoded into their types.

use std::borrow::Cow;

use serde::de::value::{StrDeserializer, StringDeserializer};
use serde::de::{DeserializeOwned, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::coerce::CoerceError;
use crate::text::decode_err;
use crate::{ClientError, coerce::Coerce};

mod stream;

use crate::xml_tree::{Role, Xml, XmlShape, parse};

/// Decodes an XML body of a response with this status.
/// Errors: the body is not well-formed XML or does not match the type.
pub fn xml_decode<T: DeserializeOwned + XmlShape>(status: Option<u16>, body: &[u8]) -> Result<T, ClientError> {
    let text = crate::text::utf8_text(status, body)?;
    // Most bodies decode straight from the events of the parser. The others go through the converted elements. So do
    // those that do not decode. The converted elements also say what is wrong.
    if let Some(v) = stream::decode(text, T::xml()) {
        return Ok(v);
    }
    let value = parse(text, Role::Shaped(T::xml())).map_err(|e| decode_err(status, body, e))?;
    T::deserialize(XmlDe(value)).map_err(|e| decode_err(status, body, e))
}

/// Decodes an [`Xml`] value into a type as [`Coerce`] decodes a `Value`. Text goes through `Coerce` itself. So a number
/// or a boolean in the text of an element decodes the same. Objects and lists are decoded without a `Value`.
struct XmlDe(Xml);

/// A method for which only the text of an element has a meaning. `Coerce` decodes a leaf. An object or list goes
/// through a `Value`. The `Value` reports why it does not fit.
macro_rules! through_coerce {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
            Coerce(self.0.into_value()).$method(v)
        }
    )*};
}

impl<'de> Deserializer<'de> for XmlDe {
    type Error = CoerceError;

    through_coerce!(
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_unit,
        deserialize_identifier
    );

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Xml::List(items) => v.visit_seq(XmlSeq(items.into_iter())),
            Xml::Obj(props) => v.visit_map(XmlMap(props.into_iter(), None)),
            leaf @ (Xml::Null | Xml::Text(_)) => Coerce(leaf.into_value()).deserialize_any(v),
        }
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Xml::Null => v.visit_none(),
            other @ (Xml::Text(_) | Xml::List(_) | Xml::Obj(_)) => v.visit_some(Self(other)),
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        Coerce(self.0.into_value()).deserialize_unit_struct(name, v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            leaf @ (Xml::Null | Xml::Text(_)) => Coerce(leaf.into_value()).deserialize_newtype_struct(name, v),
            other @ (Xml::List(_) | Xml::Obj(_)) => v.visit_newtype_struct(Self(other)),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Xml::List(items) => v.visit_seq(XmlSeq(items.into_iter())),
            // A single repeated element arrives alone.
            obj @ Xml::Obj(_) => v.visit_seq(XmlSeq(vec![obj].into_iter())),
            leaf @ (Xml::Null | Xml::Text(_)) => Coerce(leaf.into_value()).deserialize_seq(v),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, v: V) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _n: &'static str,
        _len: usize,
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.deserialize_seq(v)
    }

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        match self.0 {
            Xml::Obj(props) => v.visit_map(XmlMap(props.into_iter(), None)),
            other @ (Xml::Null | Xml::Text(_) | Xml::List(_)) => Coerce(other.into_value()).deserialize_map(v),
        }
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _n: &'static str,
        _f: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        self.deserialize_map(v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, CoerceError> {
        Coerce(self.0.into_value()).deserialize_enum(name, variants, v)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, CoerceError> {
        v.visit_unit()
    }
}

struct XmlSeq(std::vec::IntoIter<Xml>);

impl<'de> SeqAccess<'de> for XmlSeq {
    type Error = CoerceError;
    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, CoerceError> {
        self.0.next().map(|v| seed.deserialize(XmlDe(v))).transpose()
    }
}

struct XmlMap(std::vec::IntoIter<(Cow<'static, str>, Xml)>, Option<Xml>);

impl<'de> MapAccess<'de> for XmlMap {
    type Error = CoerceError;
    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, CoerceError> {
        match self.0.next() {
            Some((k, v)) => {
                self.1 = Some(v);
                // A name and never a value to coerce. The property name of a schema is borrowed without a copy.
                match k {
                    Cow::Borrowed(k) => seed.deserialize(StrDeserializer::<CoerceError>::new(k)).map(Some),
                    Cow::Owned(k) => seed.deserialize(StringDeserializer::<CoerceError>::new(k)).map(Some),
                }
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, CoerceError> {
        seed.deserialize(XmlDe(self.1.take().unwrap_or(Xml::Null)))
    }
}
