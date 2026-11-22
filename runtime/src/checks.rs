//! Schema constraints and the violations they report.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

pub use crate::violation::Violation;

/// Where a value is in a request. It is a JSON pointer that is formatted only on a violation.
#[derive(Debug, Clone, Copy)]
pub enum InstanceLocation<'a> {
    /// The request itself (`""`).
    Root,
    /// A property, parameter location or name under a parent.
    Key(&'a Self, &'a str),
    /// An array position under a parent.
    Index(&'a Self, usize),
}

impl fmt::Display for InstanceLocation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => Ok(()),
            Self::Key(parent, key) => write!(f, "{parent}/{}", key.replace('~', "~0").replace('/', "~1")),
            Self::Index(parent, i) => write!(f, "{parent}/{i}"),
        }
    }
}

/// Returns the first violation as the error of a checked type's `try_from`: its message, after its pointer when it is
/// inside the value (`/2: at least 1 characters required, found 0`).
///
/// # Errors
///
/// `violations` is not empty.
pub fn first(violations: Vec<Violation>) -> Result<(), String> {
    match violations.into_iter().next() {
        None => Ok(()),
        Some(v) if v.instance_location.is_empty() => Err(v.message),
        Some(v) => Err(format!("{}: {}", v.instance_location, v.message)),
    }
}

/// Serializes the object `v` with the property `property` set to `tag`: a variant of a tagged union whose struct does
/// not carry its tag.
///
/// # Errors
/// `v` does not serialize.
pub fn serialize_tagged<T: Serialize + ?Sized, S: serde::Serializer>(
    v: &T,
    property: &str,
    tag: &str,
    s: S,
) -> Result<S::Ok, S::Error> {
    let mut j = serde_json::to_value(v).map_err(serde::ser::Error::custom)?;
    if let Value::Object(m) = &mut j {
        m.insert(property.to_owned(), Value::String(tag.to_owned()));
    }
    Serialize::serialize(&j, s)
}

/// Returns the error of `value`, which is none of `allowed`.
#[must_use]
pub fn not_one_of(value: &str, allowed: &[&str]) -> crate::BuildError {
    let allowed: Vec<String> = allowed.iter().map(|a| format!("`{a}`")).collect();
    crate::BuildError::value(format!("`{value}` is not one of {}", allowed.join(", ")))
}

/// Records a violation.
pub fn violation(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, message: impl Into<String>) {
    out.push(Violation { instance_location: location.to_string(), message: message.into() });
}

/// Checks a string `format` the crate has no library type for: what parsing into that type would refuse.
///
/// The runtime checks each format by its own grammar: `uuid`, the dates and times, `uri`, the CIDR blocks, `decimal`
/// and the rest. `regex` needs the feature `regress`, and mail addresses and host names the feature `idn`. A format
/// that it does not know passes.
#[cfg_attr(
    feature = "idn",
    doc = "\nThose types are [`EmailStr`](crate::EmailStr) and [`HostnameStr`](crate::HostnameStr)."
)]
pub fn check_format(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, value: &str, format: &str) {
    if !is_format(value, format) {
        violation(out, location, format!("is not a valid {format}"));
    }
}

/// Is `value` of `format` as [`check_format`] checks it? A format it does not know is.
pub(crate) fn is_format(value: &str, format: &str) -> bool {
    match format {
        "uuid" => uuid(value),
        "date" => crate::format_checks::is_date(value),
        "time" => crate::full_time::is_full_time(value),
        "time-local" => crate::format_checks::is_time_local(value),
        "date-time" => crate::format_checks::is_date_time(value),
        "date-time-local" => crate::format_checks::is_date_time_local(value),
        "ipv4" => crate::format_checks::is_ipv4(value),
        "ipv6" => crate::format_checks::is_ipv6(value),
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
        "ipv4-cidr" => cidr(value, crate::format_checks::is_ipv4, IPV4_BITS, IPV4_PREFIX_DIGITS),
        "ipv6-cidr" => cidr(value, crate::format_checks::is_ipv6, IPV6_BITS, IPV6_PREFIX_DIGITS),
        _ => true,
    }
}

/// The bits of an IPv4 address: the longest prefix of an `ipv4-cidr`.
const IPV4_BITS: u8 = 32;
/// The bits of an IPv6 address: the longest prefix of an `ipv6-cidr`.
const IPV6_BITS: u8 = 128;
/// The most digits of an IPv4 prefix length. A leading zero counts: `/08` is one, `/008` is not.
const IPV4_PREFIX_DIGITS: usize = 2;
/// The most digits of an IPv6 prefix length: `/000` is one, `/0128` is not.
const IPV6_PREFIX_DIGITS: usize = 3;

/// Is `value` a CIDR block: an `address`, a `/` and a prefix length of at most `digits` ASCII digits and at most
/// `bits`? The address is checked as `ipv4` and `ipv6` check one, which refuse a leading zero (`01.2.3.4`) and a `::`
/// that stands for no group (`1:2:3:4:5:6:7::8`). The prefix parses as the networking libraries parse it.
fn cidr(value: &str, address: fn(&str) -> bool, bits: u8, digits: usize) -> bool {
    let Some((a, prefix)) = value.split_once('/') else { return false };
    address(a)
        && (1..=digits).contains(&prefix.len())
        && crate::exact::all_digits(prefix)
        && prefix.parse::<u8>().is_ok_and(|p| p <= bits)
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

/// The significant digits that IEEE 754 decimal128 represents.
const DECIMAL128_DIGITS: usize = 34;
/// The smallest exponent of the last digit that decimal128 represents: its smallest value is `1e-6176`.
const DECIMAL128_MIN_EXPONENT: i64 = -6176;
/// The largest exponent of the first digit that decimal128 represents: its largest values are below `1e6145`.
const DECIMAL128_MAX_EXPONENT: i64 = 6144;

/// Checks that `s` is a decimal number that IEEE 754 decimal128 represents exactly: an optional `-`, digits with an
/// optional fraction and an optional exponent, at most 34 significant digits, within its exponent range. Zeros before
/// the first and after the last nonzero digit are not significant: `1.50000` counts as `1.5`.
///
/// # Errors
///
/// Why it does not fit.
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
    let count = crate::exact::signed;
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

/// Checks that the decimal number `text` is one that IEEE 754 decimal128 represents exactly (`format: decimal128`).
pub fn check_decimal128(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, text: &str) {
    if let Err(why) = decimal128(text) {
        violation(out, location, format!("`{text}` {why}"));
    }
}

/// Records `message` at `location` unless `ok`.
pub fn check(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, ok: bool, message: &str) {
    if !ok {
        violation(out, location, message);
    }
}

/// Checks `required` of an object whose names `has` looks up: each of `names` is one of its properties.
pub fn check_required(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    has: impl Fn(&str) -> bool,
    names: &[&str],
) {
    for name in names.iter().filter(|name| !has(name)) {
        violation(out, &InstanceLocation::Key(location, name), "is required");
    }
}

/// Checks `minLength`/`maxLength`, `minItems`/`maxItems`, `minProperties`/`maxProperties`.
pub fn check_len(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    len: usize,
    min: Option<u64>,
    max: Option<u64>,
    unit: &str,
) {
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    if let Some(m) = min.filter(|m| len < *m) {
        violation(out, location, format!("at least {m} {unit} required, found {len}"));
    }
    if let Some(m) = max.filter(|m| len > *m) {
        violation(out, location, format!("at most {m} {unit} allowed, found {len}"));
    }
}

/// A number as JSON has one. An integer always is one. A float is one only when it is finite.
pub trait JsonNumber {
    /// Is it a JSON number (RFC 8259 section 6)? `NaN` and the infinities are not.
    fn is_json_number(&self) -> bool;

    /// Is it a multiple of the decimal text `of`, decided without its own text? `None` when the text decides.
    fn multiple_of(&self, of: &str) -> Option<bool>;
}

macro_rules! json_number {
    ($($t:ty: $is:expr, $multiple:expr);* $(;)?) => {$(
        impl JsonNumber for $t {
            fn is_json_number(&self) -> bool {
                $is(self)
            }

            fn multiple_of(&self, of: &str) -> Option<bool> {
                $multiple(*self, of)
            }
        }
    )*};
}

/// An integer as an `i128`, which fits every one.
macro_rules! whole {
    ($t:ty) => {
        |n: $t, of: &str| crate::exact::int_is_multiple(i128::from(n), of)
    };
}

json_number!(
    i8: |_| true, whole!(i8);
    i16: |_| true, whole!(i16);
    i32: |_| true, whole!(i32);
    i64: |_| true, whole!(i64);
    u8: |_| true, whole!(u8);
    u16: |_| true, whole!(u16);
    u32: |_| true, whole!(u32);
    u64: |_| true, whole!(u64);
    f32: |n: &f32| n.is_finite(), |_, _| None;
    f64: |n: &f64| n.is_finite(), crate::exact::f64_is_multiple;
);

/// The bounds of a number: `minimum`, `maximum`, `exclusiveMinimum` and `exclusiveMaximum`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds<T> {
    /// `minimum`
    pub min: Option<T>,
    /// `maximum`
    pub max: Option<T>,
    /// `exclusiveMinimum`
    pub exclusive_min: Option<T>,
    /// `exclusiveMaximum`
    pub exclusive_max: Option<T>,
}

impl<T> Bounds<T> {
    /// No bound: the start of `Bounds { max: Some(9), ..Bounds::NONE }`.
    pub const NONE: Self = Self { min: None, max: None, exclusive_min: None, exclusive_max: None };
}

/// Checks that `s` matches `re`, with the pattern in the message.
#[cfg(feature = "regress")]
pub fn check_pattern(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, re: &crate::pattern::Pattern, s: &str) {
    if !crate::pattern::pattern_matches(re, s) {
        violation(out, location, format!("does not match `{}`", crate::pattern::pattern_text(re)));
    }
}

/// Checks `minimum`, `maximum`, `exclusiveMinimum`, `exclusiveMaximum`.
pub fn check_range<T: PartialOrd + fmt::Display + Copy + JsonNumber>(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    v: T,
    bounds: Bounds<T>,
) {
    let Bounds { min, max, exclusive_min: xmin, exclusive_max: xmax } = bounds;
    // `NaN` compares false with every bound. An infinity passes the bound on its own side.
    if !v.is_json_number() {
        violation(out, location, "is not a number");
        return;
    }
    if let Some(m) = min.filter(|m| v < *m) {
        violation(out, location, format!("{v} is less than the minimum {m}"));
    }
    if let Some(m) = max.filter(|m| v > *m) {
        violation(out, location, format!("{v} is greater than the maximum {m}"));
    }
    if let Some(m) = xmin.filter(|m| v <= *m) {
        violation(out, location, format!("{v} must be greater than {m}"));
    }
    if let Some(m) = xmax.filter(|m| v >= *m) {
        violation(out, location, format!("{v} must be less than {m}"));
    }
}

/// Checks that the number `value` is a multiple of `of`, exactly: both are parsed as their decimal text.
///
/// An `f64` is its shortest text, the one that it decodes from. The text is in exponent form, so it fits on the stack:
/// `1e300` is five bytes and not 301.
pub fn check_multiple<T: fmt::LowerExp + JsonNumber + Copy>(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    value: T,
    of: &str,
) {
    if !value.is_json_number() {
        return;
    }
    match value.multiple_of(of) {
        Some(true) => {}
        Some(false) => violation(out, location, format!("is not a multiple of {of}")),
        None => {
            let mut text = StackText::default();
            if fmt::Write::write_fmt(&mut text, format_args!("{value:e}")).is_ok() {
                check_multiple_text(out, location, text.as_str(), of);
            } else {
                check_multiple_text(out, location, &format!("{value:e}"), of);
            }
        }
    }
}

/// Text on the stack, as long as the longest number in exponent form: `-9.223372036854775808e18` is 24 bytes and an
/// `f64` at most 24 too. Longer text fails to write.
#[derive(Default)]
struct StackText {
    bytes: [u8; 32],
    len: usize,
}

impl StackText {
    fn as_str(&self) -> &str {
        std::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl fmt::Write for StackText {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).filter(|end| *end <= self.bytes.len()).ok_or(fmt::Error)?;
        self.bytes[self.len..end].copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}

/// Checks that the decimal text `value` is a multiple of `of`, exactly. Text that is not decimal, or an `of` that is
/// not positive, is no multiple: validation refuses such an `of` before generation.
pub fn check_multiple_text(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, value: &str, of: &str) {
    if !crate::exact::is_multiple(value, of).unwrap_or(false) {
        violation(out, location, format!("is not a multiple of {of}"));
    }
}

/// Checks `minimum`, `maximum`, `exclusiveMinimum` and `exclusiveMaximum` of the decimal text `value` exactly: a
/// decimal keeps more digits than an `f64` compares.
pub fn check_decimal_range(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    value: &str,
    bounds: Bounds<&str>,
) {
    use std::cmp::Ordering::{Greater, Less};
    let Bounds { min, max, exclusive_min: xmin, exclusive_max: xmax } = bounds;
    let cmp = |b: &str| crate::exact::compare(value, b);
    if let Some(m) = min.filter(|m| cmp(m) == Some(Less)) {
        violation(out, location, format!("{value} is less than the minimum {m}"));
    }
    if let Some(m) = max.filter(|m| cmp(m) == Some(Greater)) {
        violation(out, location, format!("{value} is greater than the maximum {m}"));
    }
    if let Some(m) = xmin.filter(|m| cmp(m) != Some(Greater)) {
        violation(out, location, format!("{value} must be greater than {m}"));
    }
    if let Some(m) = xmax.filter(|m| cmp(m) != Some(Less)) {
        violation(out, location, format!("{value} must be less than {m}"));
    }
}

/// Whether two of `items` are equal as JSON Schema compares instances.
///
/// The canonical JSON of each item is sorted. Then neighbours are compared. This is O(n log n). A comparison of every
/// pair would let one request array cost O(n^2).
pub fn has_duplicates<T: Serialize>(items: &[T]) -> bool {
    repeats(
        items
            .iter()
            .enumerate()
            // Never equal to another item: a key no JSON text starts with.
            .map(|(i, x)| unique_key(x).unwrap_or_else(|| ["\u{0}", itoa::Buffer::new().format(i)].concat()))
            .collect(),
    )
}

/// Whether two of `keys` are equal: sorted, then neighbours compared.
fn repeats(mut keys: Vec<String>) -> bool {
    keys.sort_unstable();
    keys.windows(2).any(|w| w.first() == w.last())
}

/// Returns the canonical JSON of `x`: equal for items that `uniqueItems` treats as equal. `None` when it does not
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

/// Returns JSON Schema equality (2020-12 core 4.2.2): numbers by value, so `-2` equals `-2.0`. Objects ignore key
/// order.
#[must_use]
pub fn json_equal(a: &Value, b: &Value) -> bool {
    let (mut x, mut y) = (String::new(), String::new());
    canonical(a, &mut x);
    canonical(b, &mut y);
    x == y
}

/// Returns [`json_equal`] of a buffered value and a JSON value.
#[must_use]
pub fn content_equal(a: &crate::tagged::Content<'_>, b: &Value) -> bool {
    let (mut x, mut y) = (String::new(), String::new());
    canonical_content(a, &mut x);
    canonical(b, &mut y);
    x == y
}

/// Writes `v` as [`canonical`] does.
fn canonical_content(v: &crate::tagged::Content<'_>, out: &mut String) {
    use crate::tagged::Content;
    match v {
        Content::Seq(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_content(x, out);
            }
            out.push(']');
        }
        Content::Map(m) => {
            let mut members: Vec<_> = m.iter().collect();
            members.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
            out.push('{');
            for (i, (k, x)) in members.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.to_string()).to_string());
                out.push(':');
                canonical_content(x, out);
            }
            out.push('}');
        }
        Content::U64(u) => out.push_str(itoa::Buffer::new().format(*u)),
        Content::I64(i) => out.push_str(itoa::Buffer::new().format(*i)),
        Content::F64(f) => out.push_str(&canonical_float(*f)),
        Content::Null => out.push_str("null"),
        Content::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Content::Str(s) => out.push_str(&Value::String(s.to_string()).to_string()),
        Content::Raw(text) => canonical(&serde_json::from_str(text).unwrap_or(Value::Null), out),
    }
}

/// Checks `uniqueItems`.
pub fn check_unique<T: Serialize>(out: &mut Vec<Violation>, location: &InstanceLocation<'_>, items: &[T]) {
    if has_duplicates(items) {
        violation(out, location, "items must be unique");
    }
}

/// Checks `uniqueItems` of the items of a buffered array, as [`check_unique`] does.
pub fn check_unique_content(
    out: &mut Vec<Violation>,
    location: &InstanceLocation<'_>,
    items: &[crate::tagged::Content<'_>],
) {
    let keys = items.iter().map(|x| {
        let mut key = String::new();
        canonical_content(x, &mut key);
        key
    });
    if repeats(keys.collect()) {
        violation(out, location, "items must be unique");
    }
}

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

    /// RFC 5321 section 4.5.3.1.1: the longest local part in octets.
    const LOCAL: usize = 64;
    /// RFC 5321 section 4.5.3.1.3: the longest path is 256 octets with its angle brackets.
    const ADDRESS: usize = 254;

    /// Returns a host name with RFC 1123 labels (`hostname`) or IDNA2008 labels (`idn-hostname`). In ASCII each label
    /// contains 1 to 63 octets of letters, digits and inner hyphens. The name contains 253 in all. A punycode label
    /// (`xn--`) must decode to a valid one.
    fn hostname(value: &str, unicode: bool) -> bool {
        plain_name(value) || mapped_name(value, unicode)
    }

    /// Returns a host name as [`hostname`] parses it, through UTS 46 mapping.
    fn mapped_name(value: &str, unicode: bool) -> bool {
        // The labels as UTS 46 maps them: `\u{3002}` separates two labels and a zero width space is dropped.
        let (mapped, mapping) = Uts46::new().to_unicode(value.as_bytes(), AsciiDenyList::STD3, Hyphens::Check);
        (unicode || value.is_ascii())
            && !value.is_empty()
            && mapping.is_ok()
            && Uts46::new().to_ascii(value.as_bytes(), AsciiDenyList::STD3, Hyphens::Check, DnsLength::Verify).is_ok()
            && mapped.trim_end_matches('.').split('.').all(idna2008_label)
    }

    /// The most octets of a label (RFC 1035 section 2.3.4).
    const LABEL: usize = 63;
    /// The most octets of a host name without its root dot.
    const NAME: usize = 253;

    /// Returns a host name of plain ASCII labels that the full check accepts without parsing it again: letters, digits
    /// and inner hyphens, no label an A-label or with `--` where UTS 46 reserves it. UTS 46 maps such a name to itself
    /// and lower case. Any other name gets the full check, so a `false` here refuses nothing.
    fn plain_name(value: &str) -> bool {
        (1..=NAME).contains(&value.len())
            && value.split('.').all(|label| {
                let b = label.as_bytes();
                (1..=LABEL).contains(&b.len())
                    && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
                    && b.first() != Some(&b'-')
                    && b.last() != Some(&b'-')
                    && b.get(2..ACE.len()) != Some(b"--")
            })
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
                c => letter_digit(c),
            }
        })
    }

    /// Returns RFC 5892 section 2.1 `LetterDigits` (letters, decimal digits and marks), the hyphen and the PVALID
    /// exceptions of section 2.6. The joiners of appendix A.1 and A.2 stay to UTS 46. Punctuation, symbols and spaces
    /// are DISALLOWED: `xn--7a` decodes to an inverted exclamation mark.
    fn letter_digit(c: char) -> bool {
        use icu_properties::props::GeneralCategory as Gc;
        let category = icu_properties::CodePointMapData::<Gc>::new().get(c);
        matches!(c, '-' | '\u{200C}' | '\u{200D}' | '\u{6FD}' | '\u{6FE}' | '\u{F0B}' | '\u{3007}')
            || matches!(
                category,
                Gc::LowercaseLetter
                    | Gc::UppercaseLetter
                    | Gc::OtherLetter
                    | Gc::ModifierLetter
                    | Gc::DecimalNumber
                    | Gc::NonspacingMark
                    | Gc::SpacingMark
            )
    }

    /// Returns an RFC 5321 Mailbox (`email`) or an RFC 6531 one that may contain UTF-8 (`idn-email`). It is a dot-atom
    /// or a quoted local part, then `@`, then a host name or an address literal (`[192.0.2.1]` or `[IPv6:::1]`).
    fn email(value: &str, unicode: bool) -> bool {
        // A quoted local part may contain `@`; the domain cannot.
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
        use super::{LABEL, email, hostname, mapped_name, plain_name};
        use crate::{EmailStr, IdnHostnameStr};

        #[test]
        fn only_a_valid_value_is_one() {
            assert_eq!(
                EmailStr::try_from("ann@example.com").map(EmailStr::into_inner),
                Ok("ann@example.com".to_owned())
            );
            assert_eq!(EmailStr::try_from("x"), Err(crate::BuildError::value("`x` is not a valid email".to_owned())));
            let decoded: Result<EmailStr, _> = serde_json::from_str("\"x\"");
            assert!(decoded.unwrap_err().to_string().contains("is not a valid email"));
            let site: IdnHostnameStr = serde_json::from_str("\"bücher.example\"").unwrap();
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

        /// Every name the plain path accepts, the mapped path accepts too: names of up to six octets of letters,
        /// digits, hyphens and dots, and labels and names at their longest.
        #[test]
        fn a_plain_name_is_one_that_mapping_takes() {
            const ALPHABET: [u8; 5] = *b"aZ0-.";
            let mut names: Vec<String> = vec![];
            for len in 1..=6_u32 {
                for i in 0..ALPHABET.len().pow(len) {
                    let mut i = i;
                    let name = (0..len).map(|_| {
                        let c = ALPHABET[i % ALPHABET.len()];
                        i /= ALPHABET.len();
                        char::from(c)
                    });
                    names.push(name.collect());
                }
            }
            let label = "a".repeat(LABEL);
            names.extend([label.clone(), format!("{label}a"), format!("{label}.{label}.{label}.{}", "a".repeat(61))]);
            names.extend([
                "xn--4gbwdl".to_owned(),
                "ab--c".to_owned(),
                "abc--d".to_owned(),
                format!("{label}.{label}.{label}.{label}"),
            ]);
            let mut plain = 0;
            for name in &names {
                if plain_name(name) {
                    plain += 1;
                    assert!(mapped_name(name, false) && mapped_name(name, true), "plain but not mapped: {name}");
                }
            }
            assert!(plain > 1000, "the plain path accepted {plain} names");
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

        /// RFC 5892 appendix A: a middle dot stands between two `l`, a keraia before a Greek letter. A katakana middle
        /// dot needs Japanese in its label.
        #[test]
        fn idn_hostname_follows_the_context_rules() {
            assert!(hostname("l\u{B7}l", true) && !hostname("a\u{B7}b", true) && !hostname("l\u{B7}a", true));
            assert!(hostname("\u{3B1}\u{375}\u{3B2}", true) && !hostname("\u{3B1}\u{375}\u{375}\u{3B2}", true));
            assert!(hostname("\u{30A2}\u{30FB}\u{30A2}", true) && !hostname("a\u{30FB}b", true));
        }

        /// RFC 5892 section 2.1: a label contains letters, digits and marks. `xn--7a` is an inverted exclamation mark.
        #[test]
        fn an_idn_label_holds_letters_digits_and_marks() {
            assert!(!hostname("xn--7a", true) && !hostname("a\u{A1}b", true));
            assert!(hostname("b\u{FC}cher", true) && hostname("xn--bcher-kva", true) && hostname("a-b", true));
            assert!(hostname("\u{915}\u{94D}\u{200D}", true), "a joiner after a virama stays valid");
            assert!(hostname("a\u{3002}b", true) && hostname("a\u{200B}b", true), "UTS 46 maps these first");
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "regress")]
    #[test]
    fn a_pattern_names_itself_in_its_message() {
        static RE: crate::pattern::Pattern = crate::pattern::Pattern::new("^[a-z]+$");
        let mut out = vec![];
        super::check_pattern(&mut out, &super::InstanceLocation::Root, &RE, "ABC");
        assert_eq!(out.first().map(|v| v.message.as_str()), Some("does not match `^[a-z]+$`"));
    }

    #[test]
    fn nan_is_no_number_whatever_the_bounds() {
        let mut out = vec![];
        let root = &super::InstanceLocation::Root;
        super::check_range(
            &mut out,
            root,
            f64::NAN,
            super::Bounds { min: Some(0.0), max: Some(1.0), ..super::Bounds::NONE },
        );
        super::check_range(&mut out, root, f32::INFINITY, super::Bounds { min: Some(0.0), ..super::Bounds::NONE });
        super::check_range(&mut out, root, 5_u8, super::Bounds { min: Some(0), max: Some(5), ..super::Bounds::NONE });
        let messages: Vec<&str> = out.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(messages, ["is not a number", "is not a number"]);
    }

    /// A buffered value equals a JSON value as two JSON values do: items in order, members in any.
    #[test]
    fn a_buffered_value_equals_by_json_equality() {
        use serde_json::json;
        let equal = |text: &str, v: &serde_json::Value| {
            let content: crate::tagged::Content<'_> = serde_json::from_str(text).unwrap();
            super::content_equal(&content, v)
        };
        assert!(equal(r#"{"b": [1, 23], "a": -2.0}"#, &json!({"a": -2, "b": [1, 23]})));
        assert!(!equal("[1, 23]", &json!([12, 3])) && !equal("[1, 2]", &json!([1])));
        assert!(!equal(r#"{"a": 1}"#, &json!({"a": 2})));
    }

    /// Each missing name is a violation at its own pointer, escaped as JSON Pointer escapes it.
    #[test]
    fn each_missing_name_is_required_at_its_pointer() {
        let mut out = vec![];
        let at = super::InstanceLocation::Index(&super::InstanceLocation::Root, 3);
        super::check_required(&mut out, &at, |n| n == "a", &["a", "b/c", "d~e"]);
        let found: Vec<(&str, &str)> = out.iter().map(|v| (v.instance_location.as_str(), v.message.as_str())).collect();
        assert_eq!(found, [("/3/b~1c", "is required"), ("/3/d~0e", "is required")]);
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
            &super::InstanceLocation::Root,
            &[json!([1, 2]), json!([12]), json!({"a": 1, "b": 2})],
        );
        super::check_unique(
            &mut out,
            &super::InstanceLocation::Root,
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
        super::check_decimal128(&mut out, &super::InstanceLocation::Root, "1e6145");
        assert_eq!(out[0].message, "`1e6145` is out of the range of decimal128");
    }

    #[test]
    fn check_records_its_message_only_when_not_ok() {
        let mut out = vec![];
        super::check(&mut out, &super::InstanceLocation::Root, true, "fine");
        super::check(&mut out, &super::InstanceLocation::Root, false, "must match ^a");
        let messages: Vec<&str> = out.iter().map(|v| v.message.as_str()).collect();
        assert_eq!(messages, ["must match ^a"]);
    }

    #[test]
    fn the_first_violation_is_the_error_with_its_pointer_inside_the_value() {
        assert_eq!(super::first(vec![]), Ok(()));
        let at = |location: &str, message: &str| super::Violation {
            instance_location: location.to_owned(),
            message: message.to_owned(),
        };
        assert_eq!(super::first(vec![at("", "too short"), at("/1", "later")]), Err("too short".to_owned()));
        assert_eq!(super::first(vec![at("/2", "not a uuid")]), Err("/2: not a uuid".to_owned()));
    }

    #[test]
    fn formats_are_checked_by_name() {
        let wrong = |value: &str, format: &str| {
            let mut out = vec![];
            super::check_format(&mut out, &super::InstanceLocation::Root, value, format);
            !out.is_empty()
        };
        assert!(wrong("x", "ipv6") && !wrong("::1", "ipv6"));
        assert!(wrong("1.2.3", "ipv4") && !wrong("1.2.3.4", "ipv4"));
        assert!(wrong("x", "uri") && !wrong("https://example.com", "uri"));
        assert!(wrong("::1", "ipv6-cidr") && !wrong("::1/128", "ipv6-cidr"));
        #[cfg(feature = "idn")]
        {
            assert!(!wrong("ann@example.com", "email") && !wrong("\u{FC}@b\u{FC}cher.example", "idn-email"));
            assert!(wrong("b\u{FC}cher.example", "hostname") && !wrong("b\u{FC}cher.example", "idn-hostname"));
        }
    }

    /// The address of a CIDR block is checked as `ipv4` and `ipv6` check one, and its prefix length as digits within
    /// the address's bits.
    #[test]
    fn a_cidr_block_has_a_valid_address() {
        let wrong = |value: &str, format: &str| {
            let mut out = vec![];
            super::check_format(&mut out, &super::InstanceLocation::Root, value, format);
            !out.is_empty()
        };
        assert!(!wrong("192.168.0.0/24", "ipv4-cidr") && !wrong("0.0.0.0/0", "ipv4-cidr"));
        for octal in ["01.2.3.4/8", "192.168.000.0/24", "192.168.0.00/24"] {
            assert!(wrong(octal, "ipv4-cidr"), "{octal}: a leading zero parses as octal in some parsers");
        }
        assert!(!wrong("2001:db8::/32", "ipv6-cidr") && !wrong("::ffff:192.168.0.0/120", "ipv6-cidr"));
        for nine in ["1:2:3:4:5:6:7::8/64", "1:2:3:4::5:6:7:8/64", "::1:2:3:4:5:6:7:8/128"] {
            assert!(wrong(nine, "ipv6-cidr"), "{nine}: `::` stands for at least one group");
        }
        // The prefix length: digits within the bits of the address, as the `ipnet` crate parses them.
        for (block, format, ok) in [
            ("10.0.0.0/8", "ipv4-cidr", true),
            ("10.0.0.1/8", "ipv4-cidr", true),
            ("10.0.0.0/32", "ipv4-cidr", true),
            ("10.0.0.0/08", "ipv4-cidr", true),
            ("1.2.3.4/0", "ipv4-cidr", true),
            ("10.0.0.0/33", "ipv4-cidr", false),
            ("10.0.0.0/+8", "ipv4-cidr", false),
            ("10.0.0.0/", "ipv4-cidr", false),
            ("10.0.0.0", "ipv4-cidr", false),
            ("10.0.0.0/8/8", "ipv4-cidr", false),
            ("10.0.0.0/ 8", "ipv4-cidr", false),
            ("10.0.0.0/008", "ipv4-cidr", false),
            ("::1/128", "ipv6-cidr", true),
            ("::/0", "ipv6-cidr", true),
            ("::/00", "ipv6-cidr", true),
            ("::1/129", "ipv6-cidr", false),
            ("::1/0128", "ipv6-cidr", false),
            ("x", "ipv6-cidr", false),
        ] {
            assert_eq!(!wrong(block, format), ok, "{block} as {format}");
        }
    }
}
