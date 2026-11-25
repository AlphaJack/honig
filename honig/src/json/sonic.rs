//! `sonic-rs`: SIMD JSON that decodes without a copy of the input.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Creates `body` decoded or `None`.
pub(crate) fn from_slice<T: DeserializeOwned>(body: &[u8]) -> Option<T> {
    let mut d = sonic_rs::Deserializer::from_slice(body);
    let v = T::deserialize(crate::integral::Ints(&mut d)).ok()?;
    d.end().ok().map(|()| v)
}

/// Returns `v` appended to `out`.
pub(crate) fn to_writer<T: Serialize>(out: &mut Vec<u8>, v: &T) -> Result<(), String> {
    sonic_rs::to_writer(out, &crate::finite::Finite(v)).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::Value;

    /// The deepest nesting `sonic-rs` decodes. One level more is an error.
    const DEPTH: usize = 254;

    /// A schema that refers to itself, as generated code has it: a struct that contains itself in an object and in a
    /// list.
    #[derive(Deserialize)]
    #[expect(dead_code, reason = "decoded for its depth")]
    struct Node {
        #[serde(default)]
        child: Option<Box<Self>>,
        #[serde(default)]
        children: Vec<Self>,
        #[serde(default)]
        extra: Option<Value>,
    }

    #[test]
    fn decodes_the_deepest_nesting_on_a_worker_sized_stack() {
        let objects = format!("{}{{}}{}", r#"{"child":"#.repeat(DEPTH - 1), "}".repeat(DEPTH - 1));
        let lists = format!("{}{{}}{}", r#"{"children":["#.repeat(DEPTH / 2 - 1), "]}".repeat(DEPTH / 2 - 1));
        let values = format!(r#"{{"extra":{}0{}}}"#, "[".repeat(DEPTH - 1), "]".repeat(DEPTH - 1));
        let deeper = format!("{}0{}", "[".repeat(DEPTH + 1), "]".repeat(DEPTH + 1));
        std::thread::Builder::new()
            .stack_size(2 << 20)
            .spawn(move || {
                for (shape, body) in [("objects", objects), ("lists", lists), ("values", values)] {
                    assert!(super::from_slice::<Node>(body.as_bytes()).is_some(), "{shape}");
                }
                assert!(super::from_slice::<Value>(deeper.as_bytes()).is_none());
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
