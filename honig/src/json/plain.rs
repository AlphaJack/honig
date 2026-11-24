//! `serde_json`.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Creates `body` decoded or `None`.
pub(crate) fn from_slice<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    let mut d = serde_json::Deserializer::from_slice(body);
    let v = T::deserialize(crate::integral::Ints(&mut d)).ok()?;
    d.end().ok().map(|()| v)
}

/// Returns `v` appended to `out`.
pub(crate) fn to_writer<T: Serialize>(out: &mut Vec<u8>, v: &T) -> Result<(), String> {
    serde_json::to_writer(out, &crate::finite::Finite(v)).map_err(|e| e.to_string())
}
