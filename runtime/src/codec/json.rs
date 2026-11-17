//! JSON bodies and JSON values inside other bodies.

use std::cell::RefCell;

use serde::Deserialize;
use serde::Serialize;
use serde::de::{self, DeserializeOwned, Deserializer};
use serde_json::Value;

use crate::Error;

/// Encodes a JSON body.
pub fn json_encode<T: Serialize>(v: &T) -> Result<Vec<u8>, Error> {
    // Serialize into the reusable buffer of this thread and then copy out exactly.
    // That is one allocation per body and not one per doubling of a growing `Vec`.
    JSON_BUF.with(|buf| match buf.try_borrow_mut() {
        Ok(mut b) => {
            b.clear();
            crate::json::to_writer(&mut b, v).map_err(Error::Encode)?;
            // A large body leaves with the buffer itself (no copy); a small one
            // is copied out so the warm buffer stays for the next response.
            if b.len() > KEEP_BUFFER {
                return Ok(std::mem::take(&mut *b));
            }
            Ok(b.to_vec())
        }
        // Re-entered (a `Serialize` impl encoding JSON itself): no buffer to share.
        Err(_) => serde_json::to_vec(&crate::finite::Finite(v)).map_err(|e| Error::Encode(e.to_string())),
    })
}

thread_local! {
    static JSON_BUF: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// An encoded body larger than this leaves with its buffer; a smaller one is copied out so the buffer stays warm.
const KEEP_BUFFER: usize = 1 << 16;

/// Decodes a JSON body of a response with this status.
pub fn json_decode<T: DeserializeOwned>(status: u16, body: &[u8]) -> Result<T, Error> {
    // An empty body decodes as `null` (valid for Option and unit types).
    let body = if body.iter().all(u8::is_ascii_whitespace) { b"null".as_slice() } else { body };
    // Tracking the pointer allocates per key. That doubles the cost of a large body. So only a body that fails
    // to decode is read again with tracking to say where.
    if let Some(value) = crate::json::from_slice(body) {
        return Ok(value);
    }
    let mut de = serde_json::Deserializer::from_slice(body);
    let value = crate::path::deserialize(crate::integral::Ints(&mut de))
        .and_then(|v| de.end().map(|()| v).map_err(|e| (String::new(), e)));
    value.map_err(|(pointer, e)| Error::Decode {
        status,
        message: e.to_string(),
        body: bytes::Bytes::copy_from_slice(body),
        pointer,
    })
}

/// Decodes a `const`: only `want` or a string that spells a number or bool `want`.
/// Errors: any other value.
pub fn expect_const<'de, D: Deserializer<'de>>(d: D, want: &Value) -> Result<(), D::Error> {
    let got = Value::deserialize(d)?;
    let spelt = match (&got, want) {
        (Value::String(s), Value::Bool(b)) => s == if *b { "true" } else { "false" },
        (Value::String(s), Value::Number(n)) => {
            s.trim() == s && serde_json::from_str::<serde_json::Number>(s).is_ok_and(|x| x == *n)
        }
        _ => false,
    };
    if crate::validate::json_equal(&got, want) || spelt {
        Ok(())
    } else {
        Err(de::Error::custom(format!("expected {want}, found {got}")))
    }
}

/// Returns a value carried as JSON text inside a string (`contentMediaType: application/json`).
/// Errors: the text is not JSON of type `T`.
pub fn json_text<T: DeserializeOwned>(text: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(text)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    /// A `const` of a number takes the number or text that spells it. Other text is refused.
    #[test]
    fn a_number_const_takes_its_spelling() {
        assert!(super::expect_const(Value::from("5"), &json!(5)).is_ok());
        assert!(super::expect_const(Value::from("6"), &json!(5)).is_err());
        assert!(super::expect_const(Value::from(" 5"), &json!(5)).is_err());
    }
}
