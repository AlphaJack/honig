//! A JSON object type that refuses an array. A derived serde struct decodes an array too, field by field in order:
//! `[{}]` would become a struct whose first field is `{}`. JSON Schema's `type: object` allows no array. Generated
//! structs decode through [`NoSeq`], which passes every call on and turns an array into an error.

use std::fmt;

use serde::de::{Deserializer, MapAccess, Visitor};

/// A deserializer that hands a struct's visitor only objects. Everything else passes through unchanged.
#[derive(Debug)]
pub struct NoSeq<D>(pub D);

/// A struct's visitor without its array form.
struct Objects<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for Objects<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(visitor) = self;
        visitor.expecting(f)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let Self(visitor) = self;
        visitor.visit_map(map)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        let Self(visitor) = self;
        visitor.visit_newtype_struct(d)
    }
}

macro_rules! forward {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
            let Self(inner) = self;
            inner.$method(v)
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for NoSeq<D> {
    type Error = D::Error;

    forward!(
        deserialize_any,
        deserialize_bool,
        deserialize_i8,
        deserialize_i16,
        deserialize_i32,
        deserialize_i64,
        deserialize_i128,
        deserialize_u8,
        deserialize_u16,
        deserialize_u32,
        deserialize_u64,
        deserialize_u128,
        deserialize_f32,
        deserialize_f64,
        deserialize_char,
        deserialize_str,
        deserialize_string,
        deserialize_bytes,
        deserialize_byte_buf,
        deserialize_option,
        deserialize_unit,
        deserialize_seq,
        deserialize_identifier,
        deserialize_ignored_any
    );

    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_map(Objects(v))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_struct(name, fields, Objects(v))
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_unit_struct(name, v)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_newtype_struct(name, v)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_tuple(len, v)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_tuple_struct(name, len, v)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(inner) = self;
        inner.deserialize_enum(name, variants, v)
    }

    fn is_human_readable(&self) -> bool {
        let Self(inner) = self;
        inner.is_human_readable()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Product {
        name: Option<String>,
    }

    fn decode(json: &str) -> Result<Product, serde_json::Error> {
        let mut d = serde_json::Deserializer::from_str(json);
        Product::deserialize(NoSeq(&mut d))
    }

    #[test]
    fn an_object_decodes_and_an_array_does_not() {
        assert_eq!(decode(r#"{"name":"lamp"}"#).unwrap(), Product { name: Some("lamp".to_owned()) });
        // Derived serde alone decodes `["lamp"]` as `name: "lamp"`.
        assert!(serde_json::from_str::<Product>(r#"["lamp"]"#).is_ok());
        let err = decode(r#"["lamp"]"#).unwrap_err().to_string();
        assert!(err.contains("invalid type: sequence"), "{err}");
    }
}
