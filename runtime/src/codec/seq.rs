//! JSON Lines and JSON Text Sequences: one JSON value per record.

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::json_decode;
use crate::ClientError;

/// The byte before each record of a JSON Text Sequence (RFC 7464).
pub(crate) const RECORD_SEPARATOR: u8 = 0x1e;

/// Returns JSON Lines: one JSON value per line.
pub fn jsonl_encode<T: Serialize>(items: &[T]) -> Result<Vec<u8>, ClientError> {
    let mut out = vec![];
    for i in items {
        jsonl_write(&mut out, i)?;
    }
    Ok(out)
}

/// Appends one JSON Lines record to `out`.
pub(crate) fn jsonl_write<T: Serialize>(out: &mut Vec<u8>, item: &T) -> Result<(), ClientError> {
    crate::json::to_writer(out, item).map_err(ClientError::Encode)?;
    out.push(b'\n');
    Ok(())
}

/// Decodes a JSON Lines body.
pub fn jsonl_decode<T: DeserializeOwned>(status: Option<u16>, body: &[u8]) -> Result<Vec<T>, ClientError> {
    body.split(|b| *b == b'\n').filter(|r| !blank_record(r)).map(|r| json_record(status, r)).collect()
}

/// Returns JSON Text Sequences (RFC 7464): RS + JSON + LF.
pub fn json_seq_encode<T: Serialize>(items: &[T]) -> Result<Vec<u8>, ClientError> {
    let mut out = vec![];
    for i in items {
        json_seq_write(&mut out, i)?;
    }
    Ok(out)
}

/// Appends one JSON Text Sequence record to `out`.
pub(crate) fn json_seq_write<T: Serialize>(out: &mut Vec<u8>, item: &T) -> Result<(), ClientError> {
    out.push(RECORD_SEPARATOR);
    jsonl_write(out, item)
}

/// Decodes an RFC 7464 JSON text sequence.
pub fn json_seq_decode<T: DeserializeOwned>(status: Option<u16>, body: &[u8]) -> Result<Vec<T>, ClientError> {
    body.split(|b| *b == RECORD_SEPARATOR).filter(|r| !blank_record(r)).map(|r| json_record(status, r)).collect()
}

/// Returns whether a JSON Lines or `json-seq` record carries no item: whitespace and record separators only.
pub(crate) fn blank_record(record: &[u8]) -> bool {
    record.iter().all(|b| b.is_ascii_whitespace() || *b == RECORD_SEPARATOR)
}

/// Returns one JSON Lines or `json-seq` record without its record separator. It is decoded as a JSON body is (with
/// `simd` by sonic-rs).
pub(crate) fn json_record<T: DeserializeOwned>(status: Option<u16>, record: &[u8]) -> Result<T, ClientError> {
    json_decode(status, record.strip_prefix(&[RECORD_SEPARATOR]).unwrap_or(record))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_json_sequence_starts_each_record_with_rs() {
        assert_eq!(super::json_seq_encode(&[1, 2]).unwrap(), b"\x1e1\n\x1e2\n");
    }
}
