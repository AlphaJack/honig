//! Monomorphized `$dynamicRef` generics (JSON Schema 2020-12 dynamic scope).
//!
//! A schema that `$ref`s a generic and binds its `$dynamicAnchor`s in its own
//! `$defs` becomes a specialized copy of the generic. Each `$dynamicRef` is
//! replaced by a `$ref` to the binding.
//!
//! ```yaml
//! Page:        {type: object, properties: {items: {type: array, items: {$dynamicRef: '#item'}}},
//!              $defs: {item: {$dynamicAnchor: item}}}            # default: anything
//! ProductPage: {$ref: Page, $defs: {item: {$dynamicAnchor: item, $ref: Product}}}
//! ```
//!
//! makes `ProductPage` a struct with `items: Vec<Product>`. Recursion back into the
//! generic (`Tree<T>`'s children) points at the specialized copy itself.
//! Unbound dynamic references keep resolving to the generic's own anchor.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use crate::json::{Doc, join, pointer};

/// Returns the document with every bound generic specialized. `None` when it has
/// no `$dynamicRef` to specialize.
pub(crate) fn monomorphize(doc: &Doc) -> Option<Doc> {
    if !mentions(&doc.root, "$dynamicRef") {
        return None;
    }
    let mut root = doc.root.clone();
    let mut changed = false;
    let scopes = scopes(doc);
    rewrite(doc, &scopes, &mut root, "", &mut changed);
    changed.then(|| {
        let mut special = Doc::new(root);
        special.externals.clone_from(&doc.externals);
        special
    })
}

/// Returns the `$dynamicAnchor`s that each schema resource with an `$id` declares, by the pointer of its root: the
/// name of each anchor to the pointer of the schema that declares it. The document itself is left out: its components
/// often declare one name several times.
fn scopes(doc: &Doc) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for ((resource, name), at) in crate::schema_ids::dynamic(&doc.root).anchors {
        if !resource.is_empty() {
            out.entry(resource).or_default().insert(name, at);
        }
    }
    out
}

/// Specializes every schema object under `v` (at `ptr`) that binds anchors: a site that `$ref`s a generic beside its
/// own anchors, and the root of a resource that declares anchors. A `$dynamicRef` resolves to the outermost resource
/// in the dynamic scope that declares its anchor (JSON Schema 2020-12 section 8.2.3.2): from that root, each generic
/// that it reaches is copied with the root's anchors bound. Its original keeps its own, for every other way in.
fn rewrite(
    doc: &Doc,
    scopes: &BTreeMap<String, BTreeMap<String, String>>,
    v: &mut Value,
    ptr: &str,
    changed: &mut bool,
) {
    match v {
        Value::Object(o) => {
            if let Some(special) = specialize_site(doc, o, ptr) {
                *o = special;
                *changed = true;
            } else if let Some(bindings) = scopes.get(ptr)
                && let Value::Object(bound) =
                    bind_in(doc, &Value::Object(o.clone()), bindings, &mut vec![(ptr.to_owned(), ptr.to_owned())], true)
                && bound != *o
            {
                *o = bound;
                *changed = true;
            }
            for (k, c) in o.iter_mut() {
                rewrite(doc, scopes, c, &join(ptr, k), changed);
            }
        }
        Value::Array(a) => {
            for (i, c) in a.iter_mut().enumerate() {
                rewrite(doc, scopes, c, &format!("{ptr}/{i}"), changed);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// Returns `o` = `{$ref: generic, $defs: {.. $dynamicAnchor ..}}` becomes the body of the generic
/// with dynamic references bound plus the own keywords of `o`. A `$dynamicAnchor` of `o` itself binds its name to `o`:
/// `{$dynamicAnchor: node, $ref: tree}` makes the tree's nodes copies of `o`.
fn specialize_site(doc: &Doc, o: &Map<String, Value>, ptr: &str) -> Option<Map<String, Value>> {
    let r = o.get("$ref")?.as_str()?;
    let mut bindings: BTreeMap<String, String> = (o.get("$defs").and_then(Value::as_object).into_iter().flatten())
        .filter_map(|(k, d)| Some((d.get("$dynamicAnchor")?.as_str()?.to_owned(), join(&join(ptr, "$defs"), k))))
        .collect();
    if let Some(own) = o.get("$dynamicAnchor").and_then(Value::as_str) {
        bindings.insert(own.to_owned(), ptr.to_owned());
    }
    if bindings.is_empty() {
        return None;
    }
    let target_ptr = doc.local_pointer(r)?;
    let target = pointer(&doc.root, &target_ptr)?;
    if !uses_any(doc, target, &bindings, &mut BTreeSet::from([target_ptr.clone()])) {
        return None;
    }
    let mut chain = vec![(target_ptr, ptr.to_owned())];
    let Value::Object(body) = bind(doc, target, &bindings, &mut chain) else { return None };
    let mut body = body;
    // The generic's own `$id` would make the copy a second resource of that name. The site's own `$id` stays: the
    // site is still its resource, and its `$dynamicAnchor`s scope there.
    body.remove("$id");
    let own = o.iter().filter(|(k, _)| *k != "$ref");
    // Both apply. A keyword that both hold (`properties`) keeps each: the generic's becomes a member of `allOf`.
    // `$defs` and annotations apply to no value: the site's stand.
    let applies = |k: &str| {
        !matches!(k, "$defs" | "definitions" | "$dynamicAnchor" | "$anchor" | "$comment" | "$schema" | "description")
            && !matches!(k, "title" | "examples" | "example" | "deprecated")
            && !k.starts_with("x-")
    };
    if own.clone().any(|(k, x)| applies(k) && body.get(k).is_some_and(|b| b != x)) {
        let mut out: Map<String, Value> = own.map(|(k, x)| (k.clone(), x.clone())).collect();
        let mut members = vec![Value::Object(body)];
        members.extend(out.remove("allOf").and_then(|a| a.as_array().cloned()).into_iter().flatten());
        out.insert("allOf".into(), Value::Array(members));
        return Some(out);
    }
    for (k, x) in own {
        body.insert(k.clone(), x.clone());
    }
    Some(body)
}

/// Returns the anchor that `$dynamicRef` `r` names: `#item`, or the `$dynamicAnchor` of the schema at the pointer that
/// the `$id` pass writes for a bookended reference (`#/components/schemas/Page/$defs/item`).
fn anchor_name(doc: &Doc, r: &str) -> Option<String> {
    if !r.starts_with("#/") {
        return r.strip_prefix('#').map(String::from);
    }
    let target = pointer(&doc.root, &doc.local_pointer(r)?)?;
    target.get("$dynamicAnchor").and_then(Value::as_str).map(String::from)
}

/// Generics expanded inside one another before the rest stays unbound.
const MAX_BINDING_CHAIN: usize = 16;

/// Returns `v` with `$dynamicRef`s to bound anchors replaced. `$ref`s into other
/// generics that use them are inlined (and specialized too). `chain` holds
/// (generic pointer, specialized pointer) pairs being expanded. A `$ref` back
/// into one of them points at its specialized copy.
fn bind(doc: &Doc, v: &Value, bindings: &BTreeMap<String, String>, chain: &mut Vec<(String, String)>) -> Value {
    bind_in(doc, v, bindings, chain, false)
}

/// Returns [`bind`] of `v`. With `own_scope`, a schema resource nested in it (an `$id`) is kept as it is: only a
/// reference brings it into the dynamic scope.
fn bind_in(
    doc: &Doc,
    v: &Value,
    bindings: &BTreeMap<String, String>,
    chain: &mut Vec<(String, String)>,
    own_scope: bool,
) -> Value {
    let child = |x: &Value, chain: &mut Vec<(String, String)>| {
        if own_scope && x.get("$id").is_some() { x.clone() } else { bind_in(doc, x, bindings, chain, own_scope) }
    };
    match v {
        Value::Object(o) => {
            if let Some(name) = o.get("$dynamicRef").and_then(Value::as_str).and_then(|r| anchor_name(doc, r))
                && let Some(to) = bindings.get(&name)
            {
                let mut rest = o.clone();
                rest.remove("$dynamicRef");
                rest.insert("$ref".into(), Value::String(format!("#{to}")));
                return Value::Object(rest);
            }
            if let Some(r) = o.get("$ref").and_then(Value::as_str)
                && let Some(p) = doc.local_pointer(r)
            {
                if let Some((_, special)) = chain.iter().find(|(g, _)| *g == p) {
                    let mut rest = o.clone();
                    rest.insert("$ref".into(), Value::String(format!("#{special}")));
                    return Value::Object(rest);
                }
                if let Some(target) = pointer(&doc.root, &p)
                    && chain.len() < MAX_BINDING_CHAIN
                    && uses_any(doc, target, bindings, &mut chain.iter().map(|(g, _)| g.clone()).collect())
                {
                    // Another generic in the same dynamic scope: inline its
                    // specialization (its recursion then points back here).
                    chain.push((p, String::new()));
                    let inner = bind_in(doc, target, bindings, chain, own_scope);
                    chain.pop();
                    return inner;
                }
            }
            Value::Object(o.iter().map(|(k, x)| (k.clone(), child(x, chain))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| child(x, chain)).collect()),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => v.clone(),
    }
}

/// Does `v` (following `$ref`s once each) use a `$dynamicRef` to a bound anchor?
fn uses_any(doc: &Doc, v: &Value, bindings: &BTreeMap<String, String>, seen: &mut BTreeSet<String>) -> bool {
    match v {
        Value::Object(o) => {
            if o.get("$dynamicRef")
                .and_then(Value::as_str)
                .and_then(|r| anchor_name(doc, r))
                .is_some_and(|n| bindings.contains_key(&n))
            {
                return true;
            }
            if let Some(p) = o.get("$ref").and_then(Value::as_str).and_then(|r| doc.local_pointer(r))
                && seen.insert(p.clone())
                && pointer(&doc.root, &p).is_some_and(|t| uses_any(doc, t, bindings, seen))
            {
                return true;
            }
            o.values().any(|x| uses_any(doc, x, bindings, seen))
        }
        Value::Array(a) => a.iter().any(|x| uses_any(doc, x, bindings, seen)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn mentions(v: &Value, key: &str) -> bool {
    match v {
        Value::Object(o) => o.contains_key(key) || o.values().any(|x| mentions(x, key)),
        Value::Array(a) => a.iter().any(|x| mentions(x, key)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}
