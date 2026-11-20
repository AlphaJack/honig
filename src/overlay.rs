//! Overlays: Overlay Specification 1.0 to 1.2.
//!
//! An overlay is an ordered list of actions. Each selects nodes of the description with an RFC 9535 `JSONPath`
//! and updates them, copies another node into them or removes them. They apply to the root document as written
//! before bundling and upgrading.

use std::fmt;

use serde_json::{Map, Value};
use serde_json_path::JsonPath;

use crate::diag::Diagnostics;
use crate::json::{join, unescape_token};

/// One step from a node to a child.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

/// What an action puts into its targets.
enum Change {
    Remove,
    Merge(Value),
}

/// Why an action cannot apply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActionError {
    NotObject,
    NoTarget,
    BadRef,
    NoReusable(String),
    BadPath { path: String, reason: String },
    UpdateAndCopy,
    CopyNotPath,
    CopyCount(usize),
    MixedTargets,
    Incompatible(Mismatch),
    TargetInFields,
}

/// The `update` or `copy` value cannot merge into the target at `keys` (from the selected node down).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Mismatch {
    keys: Vec<String>,
    value: Kind,
    target: Kind,
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotObject => write!(f, "an action is an object"),
            Self::NoTarget => write!(f, "an action needs a `target`"),
            Self::BadRef => write!(f, "`$ref` refers to a reusable action as `#/components/actions/<name>`"),
            Self::NoReusable(name) => write!(f, "no reusable action `{name}`"),
            Self::BadPath { path, reason } => write!(f, "`{path}` is not an RFC 9535 JSONPath: {reason}"),
            Self::UpdateAndCopy => write!(f, "an action has `update` or `copy`, not both"),
            Self::CopyNotPath => write!(f, "`copy` is a JSONPath"),
            Self::CopyCount(n) => write!(f, "`copy` selects {n} nodes, not one"),
            Self::MixedTargets => write!(f, "the target selects objects, arrays and primitives together"),
            Self::TargetInFields => {
                write!(f, "a reusable action's `fields` cannot have a `target`: the reference supplies it")
            }
            Self::Incompatible(Mismatch { keys, value, target }) => {
                let at = if keys.is_empty() { String::new() } else { format!(" at `{}`", keys.join(".")) };
                write!(f, "cannot merge {value} into {target}{at}")
            }
        }
    }
}

/// Applies the overlay `overlay` (named `name` in diagnostics) to `doc`. A broken action is reported and skipped;
/// the actions after it still apply.
pub(crate) fn apply(doc: &mut Value, overlay: &Value, name: &str, diags: &mut Diagnostics) {
    let at = |ptr: &str| format!("{name}#{ptr}");
    let Some(version) = overlay.get("overlay").and_then(Value::as_str) else {
        diags.error(at(""), "an overlay needs an `overlay` version");
        return;
    };
    if !["1.0.", "1.1.", "1.2."].iter().any(|v| version.starts_with(v)) {
        diags.error(at("/overlay"), format!("unsupported Overlay version {version}: 1.0, 1.1 and 1.2 are"));
        return;
    }
    // 1.0 appends an `update` to an array as one entry; from 1.1 an array is concatenated.
    let append_arrays = version.starts_with("1.0.");
    let Some(actions) = overlay.get("actions").and_then(Value::as_array) else {
        diags.error(at(""), "an overlay needs an `actions` list");
        return;
    };
    if actions.is_empty() {
        diags.error(at("/actions"), "`actions` needs at least one action");
    }
    for field in ["title", "version"] {
        if overlay.get("info").and_then(|i| i.get(field)).and_then(Value::as_str).is_none() {
            diags.error(at(&format!("/info/{field}")), format!("an overlay needs `info.{field}`"));
        }
    }
    if version.starts_with("1.2.") {
        identity(doc, overlay, &at, diags);
    }
    for (i, action) in actions.iter().enumerate() {
        match run(doc, overlay, action, append_arrays) {
            Err(e) => diags.error(at(&format!("/actions/{i}")), e.to_string()),
            // Not an error ("the action has no effect"). But a target that matches
            // nothing is most often a JSONPath that does not say what was meant.
            // RFC 9535 filters test the children of the node before them.
            Ok(0) => diags
                .warning(at(&format!("/actions/{i}/target")), "the target selects no nodes: the action does nothing"),
            Ok(_) => {}
        }
    }
}

/// Returns the Overlay 1.2 document identity. `extends` and `$self` name whole documents. So
/// they carry no fragment. An absolute `extends` refers to the `$self` of the description.
fn identity(doc: &Value, overlay: &Value, at: &dyn Fn(&str) -> String, diags: &mut Diagnostics) {
    for (field, ptr) in [("extends", "/extends"), ("$self", "/$self")] {
        if overlay.get(field).and_then(Value::as_str).is_some_and(|u| u.contains('#')) {
            diags.error(at(ptr), format!("`{field}` identifies a whole document: it cannot have a `#` fragment"));
        }
    }
    let absolute = |u: &str| crate::schema_ids::scheme(u).is_some();
    if let (Some(extends), Some(own)) =
        (overlay.get("extends").and_then(Value::as_str), doc.get("$self").and_then(Value::as_str))
        && absolute(extends)
        && extends != own
    {
        diags.warning(at("/extends"), format!("`extends` is `{extends}` but the description's `$self` is `{own}`"));
    }
}

/// Applies one action; returns how many nodes its target selected.
fn run(doc: &mut Value, overlay: &Value, action: &Value, append_arrays: bool) -> Result<usize, ActionError> {
    let action = resolved(overlay, action)?;
    let target = action.get("target").and_then(Value::as_str).ok_or(ActionError::NoTarget)?;
    let targets = select(doc, target)?;
    let change = if action.get("remove") == Some(&Value::Bool(true)) {
        Change::Remove
    } else {
        match (action.get("update"), action.get("copy")) {
            (Some(_), Some(_)) => return Err(ActionError::UpdateAndCopy),
            (Some(update), None) => Change::Merge(update.clone()),
            (None, Some(copy)) => {
                let source = select(doc, copy.as_str().ok_or(ActionError::CopyNotPath)?)?;
                let [path] = source.as_slice() else { return Err(ActionError::CopyCount(source.len())) };
                // Selected from this document a moment ago: the node is there.
                Change::Merge(node(doc, path).cloned().unwrap_or_default())
            }
            (None, None) => return Ok(targets.len()),
        }
    };
    match change {
        // Last first: removing an array item leaves the indices before it valid.
        Change::Remove => {
            for path in targets.iter().rev() {
                remove(doc, path);
            }
        }
        Change::Merge(value) => {
            let kinds: Vec<Kind> = targets.iter().filter_map(|p| node(doc, p)).map(Kind::of).collect();
            if kinds.windows(2).any(|w| w[0] != w[1]) {
                return Err(ActionError::MixedTargets);
            }
            for path in &targets {
                if let Some(t) = node_mut(doc, path) {
                    merge_node(t, &value, append_arrays)?;
                }
            }
        }
    }
    Ok(targets.len())
}

/// Returns the action itself or a Reusable Action Reference (1.2) completed from `components.actions`. The own fields
/// of the reference (its `target`, `description` or any other) win over those of the reusable action.
fn resolved(overlay: &Value, action: &Value) -> Result<Map<String, Value>, ActionError> {
    let fields = action.as_object().ok_or(ActionError::NotObject)?;
    let Some(r) = fields.get("$ref") else { return Ok(fields.clone()) };
    let name = r.as_str().and_then(|r| r.strip_prefix("#/components/actions/")).ok_or(ActionError::BadRef)?;
    let reusable = overlay
        .pointer(&join("/components/actions", &unescape_token(name)))
        .ok_or_else(|| ActionError::NoReusable(name.to_owned()))?;
    let mut out = reusable.get("fields").and_then(Value::as_object).cloned().unwrap_or_default();
    if out.contains_key("target") {
        return Err(ActionError::TargetInFields);
    }
    out.extend(fields.iter().filter(|(k, _)| *k != "$ref").map(|(k, v)| (k.clone(), v.clone())));
    Ok(out)
}

/// Returns the paths of the nodes that `path` selects in document order.
fn select(doc: &Value, path: &str) -> Result<Vec<Vec<Step>>, ActionError> {
    let query =
        JsonPath::parse(path).map_err(|e| ActionError::BadPath { path: path.to_owned(), reason: e.to_string() })?;
    Ok(query
        .query_located(doc)
        .locations()
        .map(|l| {
            l.iter()
                .map(|e| match e {
                    serde_json_path::PathElement::Name(n) => Step::Key((*n).to_owned()),
                    serde_json_path::PathElement::Index(i) => Step::Index(*i),
                })
                .collect()
        })
        .collect())
}

fn node<'d>(doc: &'d Value, path: &[Step]) -> Option<&'d Value> {
    path.iter().try_fold(doc, |v, s| match s {
        Step::Key(k) => v.get(k),
        Step::Index(i) => v.get(i),
    })
}

fn node_mut<'d>(doc: &'d mut Value, path: &[Step]) -> Option<&'d mut Value> {
    path.iter().try_fold(doc, |v, s| match s {
        Step::Key(k) => v.get_mut(k),
        Step::Index(i) => v.get_mut(i),
    })
}

/// Removes the node at `path` from the object or array holding it. The root stays.
fn remove(doc: &mut Value, path: &[Step]) {
    let Some((last, parent)) = path.split_last() else { return };
    match (node_mut(doc, parent), last) {
        (Some(Value::Object(o)), Step::Key(k)) => {
            o.shift_remove(k);
        }
        (Some(Value::Array(a)), Step::Index(i)) if *i < a.len() => {
            a.remove(*i);
        }
        _ => {}
    }
}

/// The three kinds the merge rules tell apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Object,
    Array,
    Primitive,
}

impl Kind {
    fn of(v: &Value) -> Self {
        match v {
            Value::Object(_) => Self::Object,
            Value::Array(_) => Self::Array,
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Self::Primitive,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Object => "an object",
            Self::Array => "an array",
            Self::Primitive => "a primitive",
        })
    }
}

/// Merges `value` into a selected node. An object merges. An array takes `value` (concatenated when it is an
/// array itself, from 1.1). A primitive is replaced by a primitive.
fn merge_node(target: &mut Value, value: &Value, append_arrays: bool) -> Result<(), ActionError> {
    match (target, value) {
        (Value::Array(t), Value::Array(v)) if !append_arrays => t.extend(v.iter().cloned()),
        (Value::Array(t), v) => t.push(v.clone()),
        (t, v) => merge(t, v).map_err(ActionError::Incompatible)?,
    }
    Ok(())
}

/// Returns the recursive merge of the spec. Primitives replace, arrays concatenate and objects merge. Any other pair
/// is incompatible.
fn merge(target: &mut Value, value: &Value) -> Result<(), Mismatch> {
    match (target, value) {
        (Value::Object(t), Value::Object(v)) => {
            for (k, v) in v {
                let Some(existing) = t.get_mut(k) else {
                    t.insert(k.clone(), v.clone());
                    continue;
                };
                merge(existing, v).map_err(|mut m| {
                    m.keys.insert(0, k.clone());
                    m
                })?;
            }
        }
        (Value::Array(t), Value::Array(v)) => t.extend(v.iter().cloned()),
        (t, v) if Kind::of(t) == Kind::Primitive && Kind::of(v) == Kind::Primitive => *t = v.clone(),
        (t, v) => return Err(Mismatch { keys: vec![], value: Kind::of(v), target: Kind::of(t) }),
    }
    Ok(())
}
