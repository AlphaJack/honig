//! Binary data in a structure and how its bytes cross a multipart conversion without base64.

use std::cell::RefCell;
#[cfg(feature = "base64")]
use std::fmt;

#[cfg(feature = "base64")]
use serde::de::{self, Deserializer, Visitor};
#[cfg(feature = "base64")]
use serde::{Deserialize, Serialize, Serializer};

#[cfg(feature = "base64")]
use crate::base64::{base64, base64url, unbase64};

/// Marks a JSON string that carries raw part bytes (base64) inside a
/// multipart encode or decode. Never leaves this module.
pub(crate) const RAW_MARK: &str = concat!("\u{0}", env!("CARGO_PKG_NAME"), "-raw:");

/// Serde name [`Coerce`](crate::coerce::Coerce) recognizes to hand [`Binary`] the marked string.
pub(crate) const BINARY: &str = concat!(env!("CARGO_PKG_NAME"), "::Binary");

thread_local! {
    /// The binary parts of a multipart body while it is converted. They travel through `serde_json::Value` as a
    /// short marker that names their place here. They do not travel as base64. That would cost two more passes over
    /// every byte and a third more memory. `None` outside a conversion.
    static RAW_PARTS: RefCell<Option<Vec<Binary>>> = const { RefCell::new(None) };
}

/// Runs `f` inside a multipart conversion. There is one for the whole of an encode or decode. So a part marked while
/// the body is split is still there when the value is deserialized. A conversion already open is joined.
#[cfg(feature = "multipart")]
pub(crate) fn raw_parts<R>(f: impl FnOnce() -> R) -> R {
    if RAW_PARTS.with(|c| c.borrow().is_some()) {
        return f();
    }
    RAW_PARTS.with(|c| *c.borrow_mut() = Some(vec![]));
    /// Closes the conversion also when `f` unwinds. A worker thread must not keep its parts.
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            RAW_PARTS.with(|c| *c.borrow_mut() = None);
        }
    }
    let _close = Close;
    f()
}

/// Returns whether a multipart conversion is open: [`Binary`] then travels as a marker.
#[cfg(feature = "base64")]
fn in_raw_parts() -> bool {
    RAW_PARTS.with(|c| c.borrow().is_some())
}

/// Returns a marker for raw part bytes inside a multipart conversion: their place in [`RAW_PARTS`].
#[cfg(feature = "multipart")]
pub(crate) fn raw_mark(bytes: &bytes::Bytes, filename: Option<&str>, content_type: Option<&str>) -> String {
    raw_store(Binary {
        // A slice of the body the part arrived in: no copy.
        bytes: bytes.clone(),
        filename: filename.map(str::to_owned),
        content_type: content_type.map(str::to_owned),
    })
}

#[cfg(feature = "base64")]
fn raw_store(part: Binary) -> String {
    RAW_PARTS.with(|c| match c.borrow_mut().as_mut() {
        Some(parts) => {
            parts.push(part);
            [RAW_MARK, itoa::Buffer::new().format(parts.len() - 1)].concat()
        }
        // Outside a conversion no marker can be read back: the bytes as text.
        None => String::from_utf8_lossy(&part.bytes).into_owned(),
    })
}

/// Returns the part that a [`raw_mark`] marker names. It is a clone that shares its bytes. An untagged union may read
/// the same value more than once.
pub(crate) fn raw_part(s: &str) -> Option<Binary> {
    let at: usize = s.strip_prefix(RAW_MARK)?.parse().ok()?;
    RAW_PARTS.with(|c| c.borrow().as_ref()?.get(at).cloned())
}

/// Binary data in a structure (`format: binary`, `contentEncoding: base64` or a string with a non-text
/// `contentMediaType`). Raw bytes in a multipart part. A base64 string in JSON.
///
/// A multipart file part also has a file name and a content type. They describe how the data travelled. So two
/// `Binary` are equal when their bytes are.
#[derive(Debug, Clone, Default)]
pub struct Binary {
    /// The data. It is a slice of the body that it arrived in. Clones share it without a copy.
    pub bytes: bytes::Bytes,
    /// The part's `filename`: the file's name on the client.
    pub filename: Option<String>,
    /// The part's `Content-Type`.
    pub content_type: Option<String>,
}

impl PartialEq for Binary {
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for Binary {}

impl std::hash::Hash for Binary {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.bytes.hash(state);
    }
}

impl PartialOrd for Binary {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Binary {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl From<Vec<u8>> for Binary {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes: bytes.into(), filename: None, content_type: None }
    }
}

impl From<bytes::Bytes> for Binary {
    fn from(bytes: bytes::Bytes) -> Self {
        Self { bytes, filename: None, content_type: None }
    }
}

/// The bytes alone: the file name and content type of a part go.
impl From<Binary> for bytes::Bytes {
    fn from(b: Binary) -> Self {
        b.bytes
    }
}

impl AsRef<[u8]> for Binary {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

#[cfg(feature = "base64")]
impl Binary {
    fn from_wire(s: &str) -> Result<Self, String> {
        if s.starts_with(RAW_MARK) {
            return raw_part(s).ok_or_else(|| "corrupt binary part".to_owned());
        }
        if in_raw_parts() {
            // A text part sent to a binary field: its bytes as they are.
            return Ok(Self::from(s.as_bytes().to_vec()));
        }
        unbase64(s.trim()).map(Self::from).ok_or_else(|| "binary value is not base64".to_owned())
    }
}

#[cfg(feature = "base64")]
impl Serialize for Binary {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if in_raw_parts() { s.serialize_str(&raw_store(self.clone())) } else { s.serialize_str(&base64(&self.bytes)) }
    }
}

#[cfg(feature = "base64")]
impl<'de> Deserialize<'de> for Binary {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Binary;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("base64 binary data")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Binary, E> {
                Binary::from_wire(s).map_err(E::custom)
            }
            fn visit_newtype_struct<D2: Deserializer<'de>>(self, d: D2) -> Result<Binary, D2::Error> {
                let s = <String as Deserialize>::deserialize(d)?;
                Binary::from_wire(&s).map_err(de::Error::custom)
            }
        }
        d.deserialize_newtype_struct(BINARY, V)
    }
}

#[cfg(feature = "base64")]
/// Bytes in a string with `contentEncoding: base64url`. It uses the URL-safe
/// alphabet (`-` and `_`) without padding. Decoding also accepts padding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Base64Url(pub Vec<u8>);

#[cfg(feature = "base64")]
impl From<Vec<u8>> for Base64Url {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

#[cfg(feature = "base64")]
impl From<Base64Url> for Vec<u8> {
    fn from(b: Base64Url) -> Self {
        b.0
    }
}

#[cfg(feature = "base64")]
impl AsRef<[u8]> for Base64Url {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(feature = "base64")]
impl Serialize for Base64Url {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64url(&self.0))
    }
}

#[cfg(feature = "base64")]
impl<'de> Deserialize<'de> for Base64Url {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = <String as Deserialize>::deserialize(d)?;
        let standard: String = s
            .trim()
            .chars()
            .map(|c| match c {
                '-' => '+',
                '_' => '/',
                other => other,
            })
            .collect();
        if s.contains(['+', '/']) {
            return Err(de::Error::custom("base64url value uses the standard alphabet"));
        }
        unbase64(&standard).map(Self).ok_or_else(|| de::Error::custom("value is not base64url"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_data_converts_to_and_from_plain_bytes() {
        let b = Binary::from(bytes::Bytes::from_static(b"abc"));
        assert_eq!(b.as_ref(), b"abc");
        assert_eq!(bytes::Bytes::from(b), bytes::Bytes::from_static(b"abc"));
        #[cfg(feature = "base64")]
        {
            let u = Base64Url::from(b"xy".to_vec());
            assert_eq!(u.as_ref(), b"xy");
            assert_eq!(Vec::<u8>::from(u), b"xy");
        }
    }
}
