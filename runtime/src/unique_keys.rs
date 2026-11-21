//! A JSON object type that writes each key once. A generated struct with `additionalProperties` writes its declared
//! fields and then the entries of its extra map. Decoding never puts a declared name in that map. A value built by
//! hand can, and the object would then contain the name twice: RFC 8259 section 4 leaves such an object to each
//! reader, and the struct's own decoder refuses it. Generated structs with an extra map encode through
//! [`UniqueKeys`], which drops an entry whose key came before. So the declared field wins.

use serde::ser::{Error as _, Impossible, Serialize, SerializeMap, Serializer};

/// A serializer whose maps skip a key they already wrote. Everything else passes through unchanged.
#[derive(Debug)]
pub struct UniqueKeys<S>(pub S);

/// A map that remembers its keys.
#[derive(Debug)]
pub struct Keys<M> {
    map: M,
    seen: Vec<String>,
    /// The value of the last key is skipped: the key came before.
    skip: bool,
}

impl<M: SerializeMap> SerializeMap for Keys<M> {
    type Ok = M::Ok;
    type Error = M::Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        let name = key.serialize(KeyText).map_err(M::Error::custom)?;
        self.skip = self.seen.contains(&name);
        if self.skip {
            return Ok(());
        }
        self.seen.push(name);
        self.map.serialize_key(key)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Self::Error> {
        if std::mem::take(&mut self.skip) {
            return Ok(());
        }
        self.map.serialize_value(value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.map.end()
    }
}

fn not_text() -> serde_json::Error {
    serde_json::Error::custom("an object key must be a string")
}

/// The text of a key. JSON object keys are strings.
struct KeyText;

macro_rules! not_text {
    ($($method:ident($($arg:ty),*)),*) => {$(
        fn $method(self, $(_: $arg),*) -> Result<String, serde_json::Error> {
            Err(not_text())
        }
    )*};
}

impl Serializer for KeyText {
    type Ok = String;
    type Error = serde_json::Error;
    type SerializeSeq = Impossible<String, serde_json::Error>;
    type SerializeTuple = Impossible<String, serde_json::Error>;
    type SerializeTupleStruct = Impossible<String, serde_json::Error>;
    type SerializeTupleVariant = Impossible<String, serde_json::Error>;
    type SerializeMap = Impossible<String, serde_json::Error>;
    type SerializeStruct = Impossible<String, serde_json::Error>;
    type SerializeStructVariant = Impossible<String, serde_json::Error>;

    fn serialize_str(self, v: &str) -> Result<String, serde_json::Error> {
        Ok(v.to_owned())
    }

    not_text!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_bytes(&[u8]),
        serialize_none(),
        serialize_unit(),
        serialize_unit_struct(&'static str),
        serialize_unit_variant(&'static str, u32, &'static str)
    );

    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<String, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<String, serde_json::Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<String, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct, serde_json::Error> {
        Err(not_text())
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, serde_json::Error> {
        Err(not_text())
    }
}

macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*)),*) => {$(
        fn $method(self, $($arg: $ty),*) -> Result<S::Ok, S::Error> {
            let Self(inner) = self;
            inner.$method($($arg),*)
        }
    )*};
}

impl<S: Serializer> Serializer for UniqueKeys<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = S::SerializeSeq;
    type SerializeTuple = S::SerializeTuple;
    type SerializeTupleStruct = S::SerializeTupleStruct;
    type SerializeTupleVariant = S::SerializeTupleVariant;
    type SerializeMap = Keys<S::SerializeMap>;
    type SerializeStruct = S::SerializeStruct;
    type SerializeStructVariant = S::SerializeStructVariant;

    /// A struct with a flattened map encodes as a map.
    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        let Self(inner) = self;
        Ok(Keys { map: inner.serialize_map(len)?, seen: Vec::new(), skip: false })
    }

    forward!(
        serialize_bool(v: bool),
        serialize_i8(v: i8),
        serialize_i16(v: i16),
        serialize_i32(v: i32),
        serialize_i64(v: i64),
        serialize_u8(v: u8),
        serialize_u16(v: u16),
        serialize_u32(v: u32),
        serialize_u64(v: u64),
        serialize_f32(v: f32),
        serialize_f64(v: f64),
        serialize_char(v: char),
        serialize_str(v: &str),
        serialize_bytes(v: &[u8]),
        serialize_none(),
        serialize_unit(),
        serialize_unit_struct(name: &'static str),
        serialize_unit_variant(name: &'static str, index: u32, variant: &'static str)
    );

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<S::Ok, S::Error> {
        let Self(inner) = self;
        inner.serialize_some(v)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, v: &T) -> Result<S::Ok, S::Error> {
        let Self(inner) = self;
        inner.serialize_newtype_struct(name, v)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        let Self(inner) = self;
        inner.serialize_newtype_variant(name, index, variant, v)
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        let Self(inner) = self;
        inner.serialize_seq(len)
    }
    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        let Self(inner) = self;
        inner.serialize_tuple(len)
    }
    fn serialize_tuple_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeTupleStruct, S::Error> {
        let Self(inner) = self;
        inner.serialize_tuple_struct(name, len)
    }
    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        let Self(inner) = self;
        inner.serialize_tuple_variant(name, index, variant, len)
    }
    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, S::Error> {
        let Self(inner) = self;
        inner.serialize_struct(name, len)
    }
    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        let Self(inner) = self;
        inner.serialize_struct_variant(name, index, variant, len)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[derive(serde::Serialize)]
    #[serde(remote = "Self")]
    struct Product {
        name: String,
        #[serde(flatten)]
        extra: BTreeMap<String, serde_json::Value>,
    }

    impl Serialize for Product {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            Self::serialize(self, UniqueKeys(s))
        }
    }

    #[test]
    fn an_extra_entry_named_like_a_field_is_dropped() {
        let extra = BTreeMap::from([("name".to_owned(), "lamp".into()), ("sku".to_owned(), "LMP-1".into())]);
        let product = Product { name: "Desk lamp".to_owned(), extra };
        assert_eq!(serde_json::to_string(&product).unwrap(), r#"{"name":"Desk lamp","sku":"LMP-1"}"#);
    }
}
