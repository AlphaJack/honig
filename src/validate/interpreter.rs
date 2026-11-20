//! The JSON Schema interpreter that checks a value against a whole schema. Only the generator uses it: to check the
//! examples of a description against their schemas. Generated code checks values with typed checks of its own.

use std::cell::RefCell;
use std::collections::BTreeMap;

use runtime::__private::{InstancePath, Violation, check_format, json_equal};
use serde_json::{Map, Value};

/// Checks the JSON Schema keywords with no Rust type of their own against the serialized value.
/// Examples are `contains`, `if`/`then` and `dependentRequired`.
pub(crate) fn check_schema(out: &mut Vec<Violation>, path: &InstancePath<'_>, value: &Value, schema: &Value) {
    check_schema_in(out, path, value, schema, None);
}

/// [`check_schema`] with the definitions that every schema of a generated module shares.
///
/// A `#/$defs/<token>` reference resolves in the `$defs` of `schema` first, then in `shared`. Each shared definition is
/// in the generated code once, however many schemas reach it.
fn check_schema_in(
    out: &mut Vec<Violation>,
    path: &InstancePath<'_>,
    value: &Value,
    schema: &Value,
    shared: Option<&Map<String, Value>>,
) {
    let mut found = vec![];
    let defs = schema.get("$defs").and_then(Value::as_object);
    Schema { defs, shared, memo: RefCell::new(BTreeMap::new()) }.check(value, schema, &mut found);
    for (at, message) in found {
        let mut p = path.to_string();
        p.push_str(&at);
        out.push(Violation { path: p, message });
    }
}

/// What a subschema evaluated: property names (borrowed from the value) and
/// how many leading items (for `unevaluated*`).
#[derive(Default)]
struct Evaluated<'v> {
    props: Vec<&'v str>,
    items: usize,
    all_items: bool,
    /// Items that `contains` matched: evaluated too (2020-12 core 11.3.1), wherever they are.
    contained: Vec<usize>,
}

/// A schema being checked: the `$defs` of its root, which its cut cycles point into, and what each definition found
/// in each object and array already.
struct Schema<'s> {
    defs: Option<&'s Map<String, Value>>,
    /// The definitions that the schemas of a generated module share: looked up after `defs`.
    shared: Option<&'s Map<String, Value>>,
    /// By the addresses of a value and a definition: what it found, or `None` while it is being checked. A recursive
    /// `oneOf` checks each branch on the same nested values: without this, every level doubles the work. Only objects
    /// and arrays are kept. They sit in the checked value, whereas a string may be a name copied for a moment.
    memo: RefCell<BTreeMap<(usize, usize), Option<Found>>>,
}

/// What a definition found in a value: its violations and what it evaluated, owned.
#[derive(Clone)]
struct Found {
    violations: Vec<(String, String)>,
    props: Vec<String>,
    items: usize,
    all_items: bool,
    contained: Vec<usize>,
}

/// Does `s` match `pattern`? The generator holds the runtime with its `regress` feature.
fn matches(pattern: &str, s: &str) -> bool {
    runtime::__private::pattern_matches_dyn(pattern, s)
}

/// A keyword that the check reads: its discriminant is its bit in [`Keywords`].
#[derive(Clone, Copy)]
enum Keyword {
    Ref,
    AdditionalProperties,
    AllOf,
    AnyOf,
    Const,
    Contains,
    DependentRequired,
    DependentSchemas,
    Else,
    Enum,
    Format,
    If,
    Items,
    MaxContains,
    MaxItems,
    MaxLength,
    MaxProperties,
    MinContains,
    MinItems,
    MinLength,
    MinProperties,
    Not,
    OneOf,
    Pattern,
    PatternProperties,
    PrefixItems,
    Properties,
    PropertyNames,
    Required,
    Then,
    Type,
    UnevaluatedItems,
    UnevaluatedProperties,
    UniqueItems,
}

/// The bit of every keyword that the check does not read.
const OTHER_KEYWORDS: u32 = u64::BITS - 1;

/// The keywords of one schema object, with a bit for each keyword that it holds. A keyword that the check asks for
/// and the object does not hold costs a bit test, not a search of its map: most objects hold a few of the
/// 34 keywords that the check asks each one for.
struct Keywords<'s> {
    map: &'s Map<String, Value>,
    present: u64,
}

impl<'s> Keywords<'s> {
    fn new(map: &'s Map<String, Value>) -> Self {
        Self { map, present: map.keys().fold(0, |bits, k| bits | Self::bit(k)) }
    }

    /// Returns the bit of a keyword that the check reads. Every other name shares the last bit.
    fn bit(keyword: &str) -> u64 {
        let known = match keyword {
            "$ref" => Keyword::Ref,
            "additionalProperties" => Keyword::AdditionalProperties,
            "allOf" => Keyword::AllOf,
            "anyOf" => Keyword::AnyOf,
            "const" => Keyword::Const,
            "contains" => Keyword::Contains,
            "dependentRequired" => Keyword::DependentRequired,
            "dependentSchemas" => Keyword::DependentSchemas,
            "else" => Keyword::Else,
            "enum" => Keyword::Enum,
            "format" => Keyword::Format,
            "if" => Keyword::If,
            "items" => Keyword::Items,
            "maxContains" => Keyword::MaxContains,
            "maxItems" => Keyword::MaxItems,
            "maxLength" => Keyword::MaxLength,
            "maxProperties" => Keyword::MaxProperties,
            "minContains" => Keyword::MinContains,
            "minItems" => Keyword::MinItems,
            "minLength" => Keyword::MinLength,
            "minProperties" => Keyword::MinProperties,
            "not" => Keyword::Not,
            "oneOf" => Keyword::OneOf,
            "pattern" => Keyword::Pattern,
            "patternProperties" => Keyword::PatternProperties,
            "prefixItems" => Keyword::PrefixItems,
            "properties" => Keyword::Properties,
            "propertyNames" => Keyword::PropertyNames,
            "required" => Keyword::Required,
            "then" => Keyword::Then,
            "type" => Keyword::Type,
            "unevaluatedItems" => Keyword::UnevaluatedItems,
            "unevaluatedProperties" => Keyword::UnevaluatedProperties,
            "uniqueItems" => Keyword::UniqueItems,
            _ => return 1 << OTHER_KEYWORDS,
        };
        1 << known as u32
    }

    /// Returns the value of `keyword` when the object holds it.
    fn get(&self, keyword: &str) -> Option<&'s Value> {
        if self.present & Self::bit(keyword) == 0 { None } else { self.map.get(keyword) }
    }
}

/// Violations as (relative JSON pointer, message).
type Violations = Vec<(String, String)>;

/// Returns the violations of the branch that got furthest into `value`, when no branch of a `oneOf` or `anyOf`
/// matches. Furthest is the deepest violation at a place that `value` holds: a branch that failed on a property of the
/// value matched the value itself, whereas a missing required property is a failure of the value itself. A tie in depth
/// goes to the branch with fewer violations. `None` when every branch failed on the value itself, or two branches are
/// as close: then no branch is the one meant, and the union's own message says more.
fn closest(value: &Value, misses: Vec<Violations>) -> Option<Violations> {
    let reached = |at: &str| {
        let depth = at.matches('/').count();
        if value.pointer(at).is_some() { depth } else { depth.saturating_sub(1) }
    };
    let score = |found: &[(String, String)]| (found.iter().map(|(at, _)| reached(at)).max().unwrap_or(0), found.len());
    let mut best: Option<((usize, usize), Violations)> = None;
    let mut tied = false;
    for found in misses {
        let (depth, count) = score(&found);
        match &best {
            Some(((d, c), _)) if depth < *d || (depth == *d && count > *c) => {}
            Some(((d, c), _)) if depth == *d && count == *c => tied = true,
            _ => {
                tied = false;
                best = Some(((depth, count), found));
            }
        }
    }
    best.filter(|((depth, _), _)| *depth > 0 && !tied).map(|(_, found)| found)
}

/// Returns `/key` as a JSON pointer token.
fn pointer_key(k: &str) -> String {
    format!("/{}", k.replace('~', "~0").replace('/', "~1"))
}

impl Schema<'_> {
    /// Returns the definition that `reference` names (`#/$defs/<token>`): in the `$defs` of the root, else in the
    /// shared ones.
    fn def(&self, reference: &str) -> Option<&Value> {
        let token = crate::json::unescape_token(reference.strip_prefix("#/$defs/")?);
        self.defs.and_then(|d| d.get(&token)).or_else(|| self.shared?.get(&token))
    }

    /// Checks `value` against the definition `target` once per object or array. A definition met again on the same
    /// value before its check ends is a cycle that consumes nothing: it passes.
    fn check_def<'v>(&self, value: &'v Value, target: &Value, out: &mut Vec<(String, String)>) -> Evaluated<'v> {
        if !(value.is_object() || value.is_array()) {
            return self.check(value, target, out);
        }
        let key = (std::ptr::from_ref(value) as usize, std::ptr::from_ref(target) as usize);
        let known = self.memo.borrow().get(&key).cloned();
        let found = match known {
            Some(None) => return Evaluated::default(),
            Some(Some(found)) => found,
            None => {
                self.memo.borrow_mut().insert(key, None);
                let mut violations = vec![];
                let e = self.check(value, target, &mut violations);
                let found = Found {
                    violations,
                    props: e.props.iter().map(|p| (*p).to_owned()).collect(),
                    items: e.items,
                    all_items: e.all_items,
                    contained: e.contained,
                };
                self.memo.borrow_mut().insert(key, Some(found.clone()));
                found
            }
        };
        out.extend(found.violations);
        // The names borrowed from the value again.
        let props = match value {
            Value::Object(o) => {
                found.props.iter().filter_map(|p| o.get_key_value(p).map(|(k, _)| k.as_str())).collect()
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) | Value::Array(_) => vec![],
        };
        Evaluated { props, items: found.items, all_items: found.all_items, contained: found.contained }
    }

    /// Does `value` satisfy `schema`? (no messages)
    fn valid(&self, value: &Value, schema: &Value) -> bool {
        // `const`, `enum` and `type` alone (`contains: {const: vip}`) are
        // answered without a message per failing value.
        if let Value::Object(s) = schema
            && s.keys().all(|k| matches!(k.as_str(), "const" | "enum" | "type"))
        {
            let typed = match s.get("type") {
                Some(Value::String(t)) => type_is(value, t),
                Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).any(|t| type_is(value, t)),
                None | Some(Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_)) => true,
            };
            return typed
                && s.get("const").is_none_or(|c| json_equal(c, value))
                && s.get("enum").is_none_or(|e| e.as_array().is_none_or(|e| e.iter().any(|x| json_equal(x, value))));
        }
        let mut found = vec![];
        self.check(value, schema, &mut found);
        found.is_empty()
    }

    /// Is `name` an allowed property name? A schema of string keywords only
    /// (the usual `{pattern: ...}`) is checked on the name in place. The name
    /// is not copied into a `Value`.
    fn valid_name(&self, name: &str, schema: &Value) -> bool {
        let Value::Object(s) = schema else { return self.valid(&Value::String(name.to_owned()), schema) };
        let plain = crate::json::text_only(s);
        if !plain {
            return self.valid(&Value::String(name.to_owned()), schema);
        }
        let n = || u64::try_from(name.chars().count()).unwrap_or(u64::MAX);
        s.get("minLength").and_then(count).is_none_or(|m| n() >= m)
            && s.get("maxLength").and_then(count).is_none_or(|m| n() <= m)
            && s.get("pattern").and_then(Value::as_str).is_none_or(|p| matches(p, name))
    }

    /// Violations of `schema` by `value` as (relative JSON pointer, message).
    /// Returns what was evaluated for `unevaluated*` in a parent.
    #[expect(clippy::too_many_lines, reason = "one arm per keyword family reads best in one place")]
    fn check<'v>(&self, value: &'v Value, schema: &Value, out: &mut Vec<(String, String)>) -> Evaluated<'v> {
        let mut ev = Evaluated::default();
        let s = match schema {
            Value::Bool(false) => {
                out.push((String::new(), "no value is allowed here".into()));
                return ev;
            }
            Value::Object(s) => &Keywords::new(s),
            Value::Bool(true) | Value::Null | Value::Number(_) | Value::String(_) | Value::Array(_) => return ev,
        };
        let fail = |out: &mut Vec<(String, String)>, m: String| out.push((String::new(), m));
        if let Some(r) = s.get("$ref").and_then(Value::as_str) {
            match self.def(r) {
                Some(target) => {
                    let e = self.check_def(value, target, out);
                    merge(&mut ev, e);
                }
                // Passing would accept any value: the generator resolves every reference, so this is a bug to see.
                None => fail(out, format!("the reference `{r}` resolves to no schema")),
            }
        }
        if let Some(t) = s.get("type") {
            let ok = match t {
                Value::String(t) => type_is(value, t),
                Value::Array(ts) => ts.iter().filter_map(Value::as_str).any(|t| type_is(value, t)),
                Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => true,
            };
            if !ok {
                fail(out, format!("must be of type {t}"));
            }
        }
        if let Some(c) = s.get("const")
            && !json_equal(c, value)
        {
            fail(out, format!("must be {c}"));
        }
        if let Some(Value::Array(e)) = s.get("enum")
            && !e.iter().any(|x| json_equal(x, value))
        {
            fail(out, "is not one of the allowed values".into());
        }
        match value {
            Value::String(text) => {
                // As the typed checks do: a schema checked whole asserts its `format` too.
                if let Some(f) = s.get("format").and_then(Value::as_str) {
                    let mut found = vec![];
                    check_format(&mut found, &InstancePath::Root, text, f);
                    for v in found {
                        fail(out, v.message);
                    }
                }
                let n = u64::try_from(text.chars().count()).unwrap_or(u64::MAX);
                if let Some(m) = s.get("minLength").and_then(count).filter(|m| n < *m) {
                    fail(out, format!("at least {m} characters required, found {n}"));
                }
                if let Some(m) = s.get("maxLength").and_then(count).filter(|m| n > *m) {
                    fail(out, format!("at most {m} characters allowed, found {n}"));
                }
                // A value past `maxLength` is wrong already. Its pattern does not run on it: a pattern that backtracks
                // takes exponential time on a long value that almost matches.
                if let Some(p) = s.get("pattern").and_then(Value::as_str)
                    && s.get("maxLength").and_then(count).is_none_or(|m| n <= m)
                    && !matches(p, text)
                {
                    fail(out, format!("does not match `{p}`"));
                }
            }
            Value::Number(num) => {
                if let Some(x) = num.as_f64() {
                    let bound = |k: &str| s.get(k).and_then(Value::as_f64);
                    if let Some(m) = bound("minimum").filter(|m| x < *m) {
                        fail(out, format!("{num} is less than the minimum {m}"));
                    }
                    if let Some(m) = bound("maximum").filter(|m| x > *m) {
                        fail(out, format!("{num} is greater than the maximum {m}"));
                    }
                    if let Some(m) = bound("exclusiveMinimum").filter(|m| x <= *m) {
                        fail(out, format!("{num} must be greater than {m}"));
                    }
                    if let Some(m) = bound("exclusiveMaximum").filter(|m| x >= *m) {
                        fail(out, format!("{num} must be less than {m}"));
                    }
                    if let Some(m) = s.get("multipleOf").and_then(Value::as_number).filter(|m| m.as_f64() > Some(0.0))
                        && runtime::__private::is_multiple(&num.to_string(), &m.to_string()) == Some(false)
                    {
                        fail(out, format!("is not a multiple of {m}"));
                    }
                }
            }
            Value::Array(items) => self.check_array(items, s, out, &mut ev),
            Value::Object(o) => self.check_object(value, o, s, out, &mut ev),
            Value::Null | Value::Bool(_) => {}
        }
        // Applicators: their evaluations count for `unevaluated*` here.
        if let Some(Value::Array(all)) = s.get("allOf") {
            for sub in all {
                let e = self.check(value, sub, out);
                merge(&mut ev, e);
            }
        }
        if let Some(Value::Array(any)) = s.get("anyOf") {
            let mut hit = false;
            let mut misses = vec![];
            for sub in any {
                let mut tmp = vec![];
                let e = self.check(value, sub, &mut tmp);
                if tmp.is_empty() {
                    hit = true;
                    merge(&mut ev, e);
                } else {
                    misses.push(tmp);
                }
            }
            if !hit {
                match closest(value, misses) {
                    Some(found) => out.extend(found),
                    None => fail(out, "matches none of `anyOf`".into()),
                }
            }
        }
        if let Some(Value::Array(one)) = s.get("oneOf") {
            let mut hits = 0;
            let mut misses = vec![];
            for sub in one {
                let mut tmp = vec![];
                let e = self.check(value, sub, &mut tmp);
                if tmp.is_empty() {
                    hits += 1;
                    merge(&mut ev, e);
                } else {
                    misses.push(tmp);
                }
            }
            match (hits, closest(value, misses)) {
                (1, _) => {}
                (0, Some(found)) => out.extend(found),
                _ => fail(out, format!("matches {hits} of `oneOf`, not exactly one")),
            }
        }
        if let Some(not) = s.get("not")
            && self.valid(value, not)
        {
            fail(out, "matches `not`".into());
        }
        if let Some(cond) = s.get("if") {
            let mut tmp = vec![];
            let e = self.check(value, cond, &mut tmp);
            let branch = if tmp.is_empty() {
                merge(&mut ev, e);
                s.get("then")
            } else {
                s.get("else")
            };
            if let Some(b) = branch {
                let e = self.check(value, b, out);
                merge(&mut ev, e);
            }
        }
        if let (Value::Object(o), Some(un)) = (value, s.get("unevaluatedProperties")) {
            // Sorted once: each key is then found by binary search.
            let mut seen = ev.props.clone();
            seen.sort_unstable();
            for (k, v) in o {
                if seen.binary_search(&k.as_str()).is_err() {
                    if !self.valid(v, un) {
                        out.push((pointer_key(k), "is not allowed (unevaluatedProperties)".into()));
                    }
                    ev.props.push(k);
                }
            }
        }
        if let (Value::Array(items), Some(un)) = (value, s.get("unevaluatedItems"))
            && !ev.all_items
        {
            for (i, v) in items.iter().enumerate().skip(ev.items) {
                if !ev.contained.contains(&i) && !self.valid(v, un) {
                    out.push((format!("/{i}"), "is not allowed (unevaluatedItems)".into()));
                }
            }
            ev.all_items = true;
        }
        ev
    }

    fn check_array(&self, items: &[Value], s: &Keywords<'_>, out: &mut Vec<(String, String)>, ev: &mut Evaluated<'_>) {
        let n = u64::try_from(items.len()).unwrap_or(u64::MAX);
        if let Some(m) = s.get("minItems").and_then(count).filter(|m| n < *m) {
            out.push((String::new(), format!("at least {m} items required, found {n}")));
        }
        if let Some(m) = s.get("maxItems").and_then(count).filter(|m| n > *m) {
            out.push((String::new(), format!("at most {m} items allowed, found {n}")));
        }
        if s.get("uniqueItems") == Some(&Value::Bool(true)) && runtime::__private::has_duplicates(items) {
            out.push((String::new(), "items must be unique".into()));
        }
        for (i, sub) in crate::json::items(s.get("prefixItems")) {
            if let Some(v) = items.get(i) {
                self.nested(v, sub, || format!("/{i}"), out);
            }
        }
        let start = s.get("prefixItems").and_then(Value::as_array).map(Vec::len).unwrap_or_default();
        ev.items = ev.items.max(start.min(items.len()));
        if let Some(sub) = s.get("items") {
            for (i, v) in items.iter().enumerate().skip(start) {
                self.nested(v, sub, || format!("/{i}"), out);
            }
            ev.all_items = true;
        }
        if let Some(sub) = s.get("contains") {
            let matched: Vec<usize> =
                items.iter().enumerate().filter(|(_, v)| self.valid(v, sub)).map(|(i, _)| i).collect();
            let hits = u64::try_from(matched.len()).unwrap_or(u64::MAX);
            ev.contained.extend(matched);
            let min = s.get("minContains").and_then(count).unwrap_or(1);
            if hits < min {
                out.push((String::new(), format!("must contain at least {min} matching item(s), found {hits}")));
            }
            if s.get("maxContains").and_then(count).is_some_and(|m| hits > m) {
                out.push((String::new(), format!("contains {hits} matching items, more than allowed")));
            }
        }
    }

    fn check_object<'v>(
        &self,
        value: &'v Value,
        o: &'v Map<String, Value>,
        s: &Keywords<'_>,
        out: &mut Vec<(String, String)>,
        ev: &mut Evaluated<'v>,
    ) {
        let n = u64::try_from(o.len()).unwrap_or(u64::MAX);
        if let Some(m) = s.get("minProperties").and_then(count).filter(|m| n < *m) {
            out.push((String::new(), format!("at least {m} properties required, found {n}")));
        }
        if let Some(m) = s.get("maxProperties").and_then(count).filter(|m| n > *m) {
            out.push((String::new(), format!("at most {m} properties allowed, found {n}")));
        }
        for r in s.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if !o.contains_key(r) {
                out.push((pointer_key(r), "is required".into()));
            }
        }
        if let Some(Value::Object(deps)) = s.get("dependentRequired") {
            for (k, needs) in deps {
                if o.contains_key(k) {
                    for r in needs.as_array().into_iter().flatten().filter_map(Value::as_str) {
                        if !o.contains_key(r) {
                            out.push((pointer_key(r), format!("is required when `{k}` is present")));
                        }
                    }
                }
            }
        }
        if let Some(Value::Object(deps)) = s.get("dependentSchemas") {
            for (k, sub) in deps {
                if o.contains_key(k) {
                    let e = self.check(value, sub, out);
                    merge(ev, e);
                }
            }
        }
        if let Some(names) = s.get("propertyNames") {
            for k in o.keys() {
                if !self.valid_name(k, names) {
                    out.push((pointer_key(k), "is not an allowed property name".into()));
                }
            }
        }
        let props = s.get("properties").and_then(Value::as_object);
        let patterns = s.get("patternProperties").and_then(Value::as_object);
        for (k, v) in o {
            let mut matched = props.and_then(|p| p.get(k)).is_some_and(|sub| {
                self.nested(v, sub, || pointer_key(k), out);
                true
            });
            for (p, sub) in patterns.into_iter().flatten() {
                if matches(p, k) {
                    self.nested(v, sub, || pointer_key(k), out);
                    matched = true;
                }
            }
            if matched {
                ev.props.push(k);
            } else if let Some(extra) = s.get("additionalProperties") {
                self.nested(v, extra, || pointer_key(k), out);
                ev.props.push(k);
            }
        }
    }

    /// Checks a member. Its violations get `at` as a prefix. `at` is built only
    /// when there is a violation.
    fn nested(&self, value: &Value, schema: &Value, at: impl FnOnce() -> String, out: &mut Vec<(String, String)>) {
        let start = out.len();
        self.check(value, schema, out);
        if out.len() > start {
            let at = at();
            for (p, _) in &mut out[start..] {
                p.insert_str(0, &at);
            }
        }
    }
}

fn merge<'v>(into: &mut Evaluated<'v>, from: Evaluated<'v>) {
    into.props.extend(from.props);
    into.items = into.items.max(from.items);
    into.all_items |= from.all_items;
    into.contained.extend(from.contained);
}

fn type_is(v: &Value, t: &str) -> bool {
    match t {
        "null" => v.is_null(),
        "boolean" => v.is_boolean(),
        "integer" => v.as_f64().is_some_and(|x| x.fract() == 0.0),
        "number" => v.is_number(),
        "string" => v.is_string(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        _ => true,
    }
}

/// 2^53: every whole float below it converts to an `i64` exactly.
const MAX_EXACT_INT: f64 = 9_007_199_254_740_992.0;

/// Returns a keyword's non-negative integer. `2.0` counts as `2` (2020-12: a number with a zero fraction is an
/// integer).
fn count(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| {
        let f = v.as_f64()?;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "whole, non-negative and below 2^53"
        )]
        (f >= 0.0 && f.fract() == 0.0 && f < MAX_EXACT_INT).then_some(f as u64)
    })
}

#[cfg(test)]
mod tests {
    /// A reference that resolves to nothing refuses the value: passing it would accept anything.
    #[test]
    fn a_reference_that_does_not_resolve_refuses_the_value() {
        let schema = serde_json::json!({"$ref": "#/$defs/missing"});
        let mut out = vec![];
        super::check_schema(&mut out, &super::InstancePath::Root, &serde_json::json!("anything"), &schema);
        assert_eq!(out.len(), 1, "{out:?}");
        assert!(out[0].message.contains("#/$defs/missing"), "{out:?}");
    }

    fn faults(value: &serde_json::Value, schema: &serde_json::Value) -> usize {
        let mut out = vec![];
        super::check_schema(&mut out, &super::InstancePath::Root, value, schema);
        out.len()
    }

    #[test]
    fn numbers_compare_by_value_and_counts_may_be_written_as_floats() {
        use serde_json::json;
        assert!(super::json_equal(&json!(-2), &json!(-2.0)));
        assert_eq!(faults(&json!(-2), &json!({"const": -2.0})), 0);
        assert_eq!(faults(&json!([1.0]), &json!({"enum": [[1]]})), 0);
        assert_eq!(faults(&json!("foo"), &json!({"maxLength": 2.0})), 1);
    }

    #[test]
    fn multiple_of_and_property_counts_hold_at_their_bounds() {
        use serde_json::json;
        assert_eq!(faults(&json!(4), &json!({"multipleOf": 2})), 0);
        assert_eq!(faults(&json!(0.3), &json!({"multipleOf": 0.1})), 0, "exact on the decimal text");
        assert_eq!(faults(&json!(0.300_000_000_000_000_04), &json!({"multipleOf": 0.1})), 1, "0.1 + 0.2");
        assert_eq!(faults(&json!(3), &json!({"multipleOf": 2})), 1);
        assert_eq!(faults(&json!({"a": 1}), &json!({"maxProperties": 1})), 0);
        assert_eq!(faults(&json!({"a": 1, "b": 2}), &json!({"maxProperties": 1})), 1);
    }

    #[test]
    fn property_names_and_null_types_are_checked() {
        use serde_json::json;
        assert_eq!(faults(&json!({"a": 1}), &json!({"propertyNames": {"minLength": 2}})), 1);
        assert_eq!(faults(&json!({"ab": 1}), &json!({"propertyNames": {"minLength": 2, "type": "string"}})), 0);
        assert_eq!(faults(&json!([null]), &json!({"contains": {"type": "null"}})), 0);
        assert_eq!(faults(&json!([1]), &json!({"contains": {"type": "null"}})), 1);
    }

    /// A definition that refers to itself on the same value consumes nothing. It passes instead of recursing forever.
    #[test]
    fn a_definition_that_refers_to_itself_passes() {
        use serde_json::json;
        let schema = json!({"$defs": {"loop": {"$ref": "#/$defs/loop"}}, "$ref": "#/$defs/loop"});
        assert_eq!(faults(&json!({"a": [1]}), &schema), 0);
        assert_eq!(faults(&json!([1]), &schema), 0);
    }

    #[test]
    fn number_bounds_hold_at_their_value() {
        use serde_json::json;
        assert_eq!(faults(&json!(1), &json!({"minimum": 1, "maximum": 1})), 0);
        assert_eq!(faults(&json!(0), &json!({"exclusiveMinimum": 1})), 1);
        assert_eq!(faults(&json!(2), &json!({"exclusiveMinimum": 1})), 0);
        assert_eq!(faults(&json!({"a": 1}), &json!({"minProperties": 2})), 1);
        assert_eq!(faults(&json!({"a": 1, "b": 2, "c": 3}), &json!({"minProperties": 2})), 0);
    }

    /// A count is a whole number from 0 below 2^53. Any other bound is no bound.
    #[test]
    fn a_count_that_is_not_whole_or_exact_is_no_bound() {
        use serde_json::json;
        assert_eq!(faults(&json!("a"), &json!({"maxLength": -1.0})), 0);
        assert_eq!(faults(&json!("ab"), &json!({"maxLength": 1.5})), 0);
        assert_eq!(faults(&json!("a"), &json!({"minLength": 9_007_199_254_740_992.0})), 0);
    }

    #[test]
    fn a_reference_checks_its_definition_and_types_are_checked() {
        use serde_json::json;
        let schema = json!({"$defs": {"s": {"type": "string"}}, "$ref": "#/$defs/s"});
        assert_eq!(faults(&json!(1), &schema), 1);
        assert_eq!(faults(&json!(1.5), &json!({"type": "integer"})), 1);
        assert_eq!(faults(&json!(1), &json!({"type": "object"})), 1);
        assert_eq!(faults(&json!({"b": 1}), &json!({"propertyNames": {"const": "a"}})), 1);
    }

    /// `^(a+)+$` takes exponential time on a long value that almost matches. Past `maxLength` it does not run: the
    /// value is wrong already. Run, 30,001 characters would not finish.
    #[test]
    fn a_pattern_does_not_run_past_max_length() {
        use serde_json::json;
        let long = format!("{}b", "a".repeat(30_000));
        assert_eq!(faults(&json!(long), &json!({"maxLength": 20, "pattern": "^(a+)+$"})), 1);
    }

    #[test]
    fn items_that_contains_matched_are_evaluated() {
        use serde_json::json;
        let schema = json!({"prefixItems": [true], "contains": {"type": "string"}, "unevaluatedItems": false});
        assert_eq!(faults(&json!([1, "foo"]), &schema), 0);
        assert_eq!(faults(&json!([1, 2]), &schema), 2, "no string, and the second item is unevaluated");
    }

    /// A fragment checked whole asserts `format` as the typed checks do. A value of another type ignores it.
    #[test]
    fn a_fragment_checked_whole_asserts_its_format() {
        use serde_json::json;
        let schema = json!({"allOf": [{"properties": {"at": {"type": ["string", "null"], "format": "date-time"}}}]});
        let mut out = vec![];
        super::check_schema(&mut out, &super::InstancePath::Root, &json!({"at": "yesterday"}), &schema);
        let found: Vec<String> = out.iter().map(|v| format!("{} {}", v.path, v.message)).collect();
        assert_eq!(found, ["/at is not a valid date-time"]);
        assert_eq!(faults(&json!({"at": "2026-11-15T10:00:00Z"}), &schema), 0);
        assert_eq!(faults(&json!({"at": null}), &schema), 0);
    }
}
