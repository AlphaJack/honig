//! The JSON engine behind `json_decode` and `json_encode`. It is `serde_json` or `sonic-rs` with the `simd` feature on
//! the targets that it has SIMD for. A body that fails to decode is decoded again by `serde_json`. That says where.

#[cfg(not(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64"))))]
mod plain;
#[cfg(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
mod sonic;

#[cfg(not(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64"))))]
use plain as imp;
#[cfg(all(feature = "simd", any(target_arch = "x86_64", target_arch = "aarch64")))]
use sonic as imp;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::ClientError;

pub(crate) use imp::{from_slice, to_writer};

/// Returns the text that a value of a format writes: what its length and `pattern` are checked on.
pub fn format_text<T: Serialize + ?Sized>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

/// Converts a value to JSON. Failures become [`ClientError::Encode`].
pub fn to_value<T: Serialize + ?Sized>(v: &T) -> Result<Value, ClientError> {
    serde_json::to_value(crate::finite::Finite(v)).map_err(|e| ClientError::Encode(e.to_string()))
}

/// Adds `k` to an object. A key that it already has gets one more value and its values become a list.
pub(crate) fn push_multi(o: &mut Map<String, Value>, k: String, v: Value) {
    match o.get_mut(&k) {
        Some(Value::Array(a)) => a.push(v),
        Some(existing) => {
            let prev = existing.take();
            *existing = Value::Array(vec![prev, v]);
        }
        None => {
            o.insert(k, v);
        }
    }
}
