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
    if !crate::json::mentions(&doc.root, "$dynamicRef") {
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
            } else if let Some(bindings) = scopes.get(ptr) {
                let mut binder = Binder::new(doc, bindings, ptr, ptr);
                if let Value::Object(mut bound) = binder.bind(&Value::Object(o.clone()), true) {
                    binder.hoist(&mut bound);
                    if bound != *o {
                        *o = bound;
                        *changed = true;
                    }
                }
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
    let mut binder = Binder::new(doc, &bindings, &target_ptr, ptr);
    let Value::Object(mut body) = binder.bind(target, false) else { return None };
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
        binder.hoist(&mut out);
        return Some(out);
    }
    for (k, x) in own {
        body.insert(k.clone(), x.clone());
    }
    binder.hoist(&mut body);
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

/// Binds the anchors of one site in the generics that it reaches. Each generic is copied once, bound, into the `$defs`
/// of the site, and every reference to it points at that copy: the bindings are the same throughout the site, so one
/// copy serves them all, a recursion ends at its own copy, and no depth cuts it.
struct Binder<'d> {
    doc: &'d Doc,
    bindings: &'d BTreeMap<String, String>,
    /// The `$defs` of the site, where the copies go.
    defs: String,
    /// The pointer of the copy of each generic, by the generic's pointer. The site is the copy of the generic it
    /// `$ref`s.
    copies: BTreeMap<String, String>,
    /// The copies made, by their name in the site's `$defs`.
    made: Vec<(String, Value)>,
}

impl<'d> Binder<'d> {
    /// Returns a binder of `bindings` for the site at `site`, which stands for the generic at `generic`.
    fn new(doc: &'d Doc, bindings: &'d BTreeMap<String, String>, generic: &str, site: &str) -> Self {
        let copies = BTreeMap::from([(generic.to_owned(), site.to_owned())]);
        Self { doc, bindings, defs: join(site, "$defs"), copies, made: vec![] }
    }

    /// Returns `v` with `$dynamicRef`s to bound anchors replaced, and each `$ref` into a generic that uses them pointed
    /// at its bound copy. With `own_scope`, a schema resource nested in it (an `$id`) is kept as it is: only a
    /// reference brings it into the dynamic scope.
    fn bind(&mut self, v: &Value, own_scope: bool) -> Value {
        match v {
            Value::Object(o) => {
                if let Some(name) = o.get("$dynamicRef").and_then(Value::as_str).and_then(|r| anchor_name(self.doc, r))
                    && let Some(to) = self.bindings.get(&name)
                {
                    let mut rest = o.clone();
                    rest.remove("$dynamicRef");
                    rest.insert("$ref".into(), Value::String(format!("#{to}")));
                    return Value::Object(rest);
                }
                if let Some(copy) = o.get("$ref").and_then(Value::as_str).and_then(|r| self.copy_of(r)) {
                    let mut rest = o.clone();
                    rest.insert("$ref".into(), Value::String(format!("#{copy}")));
                    return Value::Object(rest);
                }
                Value::Object(
                    o.iter()
                        .map(|(k, x)| {
                            let kept = own_scope && x.get("$id").is_some();
                            (k.clone(), if kept { x.clone() } else { self.bind(x, own_scope) })
                        })
                        .collect(),
                )
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.bind(x, own_scope)).collect()),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => v.clone(),
        }
    }

    /// Returns the pointer of the bound copy of the generic that `$ref` `r` reaches, made on first use. `None` when the
    /// target uses no bound anchor: the reference stays as it is.
    fn copy_of(&mut self, r: &str) -> Option<String> {
        let p = self.doc.local_pointer(r)?;
        if let Some(copy) = self.copies.get(&p) {
            return Some(copy.clone());
        }
        let target = pointer(&self.doc.root, &p)?;
        if !uses_any(self.doc, target, self.bindings, &mut self.copies.keys().cloned().collect()) {
            return None;
        }
        // Named after the site and the generic: `SiteTree`. The type of the copy takes that name.
        let last = |p: &str| crate::json::unescape_token(p.rsplit('/').next().unwrap_or_default());
        let site = last(self.defs.trim_end_matches("/$defs"));
        let mut name = format!("{site}{}", last(&p));
        let taken = |n: &str| pointer(&self.doc.root, &join(&self.defs, n)).is_some();
        while self.made.iter().any(|(n, _)| *n == name) || taken(&name) {
            name.push('_');
        }
        let copy = join(&self.defs, &name);
        // Reserved before the body is bound: a recursion back into it points here.
        self.copies.insert(p, copy.clone());
        let i = self.made.len();
        self.made.push((name, Value::Null));
        let mut body = self.bind(target, false);
        // The generic's own `$id` would make the copy a second resource of that name.
        if let Value::Object(b) = &mut body {
            b.remove("$id");
        }
        self.made[i].1 = body;
        Some(copy)
    }

    /// Puts the copies made into the `$defs` of the site `o`.
    fn hoist(self, o: &mut Map<String, Value>) {
        if self.made.is_empty() {
            return;
        }
        let defs = o.entry("$defs").or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(defs) = defs {
            defs.extend(self.made);
        }
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
