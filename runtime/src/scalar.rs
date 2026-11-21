//! The text of a value in a parameter. That is what `serde_json::Value` would spell. It is written straight from
//! `Serialize` when it can.

use std::fmt;

use serde::{Serialize, Serializer};
use serde_json::Value;

/// Returns a JSON value as parameter text. A string loses its quotes. Null becomes nothing.
pub(crate) fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other @ (Value::Array(_) | Value::Object(_)) => other.to_string(),
    }
}

/// Returns [`scalar`] without a copy of a string or a boolean.
#[cfg(feature = "xml")]
pub(crate) fn scalar_ref(v: &Value) -> std::borrow::Cow<'_, str> {
    match v {
        Value::String(s) => std::borrow::Cow::Borrowed(s),
        Value::Bool(b) => std::borrow::Cow::Borrowed(if *b { "true" } else { "false" }),
        other @ (Value::Null | Value::Number(_) | Value::Array(_) | Value::Object(_)) => {
            std::borrow::Cow::Owned(scalar(other))
        }
    }
}

/// Returns a scalar parameter as wire text written straight from `Serialize`.
/// `None` for null, arrays and objects. They use the `Value` path.
pub(crate) fn scalar_text<T: Serialize + ?Sized>(v: &T) -> Option<String> {
    crate::finite::Finite(v).serialize(ScalarText).ok().flatten()
}

/// The one field of a raw value with its JSON text. That is a number as written or the contents of a string.
struct RawText(Option<String>);

impl serde::ser::SerializeStruct for RawText {
    type Ok = Option<String>;
    type Error = NotScalar;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, _: &'static str, v: &T) -> Result<(), NotScalar> {
        let json = v.serialize(ScalarText)?.ok_or(NotScalar)?;
        self.0 = Some(if json.starts_with('"') { serde_json::from_str(&json).ok().ok_or(NotScalar)? } else { json });
        Ok(())
    }
    fn end(self) -> Result<Option<String>, NotScalar> {
        Ok(self.0)
    }
}

/// Not a scalar: the caller falls back to `serde_json::Value`.
#[derive(Debug)]
struct NotScalar;

impl fmt::Display for NotScalar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a scalar")
    }
}

impl std::error::Error for NotScalar {}

impl serde::ser::Error for NotScalar {
    fn custom<M: fmt::Display>(_: M) -> Self {
        Self
    }
}

/// Serializes a scalar the way `serde_json::Value` would spell it.
struct ScalarText;

macro_rules! scalar_text_display {
    ($($f:ident: $t:ty),*) => {
        $(fn $f(self, v: $t) -> Result<Option<String>, NotScalar> { Ok(Some(v.to_string())) })*
    };
}

impl Serializer for ScalarText {
    type Ok = Option<String>;
    type Error = NotScalar;
    type SerializeSeq = serde::ser::Impossible<Option<String>, NotScalar>;
    type SerializeTuple = serde::ser::Impossible<Option<String>, NotScalar>;
    type SerializeTupleStruct = serde::ser::Impossible<Option<String>, NotScalar>;
    type SerializeTupleVariant = serde::ser::Impossible<Option<String>, NotScalar>;
    type SerializeMap = serde::ser::Impossible<Option<String>, NotScalar>;
    type SerializeStruct = RawText;
    type SerializeStructVariant = serde::ser::Impossible<Option<String>, NotScalar>;

    scalar_text_display!(
        serialize_bool: bool, serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64,
        serialize_i128: i128, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32, serialize_u64: u64,
        serialize_u128: u128, serialize_char: char, serialize_str: &str
    );

    fn serialize_f32(self, v: f32) -> Result<Option<String>, NotScalar> {
        Ok(serde_json::Number::from_f64(f64::from(v)).map(|n| n.to_string()))
    }
    fn serialize_f64(self, v: f64) -> Result<Option<String>, NotScalar> {
        Ok(serde_json::Number::from_f64(v).map(|n| n.to_string()))
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<Option<String>, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_none(self) -> Result<Option<String>, NotScalar> {
        Ok(None)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Option<String>, NotScalar> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<Option<String>, NotScalar> {
        Ok(None)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<Option<String>, NotScalar> {
        Ok(None)
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<Option<String>, NotScalar> {
        Ok(Some(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        v: &T,
    ) -> Result<Option<String>, NotScalar> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<Option<String>, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeTupleStruct, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NotScalar> {
        Err(NotScalar)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, NotScalar> {
        Err(NotScalar)
    }
    // The raw value of serde_json (a `format: decimal` number). Its text as written and never through an `f64`.
    fn serialize_struct(self, name: &'static str, _: usize) -> Result<RawText, NotScalar> {
        if name == crate::raw_value::RAW_VALUE { Ok(RawText(None)) } else { Err(NotScalar) }
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NotScalar> {
        Err(NotScalar)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Serialize, Serializer};

    use super::scalar_text;

    struct Bytes;
    impl Serialize for Bytes {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_bytes(b"ab")
        }
    }

    #[derive(Serialize)]
    struct Unit;
    #[derive(Serialize)]
    struct Id(i32);
    #[derive(Serialize)]
    enum Kind {
        Cat,
    }

    #[test]
    fn scalars_are_written_as_json_spells_them() {
        assert_eq!(scalar_text(&1.5_f32).as_deref(), Some("1.5"));
        assert_eq!(scalar_text(&2.5_f64).as_deref(), Some("2.5"));
        assert_eq!(scalar_text(&Id(5)).as_deref(), Some("5"));
        assert_eq!(scalar_text(&Kind::Cat).as_deref(), Some("Cat"));
        assert_eq!((scalar_text(&()), scalar_text(&Unit), scalar_text(&Bytes)), (None, None, None));
    }
}
