//! Text to `serde_json::Value`. JSON and YAML are both accepted.

use std::path::Path;

use serde_json::Value;

use crate::diag::Diagnostic;

/// Source format of a loaded document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format {
    /// JSON.
    Json,
    /// YAML.
    Yaml,
}

/// Detects the format from the first significant character. A JSON document
/// always starts with `{` or `[`. Everything else is treated as YAML. That is
/// a superset of JSON anyway.
pub(crate) fn detect(text: &str) -> Format {
    let first = text.trim_start_matches('\u{feff}').trim_start().chars().next();
    match first {
        Some('{' | '[') => Format::Json,
        _ => Format::Yaml,
    }
}

/// Parses JSON or YAML text.
pub(crate) fn parse_str(text: &str) -> Result<(Value, Format), Diagnostic> {
    let text = text.trim_start_matches('\u{feff}');
    match detect(text) {
        Format::Json => match serde_json::from_str::<Unique>(text) {
            Ok(Unique(v)) => Ok((v, Format::Json)),
            // Some ".json" files are really YAML-flavoured JSON (trailing commas,
            // comments). Fall back to the YAML parser before giving up.
            Err(json_err) => match parse_yaml(text) {
                Ok(v) => Ok((v, Format::Yaml)),
                Err(_) => Err(Diagnostic::error(
                    "",
                    format!("invalid JSON at line {} column {}: {json_err}", json_err.line(), json_err.column()),
                )),
            },
        },
        Format::Yaml => parse_yaml(text).map(|v| (v, Format::Yaml)),
    }
}

/// A JSON value whose objects have unique keys. `serde_json` keeps the last of two equal keys. The
/// specifications say "patterned fields MUST have unique names" (and the YAML parser refuses them). So a
/// repeated key is an error with its line and column.
struct Unique(Value);

impl<'de> serde::Deserialize<'de> for Unique {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(UniqueVisitor).map(Unique)
    }
}

struct UniqueVisitor;

impl<'de> serde::de::Visitor<'de> for UniqueVisitor {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }
    fn visit_bool<E: serde::de::Error>(self, b: bool) -> Result<Value, E> {
        Ok(Value::Bool(b))
    }
    fn visit_i64<E: serde::de::Error>(self, n: i64) -> Result<Value, E> {
        Ok(n.into())
    }
    fn visit_u64<E: serde::de::Error>(self, n: u64) -> Result<Value, E> {
        Ok(n.into())
    }
    fn visit_f64<E: serde::de::Error>(self, n: f64) -> Result<Value, E> {
        Ok(serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number))
    }
    fn visit_str<E: serde::de::Error>(self, s: &str) -> Result<Value, E> {
        Ok(Value::String(s.to_owned()))
    }
    fn visit_string<E: serde::de::Error>(self, s: String) -> Result<Value, E> {
        Ok(Value::String(s))
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut out = Vec::with_capacity(seq.size_hint().unwrap_or_default());
        while let Some(Unique(v)) = seq.next_element()? {
            out.push(v);
        }
        Ok(Value::Array(out))
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = serde_json::Map::new();
        while let Some(k) = map.next_key::<String>()? {
            let Unique(v) = map.next_value()?;
            if out.contains_key(&k) {
                return Err(serde::de::Error::custom(format!("duplicate key `{k}`")));
            }
            out.insert(k, v);
        }
        Ok(Value::Object(out))
    }
}

/// The YAML budget. It has room for large descriptions and none for a small document that aliases its way past it.
const MAX_NODES: usize = 20_000_000;
const MAX_EVENTS: usize = 40_000_000;
const MAX_SCALAR_BYTES: usize = 512 * 1024 * 1024;

fn parse_yaml(text: &str) -> Result<Value, Diagnostic> {
    // YAML 1.2 booleans only. `y`, `n`, `on` and `off` are strings (a
    // parameter named `y` must stay a string).
    // The default size budgets of serde-saphyr fit configs and not large
    // descriptions. Raised: nodes, events and scalar bytes. They grow with the input.
    // Kept: alias, anchor and depth budgets. They stop a small
    // input from expanding.
    let options = serde_saphyr::options! {
        strict_booleans: true,
        budget: serde_saphyr::budget! {
            max_nodes: MAX_NODES,
            max_events: MAX_EVENTS,
            max_total_scalar_bytes: MAX_SCALAR_BYTES,
        },
    };
    serde_saphyr::from_str_with_options::<Value>(text, options)
        .map_err(|e| Diagnostic::error("", format!("invalid YAML: {e}")))
}

/// Reads and parses a JSON or YAML file.
pub(crate) fn load_file(path: &Path) -> Result<(Value, Format), Diagnostic> {
    let text = crate::disk::read(path).map_err(|e| Diagnostic::error("", format!("cannot read {e}")))?;
    parse_str(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_and_yaml_give_same_value() {
        let (j, jf) = parse_str(r#"{"openapi":"3.2.0","info":{"title":"t","version":"1"}}"#).unwrap();
        let (y, yf) = parse_str("openapi: 3.2.0\ninfo:\n  title: t\n  version: '1'\n").unwrap();
        assert_eq!(jf, Format::Json);
        assert_eq!(yf, Format::Yaml);
        assert_eq!(j, y);
    }

    /// "Patterned fields MUST have unique names". JSON refuses a repeated key at any depth as YAML does.
    #[test]
    fn a_repeated_json_key_is_an_error() {
        let top = parse_str(r#"{"openapi":"3.2.0","info":{"title":"a"},"info":{"title":"b"}}"#).unwrap_err();
        assert!(top.message.contains("duplicate key `info` at line 1"), "{}", top.message);
        let deep = parse_str("{\"paths\": {\"/a\": {}, \n \"/a\": {}}}").unwrap_err();
        assert!(deep.message.contains("duplicate key `/a` at line 2"), "{}", deep.message);
        let (v, _) = parse_str(r#"{"a":[{"k":1},{"k":2}],"n":1.5,"z":null,"t":true}"#).unwrap();
        assert_eq!(
            v,
            json!({"a": [{"k": 1}, {"k": 2}], "n": 1.5, "z": null, "t": true}),
            "same keys in sibling objects"
        );
    }

    #[test]
    fn yaml_integer_keys_become_strings() {
        // Response maps are commonly written with bare integer status keys.
        let (v, _) = parse_str("responses:\n  200:\n    description: ok\n").unwrap();
        assert_eq!(v, json!({"responses": {"200": {"description": "ok"}}}));
    }

    #[test]
    fn yaml_keeps_key_order() {
        let (v, _) = parse_str("b: 1\na: 2\nc: 3\n").unwrap();
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["b", "a", "c"]);
    }

    #[test]
    fn yaml_1_1_booleans_are_strings() {
        let (v, _) = parse_str("a: y\nb: on\nc: true\n").unwrap();
        assert_eq!(v, json!({"a": "y", "b": "on", "c": true}));
    }

    #[test]
    fn yaml_alias_bomb_is_rejected() {
        let mut bomb = String::from("a0: &a0 [x,x,x,x,x,x,x,x,x,x]\n");
        for i in 1..12 {
            let p = i - 1;
            bomb.push_str(&format!("a{i}: &a{i} [*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p},*a{p}]\n"));
        }
        assert!(parse_str(&bomb).is_err());
    }

    #[test]
    fn yaml_past_the_default_node_budget_parses() {
        let yaml: String = (0..300_000).map(|i| format!("- {i}\n")).collect();
        assert!(parse_str(&yaml).is_ok());
    }
}
