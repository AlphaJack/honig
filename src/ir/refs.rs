//! References followed to the object they point at.

use serde_json::Value;

use super::types::Builder;
use crate::json::pointer;

/// `$ref`s followed to an object: a chain may loop.
pub(crate) const MAX_REF_HOPS: usize = 8;

/// Returns the object that `v` (at `ptr`) refers to and where it is.
pub(crate) fn resolve_value(b: &Builder<'_>, v: &Value, ptr: &str) -> Option<(Value, String)> {
    let mut v = v.clone();
    let mut p = ptr.to_owned();
    // 3.1+ Reference Object: its `summary` and `description` "SHOULD override that of the referenced
    // component". The one nearest the use wins along a chain of references.
    let mut overrides = serde_json::Map::new();
    for _ in 0..MAX_REF_HOPS {
        match v.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                for field in ["summary", "description"] {
                    if let Some(text) = v.get(field).filter(|t| t.is_string()) {
                        overrides.entry(field).or_insert_with(|| text.clone());
                    }
                }
                p = b.doc.local_pointer(r)?;
                v = pointer(&b.doc.root, &p)?.clone();
            }
            None => {
                if let Value::Object(o) = &mut v {
                    o.extend(overrides);
                }
                return Some((v, p));
            }
        }
    }
    None
}
