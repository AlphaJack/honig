//! The names of an object's members in the order that its `Serialize` writes them, without writing the values.

use std::fmt;

use serde::Serialize;
use serde::ser::{self, Impossible, SerializeMap, SerializeStruct, Serializer};

/// Returns the names of the members of `v` in the order its `Serialize` writes them: a struct's field order. Empty for
/// a value that is no object. A `serde_json` map would sort them.
pub(crate) fn key_order<T: Serialize + ?Sized>(v: &T) -> Vec<String> {
    v.serialize(Keys).unwrap_or_default()
}

/// Not an object: no names to give.
#[derive(Debug)]
struct NoObject;

impl fmt::Display for NoObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not an object")
    }
}

impl std::error::Error for NoObject {}

impl ser::Error for NoObject {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self
    }
}

/// Collects the names of the object it serializes.
struct Keys;

/// The names collected so far.
struct Names(Vec<String>);

impl SerializeStruct for Names {
    type Ok = Vec<String>;
    type Error = NoObject;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, key: &'static str, _: &T) -> Result<(), NoObject> {
        self.0.push(key.to_owned());
        Ok(())
    }
    fn end(self) -> Result<Vec<String>, NoObject> {
        Ok(self.0)
    }
}

impl SerializeMap for Names {
    type Ok = Vec<String>;
    type Error = NoObject;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NoObject> {
        // A name that is no text (a number) is written as JSON writes it.
        self.0.extend(crate::scalar::scalar_text(key));
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, _: &T) -> Result<(), NoObject> {
        Ok(())
    }
    fn end(self) -> Result<Vec<String>, NoObject> {
        Ok(self.0)
    }
}

/// Every kind of value but an object: no names.
macro_rules! no_object {
    ($($method:ident($($arg:ty),*)),* $(,)?) => {$(
        fn $method(self, $(_: $arg),*) -> Result<Vec<String>, NoObject> {
            Err(NoObject)
        }
    )*};
}

impl Serializer for Keys {
    type Ok = Vec<String>;
    type Error = NoObject;
    type SerializeSeq = Impossible<Vec<String>, NoObject>;
    type SerializeTuple = Impossible<Vec<String>, NoObject>;
    type SerializeTupleStruct = Impossible<Vec<String>, NoObject>;
    type SerializeTupleVariant = Impossible<Vec<String>, NoObject>;
    type SerializeMap = Names;
    type SerializeStruct = Names;
    type SerializeStructVariant = Impossible<Vec<String>, NoObject>;

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

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Vec<String>, NoObject> {
        v.serialize(self)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, v: &T) -> Result<Vec<String>, NoObject> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Vec<String>, NoObject> {
        Err(NoObject)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NoObject> {
        Err(NoObject)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NoObject> {
        Err(NoObject)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct, NoObject> {
        Err(NoObject)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NoObject> {
        Err(NoObject)
    }
    fn serialize_map(self, len: Option<usize>) -> Result<Names, NoObject> {
        Ok(Names(Vec::with_capacity(len.unwrap_or_default())))
    }
    fn serialize_struct(self, _: &'static str, len: usize) -> Result<Names, NoObject> {
        Ok(Names(Vec::with_capacity(len)))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NoObject> {
        Err(NoObject)
    }
}
