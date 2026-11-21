//! JSON numbers with a zero fractional part are integers. JSON Schema 2020-12 says so ("1.0 is an integer"), and
//! `OpenAPI` 3.1 and 3.2 use it. serde decodes `3.0` as a float only, so `count: 3.0` would not decode into an integer
//! field. [`Ints`] wraps a deserializer at every depth and hands such a number on as an integer. A float field still
//! accepts it, and a `serde_json::Value` stores `3`, the same number.

use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};

/// A deserializer whose integral floats arrive as integers, at every depth.
pub(crate) struct Ints<D>(pub D);

/// Returns an integral float in the range of `u64` or `i64` as that integer.
fn integral(v: f64) -> Option<Result<u64, i64>> {
    const TWO_63: f64 = 9_223_372_036_854_775_808.0;
    const TWO_64: f64 = 18_446_744_073_709_551_616.0;
    if v.fract() != 0.0 || !v.is_finite() {
        return None;
    }
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "the range is checked first")]
    if (0.0..TWO_64).contains(&v) {
        Some(Ok(v as u64))
    } else if (-TWO_63..0.0).contains(&v) {
        Some(Err(v as i64))
    } else {
        None
    }
}

/// A visitor whose integral floats arrive as integers and whose nested values decode through [`Ints`].
struct Visit<V>(V);

impl<'de, V: Visitor<'de>> Visitor<'de> for Visit<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(v) = self;
        v.expecting(f)
    }

    fn visit_f64<E: de::Error>(self, n: f64) -> Result<Self::Value, E> {
        let Self(v) = self;
        match integral(n) {
            Some(Ok(u)) => v.visit_u64(u),
            Some(Err(i)) => v.visit_i64(i),
            None => v.visit_f64(n),
        }
    }

    fn visit_f32<E: de::Error>(self, n: f32) -> Result<Self::Value, E> {
        self.visit_f64(f64::from(n))
    }

    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_bool(b)
    }

    fn visit_i64<E: de::Error>(self, n: i64) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_i64(n)
    }

    fn visit_u64<E: de::Error>(self, n: u64) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_u64(n)
    }

    fn visit_i128<E: de::Error>(self, n: i128) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_i128(n)
    }

    fn visit_u128<E: de::Error>(self, n: u128) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_u128(n)
    }

    fn visit_char<E: de::Error>(self, c: char) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_char(c)
    }

    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_str(s)
    }

    fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_borrowed_str(s)
    }

    fn visit_string<E: de::Error>(self, s: String) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_string(s)
    }

    fn visit_bytes<E: de::Error>(self, b: &[u8]) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_bytes(b)
    }

    fn visit_borrowed_bytes<E: de::Error>(self, b: &'de [u8]) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_borrowed_bytes(b)
    }

    fn visit_byte_buf<E: de::Error>(self, b: Vec<u8>) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_byte_buf(b)
    }

    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_none()
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        let Self(v) = self;
        v.visit_some(Ints(d))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        let Self(v) = self;
        v.visit_unit()
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        let Self(v) = self;
        v.visit_newtype_struct(Ints(d))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        let Self(v) = self;
        v.visit_seq(Ints(seq))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        let Self(v) = self;
        v.visit_map(Ints(map))
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
        let Self(v) = self;
        v.visit_enum(Ints(data))
    }
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Ints<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        let Self(seed) = self;
        seed.deserialize(Ints(d))
    }
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Ints<A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, A::Error> {
        let Self(seq) = self;
        seq.next_element_seed(Ints(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        let Self(seq) = self;
        seq.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Ints<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, A::Error> {
        // Keys are strings: nothing to change.
        let Self(map) = self;
        map.next_key_seed(seed)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        let Self(map) = self;
        map.next_value_seed(Ints(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        let Self(map) = self;
        map.size_hint()
    }
}

impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for Ints<A> {
    type Error = A::Error;
    type Variant = Ints<A::Variant>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, Self::Variant), A::Error> {
        let Self(data) = self;
        data.variant_seed(seed).map(|(v, variant)| (v, Ints(variant)))
    }
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Ints<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        let Self(variant) = self;
        variant.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        let Self(variant) = self;
        variant.newtype_variant_seed(Ints(seed))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, A::Error> {
        let Self(variant) = self;
        variant.tuple_variant(len, Visit(v))
    }

    fn struct_variant<V: Visitor<'de>>(self, fields: &'static [&'static str], v: V) -> Result<V::Value, A::Error> {
        let Self(variant) = self;
        variant.struct_variant(fields, Visit(v))
    }
}

macro_rules! wrap {
    ($($method:ident),*) => {$(
        fn $method<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Self::Error> {
            let Self(d) = self;
            d.$method(Visit(v))
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Ints<D> {
    type Error = D::Error;

    wrap!(
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
        deserialize_map,
        deserialize_identifier,
        deserialize_ignored_any
    );

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        d.deserialize_unit_struct(name, Visit(v))
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, v: V) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        // A raw JSON value keeps the text as written (decimals keep every digit): it passes through untouched.
        if name == crate::raw_value::RAW_VALUE {
            return d.deserialize_newtype_struct(name, v);
        }
        d.deserialize_newtype_struct(name, Visit(v))
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, v: V) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        d.deserialize_tuple(len, Visit(v))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        d.deserialize_tuple_struct(name, len, Visit(v))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        d.deserialize_struct(name, fields, Visit(v))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Self::Error> {
        let Self(d) = self;
        d.deserialize_enum(name, variants, Visit(v))
    }

    fn is_human_readable(&self) -> bool {
        let Self(d) = self;
        d.is_human_readable()
    }
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Deserialize)]
    struct Order {
        quantity: u32,
        offset: i64,
        price: f64,
        tags: Vec<u8>,
        extra: serde_json::Value,
    }

    fn decode<T: de::DeserializeOwned>(json: &str) -> Result<T, serde_json::Error> {
        let mut d = serde_json::Deserializer::from_str(json);
        T::deserialize(Ints(&mut d))
    }

    #[test]
    fn integral_floats_decode_as_integers_at_any_depth() {
        let o: Order =
            decode(r#"{"quantity": 3.0, "offset": -2.0, "price": 4.0, "tags": [1.0, 2], "extra": {"n": 5.0}}"#)
                .unwrap();
        assert_eq!((o.quantity, o.offset, o.price, o.tags), (3, -2, 4.0, vec![1, 2]));
        assert_eq!(o.extra, serde_json::json!({"n": 5}));
        // Plain serde refuses the same body.
        assert!(
            serde_json::from_str::<Order>(r#"{"quantity": 3.0, "offset": 0, "price": 1, "tags": [], "extra": 0}"#)
                .is_err()
        );
    }

    #[test]
    fn a_fraction_or_a_number_out_of_range_stays_a_float() {
        assert!(decode::<u32>("3.5").is_err());
        assert!(decode::<i64>("1e300").is_err());
        assert_eq!(decode::<f64>("2.5").unwrap(), 2.5);
        assert_eq!(decode::<u64>("18446744073709549568.0").unwrap(), 18_446_744_073_709_549_568);
    }

    /// A newtype decodes its integral float as an integer. A decimal keeps its text as written.
    #[test]
    fn a_newtype_reads_integers_and_a_decimal_keeps_its_digits() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Id(u32);
        assert_eq!(decode::<Id>("3.0").unwrap(), Id(3));
        let wide: crate::Decimal = decode("12345678901234567.89").unwrap();
        assert_eq!(wide.to_string(), "12345678901234567.89");
    }
}
