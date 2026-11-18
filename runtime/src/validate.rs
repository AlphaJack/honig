//! Schema constraints and the violations they report.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::{Map, Value};

/// How far from whole, relative to it, the quotient of a `multipleOf` too long to read exactly may be.
const FALLBACK_TOLERANCE: f64 = 1e-6;

/// A constraint a value breaks: where (JSON pointer) and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// JSON pointer into the request, e.g. `/body/tags/0`.
    pub path: String,
    /// What is wrong.
    pub message: String,
}

/// Where a value is in a request. It is a JSON pointer that is formatted only on a violation.
#[derive(Debug, Clone, Copy)]
pub enum InstancePath<'a> {
    /// The request itself (`""`).
    Root,
    /// A property, parameter location or name under a parent.
    Key(&'a Self, &'a str),
    /// An array position under a parent.
    Index(&'a Self, usize),
}

impl fmt::Display for InstancePath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => Ok(()),
            Self::Key(parent, key) => write!(f, "{parent}/{}", key.replace('~', "~0").replace('/', "~1")),
            Self::Index(parent, i) => write!(f, "{parent}/{i}"),
        }
    }
}

/// Records a violation.
pub fn violation(out: &mut Vec<Violation>, path: &InstancePath<'_>, message: impl Into<String>) {
    out.push(Violation { path: path.to_string(), message: message.into() });
}

/// Checks a string `format` the crate has no library type for: what parsing into that type would refuse.
///
/// The runtime checks `decimal` itself. Some formats are checked by the crates that the generator turns on when a
/// description uses them: `uuid` (`uuid`), the dates and times (`time`), `uri` (`url`) and the CIDR blocks
/// (`ipnet`). Mail addresses and host names are types of their own (feature `idn`). Decoding checks them. Other
/// formats are not checked.
#[cfg_attr(feature = "idn", doc = "\nThose types are [`Email`] and [`Hostname`].")]
pub fn check_format(out: &mut Vec<Violation>, path: &InstancePath<'_>, value: &str, format: &str) {
    if !is_format(value, format) {
        violation(out, path, format!("is not a valid {format}"));
    }
}

/// Is `value` of `format` as [`check_format`] checks it? A format it does not know is.
pub(crate) fn is_format(value: &str, format: &str) -> bool {
    match format {
        "uuid" => uuid(value),
        "date" => crate::formats::is_date(value),
        "time" => crate::full_time::is_full_time(value),
        "time-local" => crate::formats::is_time_local(value),
        "date-time" => crate::formats::is_date_time(value),
        "date-time-local" => crate::formats::is_date_time_local(value),
        "ipv4" => crate::formats::is_ipv4(value),
        "ipv6" => crate::formats::is_ipv6(value),
        "decimal" => decimal(value),
        "decimal128" => decimal128(value).is_ok(),
        "http-date" => crate::http_date::is_http_date(value),
        "duration" => crate::duration::is_duration(value),
        "uri-reference" => crate::uri::is_reference(value, crate::uri::Chars::Uri, false),
        "iri" => crate::uri::is_reference(value, crate::uri::Chars::Iri, true),
        "iri-reference" => crate::uri::is_reference(value, crate::uri::Chars::Iri, false),
        "json-pointer" => crate::json_pointer::is_json_pointer(value),
        "relative-json-pointer" => crate::json_pointer::is_relative_json_pointer(value),
        "uri-template" => crate::uri_template::is_uri_template(value),
        // An ECMA-262 regular expression, as `pattern` compiles one.
        #[cfg(feature = "regress")]
        "regex" => crate::pattern::compiles(value),
        // An absolute URI (RFC 3986): one with a scheme.
        "uri" => crate::uri::is_reference(value, crate::uri::Chars::Uri, true),
        #[cfg(feature = "idn")]
        "email" | "idn-email" | "hostname" | "idn-hostname" => mail::is(value, format),
        #[cfg(feature = "ipnet")]
        "ipv4-cidr" => cidr_address(value, crate::formats::is_ipv4) && value.parse::<ipnet::Ipv4Net>().is_ok(),
        #[cfg(feature = "ipnet")]
        "ipv6-cidr" => cidr_address(value, crate::formats::is_ipv6) && value.parse::<ipnet::Ipv6Net>().is_ok(),
        _ => true,
    }
}

/// Is the address before the `/` of a CIDR block valid? `ipnet` takes leading zeros (`01.2.3.4`) and a `::` that
/// stands for no group (`1:2:3:4:5:6:7::8`). `ipv4` and `ipv6` refuse both.
#[cfg(feature = "ipnet")]
fn cidr_address(value: &str, address: fn(&str) -> bool) -> bool {
    value.split_once('/').is_some_and(|(a, _)| address(a))
}

/// The length of a UUID's text and where its hyphens sit (RFC 9562 section 4: 8-4-4-4-12 hex digits).
const UUID_LEN: usize = 36;
/// The offsets of the hyphens of a UUID.
const UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// Returns the hyphenated form of RFC 9562. Without hyphens, in braces or as a URN it is another form.
fn uuid(s: &str) -> bool {
    s.len() == UUID_LEN
        && s.bytes().enumerate().all(|(i, b)| if UUID_HYPHENS.contains(&i) { b == b'-' } else { b.is_ascii_hexdigit() })
}

/// Returns an optional `-` and digits. Then optionally `.` and more digits.
fn decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (whole, fraction) = s.split_once('.').unwrap_or((s, "0"));
    let number = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    number(whole) && number(fraction)
}

/// The significant digits that IEEE 754 decimal128 holds.
const DECIMAL128_DIGITS: usize = 34;
/// The smallest exponent of the last digit that decimal128 holds: its smallest value is `1e-6176`.
const DECIMAL128_MIN_EXPONENT: i64 = -6176;
/// The largest exponent of the first digit that decimal128 holds: its largest values are below `1e6145`.
const DECIMAL128_MAX_EXPONENT: i64 = 6144;

/// Checks that `s` is a decimal number that IEEE 754 decimal128 holds exactly: an optional `-`, digits with an
/// optional fraction and an optional exponent, at most 34 significant digits, within its exponent range. Zeros before
/// the first and after the last nonzero digit are not significant: `1.50000` holds as `1.5`.
///
/// # Errors
///
/// Why it does not hold.
pub(crate) fn decimal128(s: &str) -> Result<(), String> {
    let unsigned = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((w, f)) if !f.is_empty() => (w, f),
        Some(_) => return Err("is not a decimal".to_owned()),
        None => (mantissa, ""),
    };
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    let exponent_digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
    if whole.is_empty() || !digits(whole) || !digits(fraction) || exponent_digits.is_empty() || !digits(exponent_digits)
    {
        return Err("is not a decimal".to_owned());
    }
    let all = format!("{whole}{fraction}");
    let significant = all.trim_start_matches('0');
    let trimmed = significant.trim_end_matches('0');
    if trimmed.is_empty() {
        return Ok(());
    }
    // An exponent too long for an `i64` is far out of range either way.
    let Ok(exponent) = exponent.strip_prefix('+').unwrap_or(exponent).parse::<i64>() else {
        return Err("is out of the range of decimal128".to_owned());
    };
    let count = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    // The exponent of the last significant digit, and of the first.
    let last = exponent.saturating_sub(count(fraction.len())).saturating_add(count(significant.len() - trimmed.len()));
    let first = last.saturating_add(count(trimmed.len()) - 1);
    if trimmed.len() > DECIMAL128_DIGITS {
        return Err(format!(
            "has {} significant digits, more than the {DECIMAL128_DIGITS} of decimal128",
            trimmed.len()
        ));
    }
    if last < DECIMAL128_MIN_EXPONENT || first > DECIMAL128_MAX_EXPONENT {
        return Err("is out of the range of decimal128".to_owned());
    }
    Ok(())
}

/// Checks that the decimal number `text` is one that IEEE 754 decimal128 holds exactly (`format: decimal128`).
pub fn check_decimal128(out: &mut Vec<Violation>, path: &InstancePath<'_>, text: &str) {
    if let Err(why) = decimal128(text) {
        violation(out, path, format!("`{text}` {why}"));
    }
}

/// Records `message` at `path` unless `ok`.
pub fn check(out: &mut Vec<Violation>, path: &InstancePath<'_>, ok: bool, message: &str) {
    if !ok {
        violation(out, path, message);
    }
}

/// Checks `minLength`/`maxLength`, `minItems`/`maxItems`, `minProperties`/`maxProperties`.
pub fn check_len(
    out: &mut Vec<Violation>,
    path: &InstancePath<'_>,
    len: usize,
    min: Option<u64>,
    max: Option<u64>,
    unit: &str,
) {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if let Some(m) = min.filter(|m| len < *m) {
        violation(out, path, format!("at least {m} {unit} required, found {len}"));
    }
    if let Some(m) = max.filter(|m| len > *m) {
        violation(out, path, format!("at most {m} {unit} allowed, found {len}"));
    }
}

/// A number as JSON has one. An integer always is one. A float is one only when it is finite.
pub trait JsonNumber {
    /// Is it a JSON number (RFC 8259 section 6)? `NaN` and the infinities are not.
    fn is_json_number(&self) -> bool;
}

macro_rules! json_number {
    ($($t:ty: $is:expr),* $(,)?) => {$(
        impl JsonNumber for $t {
            fn is_json_number(&self) -> bool {
                $is(self)
            }
        }
    )*};
}

json_number!(
    i8: |_| true, i16: |_| true, i32: |_| true, i64: |_| true, u8: |_| true, u16: |_| true, u32: |_| true,
    u64: |_| true, f32: |n: &f32| n.is_finite(), f64: |n: &f64| n.is_finite(),
);

/// Checks `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`.
pub fn check_range<T: PartialOrd + fmt::Display + Copy + JsonNumber>(
    out: &mut Vec<Violation>,
    path: &InstancePath<'_>,
    v: T,
    bounds: [Option<T>; 4],
) {
    let [min, max, xmin, xmax] = bounds;
    // `NaN` compares false with every bound. An infinity passes the bound on its own side.
    if !v.is_json_number() {
        violation(out, path, "is not a number");
        return;
    }
    if let Some(m) = min.filter(|m| v < *m) {
        violation(out, path, format!("{v} is less than the minimum {m}"));
    }
    if let Some(m) = max.filter(|m| v > *m) {
        violation(out, path, format!("{v} is greater than the maximum {m}"));
    }
    if let Some(m) = xmin.filter(|m| v <= *m) {
        violation(out, path, format!("{v} must be greater than {m}"));
    }
    if let Some(m) = xmax.filter(|m| v >= *m) {
        violation(out, path, format!("{v} must be less than {m}"));
    }
}

/// Checks that the number `value` is a multiple of `of`, exactly: both are read as their decimal text. An `f64` is its
/// shortest text, the one that it decodes from.
pub fn check_multiple<T: fmt::Display + JsonNumber + Copy>(
    out: &mut Vec<Violation>,
    path: &InstancePath<'_>,
    value: T,
    of: &str,
) {
    if value.is_json_number() {
        check_multiple_text(out, path, &value.to_string(), of);
    }
}

/// Checks that the decimal text `value` is a multiple of `of`, exactly. An `of` of more than 19 significant digits is
/// compared by `f64`, within a millionth of the quotient.
pub fn check_multiple_text(out: &mut Vec<Violation>, path: &InstancePath<'_>, value: &str, of: &str) {
    let multiple = crate::exact::is_multiple(value, of).unwrap_or_else(|| {
        let (v, m) = (value.parse::<f64>().unwrap_or_default(), of.parse::<f64>().unwrap_or(1.0));
        let q = v / m;
        (q - q.round()).abs() <= q.abs().max(1.0) * FALLBACK_TOLERANCE
    });
    if !multiple {
        violation(out, path, format!("is not a multiple of {of}"));
    }
}

/// Checks `minimum`, `maximum`, `exclusiveMinimum` and `exclusiveMaximum` of the decimal text `value` exactly: a
/// decimal keeps more digits than an `f64` compares.
pub fn check_decimal_range(out: &mut Vec<Violation>, path: &InstancePath<'_>, value: &str, bounds: [Option<&str>; 4]) {
    use std::cmp::Ordering::{Greater, Less};
    let [min, max, xmin, xmax] = bounds;
    let cmp = |b: &str| crate::exact::compare(value, b);
    if let Some(m) = min.filter(|m| cmp(m) == Some(Less)) {
        violation(out, path, format!("{value} is less than the minimum {m}"));
    }
    if let Some(m) = max.filter(|m| cmp(m) == Some(Greater)) {
        violation(out, path, format!("{value} is greater than the maximum {m}"));
    }
    if let Some(m) = xmin.filter(|m| cmp(m) != Some(Greater)) {
        violation(out, path, format!("{value} must be greater than {m}"));
    }
    if let Some(m) = xmax.filter(|m| cmp(m) != Some(Less)) {
        violation(out, path, format!("{value} must be less than {m}"));
    }
}

/// Returns whether two of `items` are equal.
/// Whether two items are equal as JSON Schema compares instances. The canonical JSON of each item is sorted. Then
/// neighbours are compared. This is O(n log n). A comparison of every pair would let one request array cost O(n^2).
fn has_duplicates<T: Serialize>(items: &[T]) -> bool {
    let mut keys: Vec<String> = items
        .iter()
        .enumerate()
        // Never equal to another item: a key no JSON text starts with.
        .map(|(i, x)| unique_key(x).unwrap_or_else(|| ["\u{0}", itoa::Buffer::new().format(i)].concat()))
        .collect();
    keys.sort_unstable();
    keys.windows(2).any(|w| w.first() == w.last())
}

/// Returns the canonical JSON of `x`: equal for items that `uniqueItems` takes as equal. `None` when it does not
/// serialize.
pub(crate) fn unique_key<T: Serialize>(x: &T) -> Option<String> {
    let v = serde_json::to_value(x).ok()?;
    let mut key = String::new();
    canonical(&v, &mut key);
    Some(key)
}

/// Writes JSON text that is the same for equal instances. Object keys are sorted. A number is written by its value.
/// `1`, `1.0` and `1e0` are one number. So are `0` and `-0.0`.
fn canonical(v: &Value, out: &mut String) {
    match v {
        Value::Array(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            let mut members: Vec<_> = o.iter().collect();
            members.sort_unstable_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (i, (k, x)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical(x, out);
            }
            out.push('}');
        }
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), ..) => out.push_str(itoa::Buffer::new().format(i)),
            (None, Some(u), _) => out.push_str(itoa::Buffer::new().format(u)),
            (None, None, Some(f)) => out.push_str(&canonical_float(f)),
            (None, None, None) => out.push_str(&n.to_string()),
        },
        Value::Null | Value::Bool(_) | Value::String(_) => out.push_str(&v.to_string()),
    }
}

/// Returns a float as its shortest round-trip text. `Display` writes a whole float as the integer it equals (`1.0` as
/// `1`) and never in exponent form. Only `-0.0` needs a case of its own.
fn canonical_float(f: f64) -> String {
    if f == 0.0 {
        return "0".to_owned();
    }
    f.to_string()
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

/// Returns JSON Schema equality (2020-12 core 4.2.2): numbers by value, so `-2` equals `-2.0`. Objects ignore key
/// order.
#[must_use]
pub fn json_equal(a: &Value, b: &Value) -> bool {
    let (mut x, mut y) = (String::new(), String::new());
    canonical(a, &mut x);
    canonical(b, &mut y);
    x == y
}

/// Checks `uniqueItems`.
pub fn check_unique<T: Serialize>(out: &mut Vec<Violation>, path: &InstancePath<'_>, items: &[T]) {
    if has_duplicates(items) {
        violation(out, path, "items must be unique");
    }
}

/// Checks the JSON Schema keywords with no Rust type of their own against the serialized value.
/// Examples are `contains`, `if`/`then` and `dependentRequired`.
pub fn check_schema(out: &mut Vec<Violation>, path: &InstancePath<'_>, value: &Value, schema: &Value) {
    let mut found = vec![];
    let defs = schema.get("$defs").and_then(Value::as_object);
    Schema { defs, memo: RefCell::new(BTreeMap::new()) }.check(value, schema, &mut found);
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

/// The token of a JSON pointer unescaped: `~1` is `/` and `~0` is `~`.
fn unescape(token: &str) -> String {
    token.replace("~1", "/").replace("~0", "~")
}

/// Does `s` match `pattern`? A description with a `pattern` turns on the `regress` feature. So without the feature
/// there is no pattern to check.
#[cfg_attr(not(feature = "regress"), expect(clippy::missing_const_for_fn, reason = "the feature's body is not"))]
fn matches(pattern: &str, s: &str) -> bool {
    #[cfg(feature = "regress")]
    return crate::pattern::pattern_matches_dyn(pattern, s);
    #[cfg(not(feature = "regress"))]
    {
        let _ = (pattern, s);
        true
    }
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
    /// Returns the definition that `reference` names in the `$defs` of the root (`#/$defs/<token>`).
    fn def(&self, reference: &str) -> Option<&Value> {
        self.defs?.get(&unescape(reference.strip_prefix("#/$defs/")?))
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
        let plain = s.iter().all(|(k, v)| match k.as_str() {
            "pattern" | "minLength" | "maxLength" => true,
            "type" => v == "string",
            _ => false,
        });
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
        if let Some(target) = s.get("$ref").and_then(Value::as_str).and_then(|r| self.def(r)) {
            let e = self.check_def(value, target, out);
            merge(&mut ev, e);
        }
        let fail = |out: &mut Vec<(String, String)>, m: String| out.push((String::new(), m));
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
                        && crate::exact::is_multiple(&num.to_string(), &m.to_string()) == Some(false)
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
        if s.get("uniqueItems") == Some(&Value::Bool(true)) && has_duplicates(items) {
            out.push((String::new(), "items must be unique".into()));
        }
        let prefix = s.get("prefixItems").and_then(Value::as_array);
        for (i, sub) in prefix.into_iter().flatten().enumerate() {
            if let Some(v) = items.get(i) {
                self.nested(v, sub, || format!("/{i}"), out);
            }
        }
        let start = prefix.map(Vec::len).unwrap_or_default();
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

/// Check the schema constraints of a value (generated per type).
pub trait Validate {
    /// Appends every violation under `path` to `out`. Default: nothing can fail.
    fn validate(&self, _path: &InstancePath<'_>, _out: &mut Vec<Violation>) {}
}

/// Does `value` pass every check? An untagged union takes a branch only when it does: a branch that decodes but fails
/// its checks leaves the value to the branches after it.
#[must_use]
pub fn is_valid<T: Validate + ?Sized>(value: &T) -> bool {
    let mut out = Vec::new();
    value.validate(&InstancePath::Root, &mut out);
    out.is_empty()
}

impl<T: Validate> Validate for Option<T> {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        if let Some(v) = self {
            v.validate(path, out);
        }
    }
}

impl<T: Validate> Validate for Box<T> {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        (**self).validate(path, out);
    }
}

impl<T: Validate> Validate for [T] {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        for (i, v) in self.iter().enumerate() {
            v.validate(&InstancePath::Index(path, i), out);
        }
    }
}

impl<T: Validate> Validate for Vec<T> {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        self.as_slice().validate(path, out);
    }
}

impl<T: Validate> Validate for BTreeMap<String, T> {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        for (k, v) in self {
            v.validate(&InstancePath::Key(path, k), out);
        }
    }
}

impl<T: Validate, const N: usize> Validate for [T; N] {
    fn validate(&self, path: &InstancePath<'_>, out: &mut Vec<Violation>) {
        self.as_slice().validate(path, out);
    }
}

#[cfg(feature = "idn")]
pub use mail::{Email, Hostname, IdnEmail, IdnHostname};

/// Mail addresses and host names: types only a valid value can be.
#[cfg(feature = "idn")]
mod mail {
    /// Does `value` have `format` (an email or host name format)?
    pub(super) fn is(value: &str, format: &str) -> bool {
        match format {
            "email" => email(value, false),
            "idn-email" => email(value, true),
            "hostname" => hostname(value, false),
            _ => hostname(value, true),
        }
    }

    use idna::uts46::{AsciiDenyList, DnsLength, Hyphens, Uts46};

    /// A string of a `format` that only a valid value can be. `TryFrom`, `FromStr` and decoding check it.
    macro_rules! checked_text {
        ($(#[$doc:meta])* $name:ident, $format:literal, $check:expr) => {
            $(#[$doc])*
            #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
            #[serde(transparent)]
            pub struct $name(String);

            impl $name {
                /// The `format` it stands for.
                pub const FORMAT: &'static str = $format;

                /// Returns the value as text.
                #[must_use]
                pub fn as_str(&self) -> &str {
                    &self.0
                }

                /// Returns the value as an owned `String`.
                #[must_use]
                pub fn into_inner(self) -> String {
                    self.0
                }
            }

            impl TryFrom<String> for $name {
                type Error = String;

                fn try_from(value: String) -> Result<Self, String> {
                    if $check(&value) { Ok(Self(value)) } else { Err(format!("`{value}` is not a valid {}", $format)) }
                }
            }

            impl TryFrom<&str> for $name {
                type Error = String;

                fn try_from(value: &str) -> Result<Self, String> {
                    Self::try_from(value.to_owned())
                }
            }

            impl std::str::FromStr for $name {
                type Err = String;

                fn from_str(value: &str) -> Result<Self, String> {
                    Self::try_from(value)
                }
            }

            impl AsRef<str> for $name {
                fn as_ref(&self) -> &str {
                    &self.0
                }
            }

            impl std::fmt::Display for $name {
                fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str(&self.0)
                }
            }

            impl<'de> serde::Deserialize<'de> for $name {
                fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                    let s = <String as serde::Deserialize>::deserialize(d)?;
                    Self::try_from(s).map_err(serde::de::Error::custom)
                }
            }
        };
    }

    checked_text!(
        /// An RFC 5321 mail address (`format: email`). It is an ASCII dot-atom or quoted local part, then `@`, then a
        /// host name or an address literal.
        Email,
        "email",
        |v: &str| email(v, false)
    );
    checked_text!(
        /// An RFC 6531 mail address (`format: idn-email`): an [`Email`] that may hold UTF-8.
        IdnEmail,
        "idn-email",
        |v: &str| email(v, true)
    );
    checked_text!(
        /// An RFC 1123 host name (`format: hostname`) with ASCII labels. A punycode label must decode to a valid name.
        Hostname,
        "hostname",
        |v: &str| hostname(v, false)
    );
    checked_text!(
        /// An IDNA2008 host name (`format: idn-hostname`): a [`Hostname`] whose labels may be Unicode.
        IdnHostname,
        "idn-hostname",
        |v: &str| hostname(v, true)
    );

    /// RFC 5321 section 4.5.3.1.1: the longest local part in octets.
    const LOCAL: usize = 64;
    /// RFC 5321 section 4.5.3.1.3: the longest path is 256 octets with its angle brackets.
    const ADDRESS: usize = 254;

    /// Returns a host name with RFC 1123 labels (`hostname`) or IDNA2008 labels (`idn-hostname`). In ASCII each label
    /// holds 1 to 63 octets of letters, digits and inner hyphens. The name holds 253 in all. A punycode label (`xn--`)
    /// must decode to a valid one.
    fn hostname(value: &str, unicode: bool) -> bool {
        (unicode || value.is_ascii())
            && !value.is_empty()
            && Uts46::new().to_ascii(value.as_bytes(), AsciiDenyList::STD3, Hyphens::Check, DnsLength::Verify).is_ok()
            && value.trim_end_matches('.').split('.').all(idna2008_label)
    }

    /// The prefix of an A-label (RFC 5890 section 2.3.2.1).
    const ACE: &str = "xn--";
    /// The Greek script (RFC 5892 appendix A.4).
    const GREEK: [(char, char); 2] = [('\u{370}', '\u{3FF}'), ('\u{1F00}', '\u{1FFF}')];
    /// The Hebrew script (appendix A.5 and A.6).
    const HEBREW: [(char, char); 2] = [('\u{590}', '\u{5FF}'), ('\u{FB1D}', '\u{FB4F}')];
    /// Hiragana, Katakana and Han (appendix A.7).
    const JAPANESE: [(char, char); 7] = [
        ('\u{3040}', '\u{309F}'),
        ('\u{30A0}', '\u{30FF}'),
        ('\u{31F0}', '\u{31FF}'),
        ('\u{3400}', '\u{4DBF}'),
        ('\u{4E00}', '\u{9FFF}'),
        ('\u{F900}', '\u{FAFF}'),
        ('\u{20000}', '\u{2A6DF}'),
    ];

    /// Returns RFC 5892 beyond UTS 46: the code points it makes DISALLOWED (section 2.6) and the CONTEXTO rules of its
    /// appendix A. An A-label (`xn--...`) is checked as the label it decodes to.
    fn idna2008_label(label: &str) -> bool {
        let decoded;
        let label = match label.get(..ACE.len()).filter(|p| p.eq_ignore_ascii_case(ACE)) {
            Some(_) => match label.get(ACE.len()..).and_then(idna::punycode::decode_to_string) {
                Some(d) => {
                    decoded = d;
                    decoded.as_str()
                }
                None => return false,
            },
            None => label,
        };
        let chars: Vec<char> = label.chars().collect();
        let within = |ranges: &[(char, char)], c: char| ranges.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c));
        let greek = |c: char| within(&GREEK, c) && c != '\u{375}';
        let hebrew = |c: char| within(&HEBREW, c);
        let japanese = |c: char| c != '\u{30FB}' && within(&JAPANESE, c);
        chars.iter().enumerate().all(|(i, &c)| {
            let (before, after) = (i.checked_sub(1).and_then(|j| chars.get(j)), chars.get(i + 1));
            match c {
                // Section 2.6: DISALLOWED exceptions.
                '\u{640}' | '\u{7FA}' | '\u{302E}' | '\u{302F}' | '\u{3031}'..='\u{3035}' | '\u{303B}' => false,
                // Appendix A.3 MIDDLE DOT: between two `l`.
                '\u{B7}' => before == Some(&'l') && after == Some(&'l'),
                // A.4 GREEK LOWER NUMERAL SIGN (KERAIA): followed by Greek.
                '\u{375}' => after.is_some_and(|&a| greek(a)),
                // A.5 and A.6 HEBREW GERESH and GERSHAYIM: preceded by Hebrew.
                '\u{5F3}' | '\u{5F4}' => before.is_some_and(|&b| hebrew(b)),
                // A.7 KATAKANA MIDDLE DOT: the label has Hiragana, Katakana or Han.
                '\u{30FB}' => chars.iter().any(|&x| japanese(x)),
                // A decoded label holds no control character.
                c => !c.is_control(),
            }
        })
    }

    /// Returns an RFC 5321 Mailbox (`email`) or an RFC 6531 one that may hold UTF-8 (`idn-email`). It is a dot-atom or
    /// a quoted local part, then `@`, then a host name or an address literal (`[192.0.2.1]` or `[IPv6:::1]`).
    fn email(value: &str, unicode: bool) -> bool {
        // A quoted local part may hold `@`; the domain cannot.
        let Some((local, domain)) = value.rsplit_once('@') else { return false };
        (unicode || value.is_ascii())
            && local.len() <= LOCAL
            && value.len() <= ADDRESS
            && local_part(local, unicode)
            && (address_literal(domain) || hostname(domain, unicode))
    }

    /// Returns a dot-atom (atoms of `atext` between single dots) or a quoted string.
    fn local_part(local: &str, unicode: bool) -> bool {
        match local.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
            Some(quoted) => quoted_string(quoted, unicode),
            None => local.split('.').all(|atom| !atom.is_empty() && atom.chars().all(|c| atext(c, unicode))),
        }
    }

    /// Returns RFC 5321 `atext`: letters, digits and ``!#$%&'*+-/=?^_`{|}~``. RFC 6531 adds any non-ASCII character.
    fn atext(c: char, unicode: bool) -> bool {
        c.is_ascii_alphanumeric() || "!#$%&'*+-/=?^_`{|}~".contains(c) || (unicode && !c.is_ascii())
    }

    /// Returns the inside of a quoted string. That is printable ASCII except `"` and `\`. Or `\` before a printable
    /// character.
    fn quoted_string(inner: &str, unicode: bool) -> bool {
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            let ok = match c {
                '\\' => chars.next().is_some_and(|q| (' '..='~').contains(&q)),
                '"' => false,
                c => (' '..='~').contains(&c) || (unicode && !c.is_ascii()),
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// Returns RFC 5321 section 4.1.3: `[IPv4]` or `[IPv6:address]`.
    fn address_literal(domain: &str) -> bool {
        domain.strip_prefix('[').and_then(|d| d.strip_suffix(']')).is_some_and(|inner| {
            // ABNF literal text is case-insensitive (RFC 5234 section 2.3): `ipv6:` is the same tag.
            const TAG: &str = "IPv6:";
            match inner.get(..TAG.len()).filter(|t| t.eq_ignore_ascii_case(TAG)).and_then(|_| inner.get(TAG.len()..)) {
                Some(v6) => v6.parse::<std::net::Ipv6Addr>().is_ok(),
                None => inner.parse::<std::net::Ipv4Addr>().is_ok(),
            }
        })
    }

    #[cfg(test)]
    mod tests {
        use super::{Email, IdnHostname, email, hostname};

        #[test]
        fn only_a_valid_value_is_one() {
            assert_eq!(Email::try_from("ann@example.com").map(Email::into_inner), Ok("ann@example.com".to_owned()));
            assert_eq!(Email::try_from("x"), Err("`x` is not a valid email".to_owned()));
            let decoded: Result<Email, _> = serde_json::from_str("\"x\"");
            assert!(decoded.unwrap_err().to_string().contains("is not a valid email"));
            let site: IdnHostname = serde_json::from_str("\"bücher.example\"").unwrap();
            assert_eq!(serde_json::to_string(&site).unwrap(), "\"bücher.example\"", "the value as written");
        }

        /// JSON Schema Test Suite, draft 2020-12, `optional/format/email.json` and `idn-email.json`.
        #[test]
        fn email_follows_the_test_suite() {
            let valid = [
                "joe.bloggs@example.com",
                "te~st@example.com",
                "~test@example.com",
                "test~@example.com",
                "\"joe bloggs\"@example.com",
                "\"joe..bloggs\"@example.com",
                "\"joe@bloggs\"@example.com",
                "joe.bloggs@[127.0.0.1]",
                "joe.bloggs@[IPv6:::1]",
                "te.s.t@example.com",
                "user@localhost",
            ];
            let invalid = [
                "2962",
                ".test@example.com",
                "test.@example.com",
                "te..st@example.com",
                "joe.bloggs@invalid=domain.com",
                "joe.bloggs@[127.0.0.300]",
                "user@-leading-hyphen.com",
                "\"unterminated@example.com",
                "실례@실례.테스트",
            ];
            for v in valid {
                assert!(email(v, false), "valid email: {v}");
            }
            for v in invalid {
                assert!(!email(v, false), "invalid email: {v}");
            }
            assert!(email("실례@실례.테스트", true), "idn-email allows UTF-8");
            assert!(!email("2962", true));
            assert!(!email(&format!("{}@example.com", "a".repeat(65)), false), "a local part over 64 octets");
        }

        /// JSON Schema Test Suite, draft 2020-12, `optional/format/hostname.json` and `idn-hostname.json`.
        #[test]
        fn hostname_follows_the_test_suite() {
            let label = "a".repeat(63);
            for v in ["www.example.com", "xn--4gbwdl.xn--wgbh1c", "hostname", &label] {
                assert!(hostname(v, false), "valid hostname: {v}");
            }
            for v in [
                "",
                "-a-host-name-that-starts-with--",
                "not_a_valid_host_name",
                "a-vvvvvvvvvvvvvvvveeeeeeeeeeeeeeeerrrrrrrrrrrrrrrryyyyyyyyyyyyyyyy-long-host-name-component",
                "xn--X",
                "XN--aa---o47jg78q",
                "실례.테스트",
            ] {
                assert!(!hostname(v, false), "invalid hostname: {v}");
            }
            assert!(hostname("실례.테스트", true), "idn-hostname allows Unicode labels");
            assert!(!hostname("〮실례.테스트", true), "a combining mark cannot start a label");
        }

        /// The local part of `email` is ASCII. `idn-email` adds any non-ASCII character but no more ASCII.
        #[test]
        fn only_idn_email_allows_non_ascii_local_parts() {
            assert!(!email("ü@example.com", false) && email("ü@example.com", true));
            assert!(!email("a(b@example.com", true));
            assert!(!email("\"ü\"@example.com", false) && email("\"ü\"@example.com", true));
            assert!(!email("\"a\tb\"@example.com", true));
        }

        /// RFC 5892 appendix A: a middle dot stands between two `l`. A katakana middle dot needs Japanese in its label.
        #[test]
        fn idn_hostname_follows_the_context_rules() {
            assert!(hostname("l\u{B7}l", true) && !hostname("a\u{B7}b", true));
            assert!(hostname("\u{30A2}\u{30FB}\u{30A2}", true) && !hostname("a\u{30FB}b", true));
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn nan_is_no_number_whatever_the_bounds() {
        let mut out = vec![];
        super::check_range(&mut out, &super::InstancePath::Root, f64::NAN, [Some(0.0), Some(1.0), None, None]);
        super::check_range(&mut out, &super::InstancePath::Root, f32::INFINITY, [Some(0.0), None, None, None]);
        super::check_range(&mut out, &super::InstancePath::Root, 5_u8, [Some(0), None, None, None]);
        let messages: Vec<&str> = out.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(messages, ["is not a number", "is not a number"]);
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
    fn floats_are_equal_by_value_also_beyond_the_integers() {
        use serde_json::json;
        assert!(super::json_equal(&json!(-0.0), &json!(0)));
        assert!(super::json_equal(&json!(3.0), &json!(3)));
        assert!(super::json_equal(&json!(1e20), &json!(100_000_000_000_000_000_000.0)));
        assert!(!super::json_equal(&json!(1e20), &json!(2e20)));
        assert!(!super::json_equal(&json!(0.5), &json!(0.25)));
    }

    #[test]
    fn unique_items_compares_whole_arrays() {
        use serde_json::json;
        let mut out = vec![];
        super::check_unique(
            &mut out,
            &super::InstancePath::Root,
            &[json!([1, 2]), json!([12]), json!({"a": 1, "b": 2})],
        );
        super::check_unique(
            &mut out,
            &super::InstancePath::Root,
            &[json!({"a": 1, "b": [1, 2]}), json!({"b": [1, 2], "a": 1.0})],
        );
        let messages: Vec<&str> = out.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(messages, ["items must be unique"], "only the second list has a duplicate");
    }

    #[test]
    fn decimal128_holds_34_significant_digits_within_its_exponent_range() {
        for ok in [
            "0",
            "-0.000",
            "1.5",
            "1.50000",
            "1234567890123456789012345678901234",
            "1e6144",
            "9.999e6144",
            "1e-6176",
            "12E+3",
            "100000000000000000000000000000000000000000",
        ] {
            assert_eq!(super::decimal128(ok), Ok(()), "{ok}");
        }
        assert_eq!(
            super::decimal128("12345678901234567890123456789012345"),
            Err("has 35 significant digits, more than the 34 of decimal128".to_owned())
        );
        for out in ["1e6145", "1e-6177", "1e99999999999999999999"] {
            assert_eq!(super::decimal128(out), Err("is out of the range of decimal128".to_owned()), "{out}");
        }
        for not in ["", "-", ".5", "1.", "1e", "1e+", "+1", "1,5", "0x10"] {
            assert_eq!(super::decimal128(not), Err("is not a decimal".to_owned()), "{not:?}");
        }
        let mut out = vec![];
        super::check_decimal128(&mut out, &super::InstancePath::Root, "1e6145");
        assert_eq!(out[0].message, "`1e6145` is out of the range of decimal128");
    }

    #[test]
    fn check_records_its_message_only_when_not_ok() {
        let mut out = vec![];
        super::check(&mut out, &super::InstancePath::Root, true, "fine");
        super::check(&mut out, &super::InstancePath::Root, false, "must match ^a");
        let messages: Vec<&str> = out.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(messages, ["must match ^a"]);
    }

    #[test]
    fn a_map_validates_each_value_under_its_key() {
        struct Bad(&'static str);
        impl super::Validate for Bad {
            fn validate(&self, path: &super::InstancePath<'_>, out: &mut Vec<super::Violation>) {
                super::violation(out, path, self.0);
            }
        }
        let map = std::collections::BTreeMap::from([("x".to_owned(), Bad("bad"))]);
        let mut out = vec![];
        super::Validate::validate(&map, &super::InstancePath::Root, &mut out);
        let found: Vec<String> = out.iter().map(|v| format!("{} {}", v.path, v.message)).collect();
        assert_eq!(found, ["/x bad"]);
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

    #[test]
    fn formats_are_checked_by_name() {
        let wrong = |value: &str, format: &str| {
            let mut out = vec![];
            super::check_format(&mut out, &super::InstancePath::Root, value, format);
            !out.is_empty()
        };
        assert!(wrong("x", "ipv6") && !wrong("::1", "ipv6"));
        assert!(wrong("x", "uri") && !wrong("https://example.com", "uri"));
        #[cfg(feature = "ipnet")]
        assert!(wrong("::1", "ipv6-cidr") && !wrong("::1/128", "ipv6-cidr"));
        #[cfg(feature = "idn")]
        assert!(!wrong("ann@example.com", "email"));
    }

    /// The address of a CIDR block is checked as `ipv4` and `ipv6` check one.
    #[cfg(feature = "ipnet")]
    #[test]
    fn a_cidr_block_has_a_valid_address() {
        let wrong = |value: &str, format: &str| {
            let mut out = vec![];
            super::check_format(&mut out, &super::InstancePath::Root, value, format);
            !out.is_empty()
        };
        assert!(!wrong("192.168.0.0/24", "ipv4-cidr") && !wrong("0.0.0.0/0", "ipv4-cidr"));
        for octal in ["01.2.3.4/8", "192.168.000.0/24", "192.168.0.00/24"] {
            assert!(wrong(octal, "ipv4-cidr"), "{octal}: a leading zero reads as octal in some parsers");
        }
        assert!(!wrong("2001:db8::/32", "ipv6-cidr") && !wrong("::ffff:192.168.0.0/120", "ipv6-cidr"));
        for nine in ["1:2:3:4:5:6:7::8/64", "1:2:3:4::5:6:7:8/64", "::1:2:3:4:5:6:7:8/128"] {
            assert!(wrong(nine, "ipv6-cidr"), "{nine}: `::` stands for at least one group");
        }
    }

    /// `^(a+)+$` takes exponential time on a long value that almost matches. Past `maxLength` it does not run: the
    /// value is wrong already. Run, 30,001 characters would not finish.
    #[cfg(feature = "regress")]
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
