//! `application/linkset` (RFC 9264): the links of a `Link` header modelled as the `application/linkset+json`
//! data (3.2 "Modeling Link Headers"). `{"linkset": [{"anchor": A, "next": [{"href": H, "type": T}]}]}` is
//! `<H>; rel="next"; anchor="A"; type="T"`.

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::ClientError;
use crate::json::to_value;
use crate::text::decode_err;

/// Target attributes RFC 9264 section 4.2.4.2 models as a list even when one is given.
const LIST_ATTRIBUTES: [&str; 1] = ["hreflang"];

/// Encodes a linkset (the `application/linkset+json` model) as `Link` header text.
/// Errors: the value does not serialize.
pub fn linkset_encode<T: Serialize>(v: &T) -> Result<String, ClientError> {
    let value = to_value(v)?;
    let mut links = vec![];
    for context in value.get("linkset").and_then(Value::as_array).into_iter().flatten() {
        let Some(context) = context.as_object() else { continue };
        let anchor = context.get("anchor").and_then(Value::as_str);
        for (rel, targets) in context.iter().filter(|(k, _)| *k != "anchor") {
            for target in targets.as_array().into_iter().flatten().filter_map(Value::as_object) {
                let Some(href) = target.get("href").and_then(Value::as_str) else { continue };
                let mut link = format!("<{href}>; rel=\"{}\"", quoted(rel));
                if let Some(a) = anchor {
                    push_param(&mut link, "anchor", a);
                }
                for (k, v) in target.iter().filter(|(k, _)| *k != "href") {
                    let values = match v {
                        Value::Array(vs) => vs.iter().filter_map(Value::as_str).collect(),
                        Value::String(s) => vec![s.as_str()],
                        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => vec![],
                    };
                    for one in values {
                        push_param(&mut link, k, one);
                    }
                }
                links.push(link);
            }
        }
    }
    Ok(links.join(", "))
}

/// Decodes `Link` header text into a linkset (the `application/linkset+json` model). There is one link context per
/// anchor in order. Each relation is a list of targets.
/// Errors: the links do not fit `T`.
pub fn linkset_decode<T: DeserializeOwned>(status: Option<u16>, text: &str) -> Result<T, ClientError> {
    let mut contexts: Vec<(Option<String>, Map<String, Value>)> = vec![];
    for link in split_outside_quotes(text, ',') {
        let link = link.trim();
        let Some(rest) = link.strip_prefix('<') else { continue };
        let Some((href, params)) = rest.split_once('>') else { continue };
        let mut rels = vec![];
        let mut anchor = None;
        let mut target = Map::new();
        target.insert("href".into(), Value::String(href.to_owned()));
        for param in split_outside_quotes(params, ';').into_iter().map(str::trim).filter(|p| !p.is_empty()) {
            let (k, v) = param.split_once('=').unwrap_or((param, ""));
            let (k, v) = (k.trim().to_ascii_lowercase(), unquoted(v.trim()));
            match k.as_str() {
                "rel" => rels.extend(v.split_ascii_whitespace().map(str::to_owned)),
                "anchor" => anchor = Some(v),
                _ => push_attribute(&mut target, k, v),
            }
        }
        let i = contexts.iter().position(|(a, _)| *a == anchor).unwrap_or_else(|| {
            let mut c = Map::new();
            if let Some(a) = &anchor {
                c.insert("anchor".into(), Value::String(a.clone()));
            }
            contexts.push((anchor.clone(), c));
            contexts.len() - 1
        });
        for rel in rels {
            if let Value::Array(targets) = contexts[i].1.entry(rel).or_insert_with(|| Value::Array(vec![])) {
                targets.push(Value::Object(target.clone()));
            }
        }
    }
    let linkset: Vec<Value> = contexts.into_iter().map(|(_, c)| Value::Object(c)).collect();
    let value = serde_json::json!({ "linkset": linkset });
    serde_json::from_value(value).map_err(|e| decode_err(status, text.as_bytes(), e))
}

/// Returns a header declared with `application/linkset` `content` (a `Link` header). Its linkset with every line
/// joined.
pub fn linkset_header(raw: Option<&str>) -> Option<Value> {
    raw.and_then(|r| linkset_decode::<Value>(None, r).ok())
}

/// Returns a header declared with `application/linkset` `content`: the links as `Link` header text.
pub fn encode_linkset_header(v: &Value) -> Option<String> {
    if v.is_null() { None } else { linkset_encode(v).ok().filter(|s| !s.is_empty()) }
}

/// Appends a target attribute. It is a string or a list where RFC 9264 models a list or the attribute repeats.
fn push_attribute(target: &mut Map<String, Value>, k: String, v: String) {
    match target.get_mut(&k) {
        Some(Value::Array(vs)) => vs.push(Value::String(v)),
        Some(one) => {
            let first = std::mem::take(one);
            *one = Value::Array(vec![first, Value::String(v)]);
        }
        None if LIST_ATTRIBUTES.contains(&k.as_str()) => {
            target.insert(k, Value::Array(vec![Value::String(v)]));
        }
        None => {
            target.insert(k, Value::String(v));
        }
    }
}

/// Returns `s` cut at each `sep` outside a quoted string and outside `<...>`.
fn split_outside_quotes(s: &str, sep: char) -> Vec<&str> {
    let (mut out, mut start, mut quote, mut angle, mut escaped) = (vec![], 0, false, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quote => escaped = true,
            '"' if !angle => quote = !quote,
            '<' if !quote => angle = true,
            // Inside a quoted string `angle` is false already.
            '>' => angle = false,
            c if c == sep && !quote && !angle => {
                out.push(s.get(start..i).unwrap_or_default());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(s.get(start..).unwrap_or_default());
    out
}

/// Returns a parameter value without its quotes and escapes.
fn unquoted(v: &str) -> String {
    let Some(inner) = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else { return v.to_owned() };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' { chars.next().unwrap_or('\\') } else { c });
    }
    out
}

/// Appends `; key="value"` to a link.
fn push_param(link: &mut String, key: &str, value: &str) {
    for piece in ["; ", key, "=\"", &quoted(value), "\""] {
        link.push_str(piece);
    }
}

/// Returns a value inside a quoted string: `"` and `\` escaped.
fn quoted(v: &str) -> String {
    v.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 3.2 "Modeling Link Headers": a pagination linkset as the `Link` header carries it.
    #[test]
    fn a_pagination_linkset_round_trips_through_the_header() {
        let model = json!({"linkset": [{"anchor": "https://example.com/items",
            "first": [{"href": "/items?page=1"}], "prev": [{"href": "/items?page=1"}],
            "next": [{"href": "/items?page=3", "type": "application/json"}], "last": [{"href": "/items?page=9"}]}]});
        let header = linkset_encode(&model).unwrap();
        assert!(
            header.starts_with("</items?page=1>; rel=\"first\"; anchor=\"https://example.com/items\", "),
            "{header}"
        );
        assert!(header.contains(
            "</items?page=3>; rel=\"next\"; anchor=\"https://example.com/items\"; type=\"application/json\""
        ));
        assert_eq!(linkset_decode::<Value>(Some(200), &header).unwrap(), model);
    }

    #[test]
    fn header_syntax_variants_read_the_same() {
        let text =
            "<https://a.example/b,c>; rel=\"next prev\" , </x>;rel=up;title=\"a, \\\"b\\\"\";hreflang=en;hreflang=de";
        let v: Value = linkset_decode(Some(200), text).unwrap();
        let ctx = &v["linkset"][0];
        assert_eq!(ctx["next"][0]["href"], "https://a.example/b,c", "a comma inside <...> is not a separator");
        assert_eq!(ctx["prev"], ctx["next"], "one link, two relations");
        assert_eq!(ctx["up"][0], json!({"href": "/x", "title": "a, \"b\"", "hreflang": ["en", "de"]}));
    }

    /// A quote escaped inside a quoted string does not end it. `<` inside a quoted string opens no target.
    #[test]
    fn quotes_hide_separators() {
        let v: Value =
            linkset_decode(Some(200), r#"</b>; rel=next; title="<", </a>; rel=up; title="\"a, b\""; hreflang=en"#)
                .unwrap();
        let ctx = &v["linkset"][0];
        assert_eq!(ctx["up"][0], json!({"href": "/a", "title": "\"a, b\"", "hreflang": ["en"]}));
        assert_eq!(ctx["next"][0], json!({"href": "/b", "title": "<"}));
    }

    #[test]
    fn a_link_header_reads_and_writes_its_linkset() {
        let model = json!({"linkset": [{"anchor": "https://example.com/items", "next": [{"href": "/items?page=3"}]}]});
        let header = encode_linkset_header(&model).unwrap();
        assert_eq!(header, "</items?page=3>; rel=\"next\"; anchor=\"https://example.com/items\"");
        assert_eq!(linkset_header(Some(&header)), Some(model));
        assert_eq!(linkset_header(None), None);
        assert_eq!(encode_linkset_header(&Value::Null), None);
        assert_eq!(encode_linkset_header(&json!({"linkset": []})), None);
    }
}
