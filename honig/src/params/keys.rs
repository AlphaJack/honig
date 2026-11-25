//! The members of an object in the order that its `Serialize` writes them: their names alone, or with their values.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Impossible, SerializeMap, SerializeStruct, Serializer};

/// Returns the names of the members of `v` in the order its `Serialize` writes them: a struct's field order. Empty for
/// a value that is no object. A `serde_json` map would sort them.
pub(crate) fn key_order<T: Serialize + ?Sized>(v: &T) -> Vec<String> {
    v.serialize(Keys { values: false }).unwrap_or_default().into_iter().map(|(k, _)| k).collect()
}

/// Returns the members of `v` with their values as JSON, in the order its `Serialize` writes them. `None` for a value
/// that is no object. One pass and no sorted map, as `serde_json` would make.
///
/// # Errors
/// A value that does not serialize as JSON, such as a `NaN`.
pub(crate) fn members<T: Serialize + ?Sized>(
    v: &T,
) -> Result<Option<Vec<(String, serde_json::Value)>>, crate::ClientError> {
    match v.serialize(Keys { values: true }) {
        Ok(members) => Ok(Some(members)),
        Err(NoObject(None)) => Ok(None),
        Err(NoObject(Some(why))) => Err(crate::ClientError::Encode(why)),
    }
}

/// Not an object: no names to give. Or a value that does not serialize, with why.
#[derive(Debug)]
struct NoObject(Option<String>);

impl fmt::Display for NoObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_deref().unwrap_or("not an object"))
    }
}

impl std::error::Error for NoObject {}

impl ser::Error for NoObject {
    fn custom<T: fmt::Display>(why: T) -> Self {
        Self(Some(why.to_string()))
    }
}

/// Collects the members of the object it serializes, with their values when `values`.
struct Keys {
    values: bool,
}

/// The members collected so far. A value is `Null` when only names are collected.
struct Names {
    values: bool,
    members: Vec<(String, serde_json::Value)>,
}

impl Names {
    fn value<T: Serialize + ?Sized>(&self, v: &T) -> Result<serde_json::Value, NoObject> {
        if self.values {
            crate::json::to_value(v).map_err(|e| NoObject(Some(e.to_string())))
        } else {
            Ok(serde_json::Value::Null)
        }
    }
}

impl SerializeStruct for Names {
    type Ok = Vec<(String, serde_json::Value)>;
    type Error = NoObject;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, v: &T) -> Result<(), NoObject> {
        let value = self.value(v)?;
        self.members.push((key.to_owned(), value));
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, NoObject> {
        Ok(self.members)
    }
}

impl SerializeMap for Names {
    type Ok = Vec<(String, serde_json::Value)>;
    type Error = NoObject;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NoObject> {
        // A name that is no text (a number) is written as JSON writes it.
        self.members.extend(crate::scalar::scalar_text(key).map(|k| (k, serde_json::Value::Null)));
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), NoObject> {
        let value = self.value(v)?;
        if let Some((_, slot)) = self.members.last_mut() {
            *slot = value;
        }
        Ok(())
    }
    fn end(self) -> Result<Self::Ok, NoObject> {
        Ok(self.members)
    }
}

/// Every kind of value but an object: no names.
macro_rules! no_object {
    ($($method:ident($($arg:ty),*)),* $(,)?) => {$(
        fn $method(self, $(_: $arg),*) -> Result<Vec<(String, serde_json::Value)>, NoObject> {
            Err(NoObject(None))
        }
    )*};
}

impl Serializer for Keys {
    type Ok = Vec<(String, serde_json::Value)>;
    type Error = NoObject;
    type SerializeSeq = Impossible<Vec<(String, serde_json::Value)>, NoObject>;
    type SerializeTuple = Impossible<Vec<(String, serde_json::Value)>, NoObject>;
    type SerializeTupleStruct = Impossible<Vec<(String, serde_json::Value)>, NoObject>;
    type SerializeTupleVariant = Impossible<Vec<(String, serde_json::Value)>, NoObject>;
    type SerializeMap = Names;
    type SerializeStruct = Names;
    type SerializeStructVariant = Impossible<Vec<(String, serde_json::Value)>, NoObject>;

    no_object!(
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_f32(f32),
        serialize_f64(f64),
        serialize_char(char),
        serialize_str(&str),
        serialize_bytes(&[u8]),
        serialize_none(),
        serialize_unit(),
        serialize_unit_struct(&'static str),
        serialize_unit_variant(&'static str, u32, &'static str),
    );

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Vec<(String, serde_json::Value)>, NoObject> {
        v.serialize(self)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<Vec<(String, serde_json::Value)>, NoObject> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Vec<(String, serde_json::Value)>, NoObject> {
        Err(NoObject(None))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NoObject> {
        Err(NoObject(None))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NoObject> {
        Err(NoObject(None))
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct, NoObject> {
        Err(NoObject(None))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NoObject> {
        Err(NoObject(None))
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Names, NoObject> {
        Ok(Names { values: self.values, members: Vec::with_capacity(len.unwrap_or_default()) })
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Names, NoObject> {
        Ok(Names { values: self.values, members: Vec::with_capacity(len) })
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NoObject> {
        Err(NoObject(None))
    }
}
