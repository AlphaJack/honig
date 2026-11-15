//! Writing numbers as JSON has them. A JSON number (RFC 8259 section 6) is finite. So an `OpenAPI` `number` holds no
//! `NaN` and no infinity. `serde_json` writes those as `null`, which a non-nullable field never is. Through
//! [`Finite`] they are an error instead.

use std::fmt::Display;

use serde::ser::{
    self, Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant, Serializer,
};

/// `T` serialized with every `f32` and `f64` in it checked to be finite.
pub(crate) struct Finite<'a, T: ?Sized>(pub &'a T);

impl<T: Serialize + ?Sized> Serialize for Finite<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(Checked(s))
    }
}

/// Returns the error for a float JSON cannot hold.
fn not_finite<E: ser::Error>(v: impl Display) -> E {
    E::custom(format!("{v} is not a JSON number: OpenAPI numbers are finite"))
}

/// A serializer that refuses non-finite floats. It passes everything else to `S` with its compound parts too.
struct Checked<S>(S);

macro_rules! forward {
    ($($method:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $method(self, $($arg: $ty),*) -> Result<S::Ok, S::Error> {
            let Self(inner) = self;
            inner.$method($($arg),*)
        }
    )*};
}

impl<S: Serializer> Serializer for Checked<S> {
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = Checked<S::SerializeSeq>;
    type SerializeTuple = Checked<S::SerializeTuple>;
    type SerializeTupleStruct = Checked<S::SerializeTupleStruct>;
    type SerializeTupleVariant = Checked<S::SerializeTupleVariant>;
    type SerializeMap = Checked<S::SerializeMap>;
    type SerializeStruct = Checked<S::SerializeStruct>;
    type SerializeStructVariant = Checked<S::SerializeStructVariant>;

    forward! {
        serialize_bool(v: bool);
        serialize_i8(v: i8);
        serialize_i16(v: i16);
        serialize_i32(v: i32);
        serialize_i64(v: i64);
        serialize_i128(v: i128);
        serialize_u8(v: u8);
        serialize_u16(v: u16);
        serialize_u32(v: u32);
        serialize_u64(v: u64);
        serialize_u128(v: u128);
        serialize_char(v: char);
        serialize_str(v: &str);
        serialize_bytes(v: &[u8]);
        serialize_none();
        serialize_unit();
        serialize_unit_struct(name: &'static str);
        serialize_unit_variant(name: &'static str, index: u32, variant: &'static str);
    }

    fn serialize_f32(self, v: f32) -> Result<S::Ok, S::Error> {
        if v.is_finite() { self.0.serialize_f32(v) } else { Err(not_finite(v)) }
    }

    fn serialize_f64(self, v: f64) -> Result<S::Ok, S::Error> {
        if v.is_finite() { self.0.serialize_f64(v) } else { Err(not_finite(v)) }
    }

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_some(&Finite(v))
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, name: &'static str, v: &T) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_struct(name, &Finite(v))
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<S::Ok, S::Error> {
        self.0.serialize_newtype_variant(name, index, variant, &Finite(v))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, S::Error> {
        self.0.serialize_seq(len).map(Checked)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, S::Error> {
        self.0.serialize_tuple(len).map(Checked)
    }

    fn serialize_tuple_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeTupleStruct, S::Error> {
        self.0.serialize_tuple_struct(name, len).map(Checked)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, S::Error> {
        self.0.serialize_tuple_variant(name, index, variant, len).map(Checked)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, S::Error> {
        self.0.serialize_map(len).map(Checked)
    }

    fn serialize_struct(self, name: &'static str, len: usize) -> Result<Self::SerializeStruct, S::Error> {
        self.0.serialize_struct(name, len).map(Checked)
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, S::Error> {
        self.0.serialize_struct_variant(name, index, variant, len).map(Checked)
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

/// The compound parts. Each element, key, value and field is checked. Then `end` runs as `S` has it.
macro_rules! compound {
    ($trait:ident { $($method:ident($($arg:ident: $ty:ty),*);)* }) => {
        impl<S: $trait> $trait for Checked<S> {
            type Ok = S::Ok;
            type Error = S::Error;
            $(
                fn $method<T: Serialize + ?Sized>(&mut self, $($arg: $ty,)* v: &T) -> Result<(), S::Error> {
                    let Self(inner) = self;
                    inner.$method($($arg,)* &Finite(v))
                }
            )*
            fn end(self) -> Result<S::Ok, S::Error> {
                self.0.end()
            }
        }
    };
}

compound!(SerializeSeq { serialize_element(); });
compound!(SerializeTuple { serialize_element(); });
compound!(SerializeTupleStruct { serialize_field(); });
compound!(SerializeTupleVariant { serialize_field(); });
compound!(SerializeStruct { serialize_field(key: &'static str); });
compound!(SerializeStructVariant { serialize_field(key: &'static str); });

impl<S: SerializeMap> SerializeMap for Checked<S> {
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, k: &T) -> Result<(), S::Error> {
        self.0.serialize_key(&Finite(k))
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), S::Error> {
        self.0.serialize_value(&Finite(v))
    }

    fn end(self) -> Result<S::Ok, S::Error> {
        self.0.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Serialize)]
    struct Reading {
        at: Vec<Option<f64>>,
        by: std::collections::BTreeMap<String, f32>,
    }

    #[test]
    fn a_non_finite_float_anywhere_is_an_error_and_a_finite_one_is_written() {
        let ok = Reading { at: vec![Some(1.5), None], by: [("x".to_owned(), 2.0)].into() };
        assert_eq!(serde_json::to_string(&Finite(&ok)).unwrap(), r#"{"at":[1.5,null],"by":{"x":2.0}}"#);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let nested = Reading { at: vec![Some(bad)], by: std::collections::BTreeMap::new() };
            let e = serde_json::to_string(&Finite(&nested)).unwrap_err().to_string();
            assert!(e.contains("is not a JSON number"), "{e}");
        }
        let by = Reading { at: vec![], by: [("x".to_owned(), f32::NAN)].into() };
        assert!(serde_json::to_value(Finite(&by)).is_err(), "a map value too");
    }
}
