//! Schema to type graph.
//!
//! Works on the bundled 3.2 document as a JSON value. Every named type is
//! a `TypeDef`; field and parameter types are `Ty` expressions over them.

mod build;
mod cycles;
mod rewrite;

use build::kind_in;
pub(crate) use build::{Builder, doc_of, literal_default, text_of, with_external_docs};
pub(crate) use build::{binary_media, is_annotation_only};
pub(crate) use rewrite::request_view;

pub(crate) use cycles::{all_refs, def_types};
use cycles::{direction, map_kind, map_named};

use std::collections::BTreeSet;

use serde_json::Value;

use super::names::{Name, unique};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
/// Index of a named type in a `TypeSpace`.
pub(crate) struct TypeId(pub usize);

#[derive(Debug, Clone, PartialEq)]
/// A type expression. It is what a value of a schema holds in terms that every target language can spell.
pub(crate) enum Ty {
    /// A boolean.
    Bool,
    /// A signed 8-bit integer (`format: int8`).
    I8,
    /// A signed 16-bit integer (`format: int16`).
    I16,
    /// A signed 32-bit integer (`format: int32`).
    I32,
    /// A signed 64-bit integer: any other `integer`.
    I64,
    /// An unsigned 8-bit integer (`format: uint8`).
    U8,
    /// An unsigned 16-bit integer (`format: uint16`).
    U16,
    /// An unsigned 32-bit integer (`format: uint32`).
    U32,
    /// An unsigned 64-bit integer (`format: uint64`).
    U64,
    /// A 32-bit float (`format: float`).
    F32,
    /// A 64-bit float: any other `number`.
    F64,
    /// A number with `format: decimal` or `decimal128` that keeps every digit.
    Decimal,
    /// A string.
    String,
    /// Raw bytes (non-JSON bodies).
    Bytes,
    /// Binary data inside a structure (`format: binary` or `contentEncoding:
    /// base64`). Raw in multipart parts and base64 in JSON.
    Binary,
    /// A string with a well-known `format` (`uuid`, `date-time`, `date`). The emitter maps it to a library
    /// type that the target has or to a string.
    Format(Format),
    /// Any JSON value.
    Any,
    /// JSON `null` only.
    Unit,
    /// A generated type.
    Named(TypeId),
    /// A list.
    Vec(Box<Self>),
    /// A list of exactly N items: `items` with `minItems == maxItems == N` (1 to 32).
    Array(Box<Self>, usize),
    /// A fixed sequence of types: closed `prefixItems` (`items: false` with every position required).
    Tuple(Vec<Self>),
    /// A map from string keys in key order.
    Map(Box<Self>),
    /// Nullable or optional.
    Option(Box<Self>),
    /// Held by reference where a type would contain itself by value (Rust `Box<T>` or Go `*T`).
    Indirect(Box<Self>),
}

impl Ty {
    /// Wraps in `Option` unless the type already accepts `null`.
    #[must_use]
    pub(crate) fn option(self) -> Self {
        if matches!(self, Self::Option(_) | Self::Any | Self::Unit) { self } else { Self::Option(Box::new(self)) }
    }

    /// Returns the types that this one holds directly. That is its item, value or positions or the type that it holds
    /// by reference.
    pub(crate) fn children(&self) -> &[Self] {
        match self {
            Self::Vec(t) | Self::Array(t, _) | Self::Map(t) | Self::Option(t) | Self::Indirect(t) => {
                std::slice::from_ref(t)
            }
            Self::Tuple(ts) => ts,
            Self::Bool
            | Self::I8
            | Self::I16
            | Self::I32
            | Self::I64
            | Self::U8
            | Self::U16
            | Self::U32
            | Self::U64
            | Self::F32
            | Self::F64
            | Self::Decimal
            | Self::String
            | Self::Bytes
            | Self::Binary
            | Self::Format(_)
            | Self::Any
            | Self::Unit
            | Self::Named(_) => &[],
        }
    }

    /// Returns [`Ty::children`] as mutable.
    pub(crate) fn children_mut(&mut self) -> &mut [Self] {
        match self {
            Self::Vec(t) | Self::Array(t, _) | Self::Map(t) | Self::Option(t) | Self::Indirect(t) => {
                std::slice::from_mut(t)
            }
            Self::Tuple(ts) => ts,
            Self::Bool
            | Self::I8
            | Self::I16
            | Self::I32
            | Self::I64
            | Self::U8
            | Self::U16
            | Self::U32
            | Self::U64
            | Self::F32
            | Self::F64
            | Self::Decimal
            | Self::String
            | Self::Bytes
            | Self::Binary
            | Self::Format(_)
            | Self::Any
            | Self::Unit
            | Self::Named(_) => &mut [],
        }
    }

    /// Does this hold its children on the heap? A `Vec`, a map and a type held by reference do. An `Option`, an
    /// array and a tuple hold theirs by value. So a type inside one of those that contains it again needs a box.
    pub(crate) const fn allocates(&self) -> bool {
        matches!(self, Self::Vec(_) | Self::Map(_) | Self::Indirect(_))
    }

    /// Is this an `Option`?
    pub(crate) fn is_option(&self) -> bool {
        matches!(self, Self::Option(_))
    }

    /// Returns the type with any outer `Option` removed.
    pub(crate) fn non_null(&self) -> &Self {
        if let Self::Option(t) = self { t } else { self }
    }
}

#[derive(Debug, Clone)]
/// A named type.
pub(crate) struct TypeDef {
    /// Type name that is unique in the API.
    pub name: Name,
    /// Title and description.
    pub doc: Option<String>,
    /// What kind of type it is.
    pub kind: Kind,
    /// The `xml` object of the schema: element name, namespace and prefix.
    pub xml: XmlHint,
    /// `x-rust-type`: your own type replaces the generated one (a Rust path).
    pub rust_type: Option<String>,
    /// The documented examples of the schema as (JSON pointer, value).
    pub examples: Vec<(String, Value)>,
    /// `deprecated: true` on the schema.
    pub deprecated: bool,
    /// A `Newtype` carried as JSON text in a string (`contentMediaType:
    /// application/json` with `contentSchema`).
    pub json_string: bool,
    /// A schema of `components/schemas`: one the description names. An identical one keeps its own type.
    pub component: bool,
}

/// Validation keywords of a schema (JSON Schema 2020-12 validation vocabulary).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Constraints {
    /// `minLength` (characters).
    pub min_length: Option<u64>,
    /// `maxLength` (characters).
    pub max_length: Option<u64>,
    /// `pattern` (ECMA-262).
    pub pattern: Option<String>,
    /// `minimum`.
    pub minimum: Option<serde_json::Number>,
    /// `maximum`.
    pub maximum: Option<serde_json::Number>,
    /// `exclusiveMinimum`.
    pub exclusive_minimum: Option<serde_json::Number>,
    /// `exclusiveMaximum`.
    pub exclusive_maximum: Option<serde_json::Number>,
    /// `multipleOf`.
    pub multiple_of: Option<serde_json::Number>,
    /// `minItems`.
    pub min_items: Option<u64>,
    /// `maxItems`.
    pub max_items: Option<u64>,
    /// `uniqueItems`.
    pub unique_items: bool,
    /// `minProperties`.
    pub min_properties: Option<u64>,
    /// `maxProperties`.
    pub max_properties: Option<u64>,
    /// `{type: number, format: decimal128}`: the number is one that IEEE 754 decimal128 holds exactly.
    pub decimal128: bool,
    /// Constraints of an array's items.
    pub items: Option<Box<Self>>,
    /// The schema with `$ref`s inlined when it uses keywords that no type expresses on its own
    /// (`contains`, `patternProperties`, `if`, `not`, ...). `runtime::check_schema`
    /// checks it at runtime.
    pub schema: Option<Value>,
}

/// An `xml` object (XML Object of the specification).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct XmlHint {
    /// Element (or attribute) name instead of the property or schema name.
    pub name: Option<String>,
    /// Namespace URI.
    pub namespace: Option<String>,
    /// Namespace prefix.
    pub prefix: Option<String>,
    /// An attribute and not an element (`attribute: true` or 3.2 `nodeType: attribute`).
    pub attribute: bool,
    /// The element's text content (3.2 `nodeType: text`).
    pub text: bool,
    /// Arrays: one wrapping element around the items.
    pub wrapped: bool,
    /// Arrays: element name of each item (`items.xml.name`).
    pub item: Option<String>,
    /// 3.2 `nodeType: cdata`: the text of the parent written as a CDATA section.
    pub cdata: bool,
    /// 3.2 `nodeType: none`: no element of its own; an object's fields sit in the parent.
    pub inline: bool,
}

impl XmlHint {
    /// Reads the `xml` object of a schema (and of its `items`).
    pub(crate) fn of(schema: &Value) -> Self {
        let x = schema.get("xml");
        let s = |k: &str| x.and_then(|x| x.get(k)).and_then(Value::as_str).map(String::from);
        let b = |k: &str| x.and_then(|x| x.get(k)).and_then(Value::as_bool).unwrap_or(false);
        let node = s("nodeType");
        Self {
            name: s("name"),
            namespace: s("namespace"),
            prefix: s("prefix"),
            attribute: b("attribute") || node.as_deref() == Some("attribute"),
            text: matches!(node.as_deref(), Some("text" | "cdata")),
            cdata: node.as_deref() == Some("cdata"),
            inline: node.as_deref() == Some("none"),
            // `nodeType: element` on an array is its wrapper (3.2). The array may be reached through `allOf` or
            // `$ref` (S3's `Buckets`) and have no `items` here: only lists read `wrapped`.
            wrapped: b("wrapped") || node.as_deref() == Some("element"),
            item: schema
                .get("items")
                .and_then(|i| i.get("xml"))
                .and_then(|x| x.get("name"))
                .and_then(Value::as_str)
                .map(String::from),
        }
    }
}

#[derive(Debug, Clone)]
/// The shape of a named type.
pub(crate) enum Kind {
    /// An object with known properties.
    Struct(Struct),
    /// Open string enum: known values plus an `Other(String)` fallback.
    StrEnum(Vec<EnumValue>),
    /// Open integer enum: (variant name, value) plus an `Other(i64)` fallback.
    IntEnum(Vec<(Name, i64)>),
    /// One of several types (`oneOf` / `anyOf`).
    Union(Union),
    /// `pub type Name = Ty;`
    Alias(Ty),
    /// `pub struct Name(pub Ty);` (used when an alias would be recursive).
    Newtype(Ty),
    /// A single allowed value (`const` with a string, number or bool). It is a unit
    /// struct. So no other value can be built.
    Const(Value),
    /// A string with a `pattern`. It is a newtype that only `TryFrom`
    /// (and decoding) build. They check the pattern and the length limits.
    Pattern(Box<Constraints>),
    /// Placeholder while a component is being converted.
    Pending,
}

#[derive(Debug, Clone)]
/// One value of a string enum.
pub(crate) struct EnumValue {
    /// Variant name.
    pub name: Name,
    /// Value on the wire.
    pub wire: String,
    /// Description when the spec has one.
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Default)]
/// An object type.
pub(crate) struct Struct {
    /// Declared properties in document order.
    pub fields: Vec<Field>,
    /// `additionalProperties` schema: flattened map of extra keys.
    pub additional: Option<Ty>,
    /// `additionalProperties: false`: decoding rejects undeclared properties.
    pub closed: bool,
}

#[derive(Debug, Clone)]
/// One property of a struct.
pub(crate) struct Field {
    /// Field name.
    pub name: Name,
    /// Property name on the wire.
    pub wire: String,
    /// Type that is `Option` when nullable.
    pub ty: Ty,
    /// In `required` and not read-only or write-only.
    pub required: bool,
    /// Title and description.
    pub doc: Option<String>,
    /// The property's `xml` object.
    pub xml: XmlHint,
    /// Validation keywords of the property's schema.
    pub constraints: Constraints,
    /// `deprecated: true` on the property.
    pub deprecated: bool,
    /// `default` of an optional property with a literal default: the field
    /// is marked required (always present after decoding) and filled with it
    /// when absent.
    pub default: Option<Value>,
    /// `readOnly` / `writeOnly`: the direction the property travels in.
    pub access: Access,
    /// In the schema's `required`. [`TypeSpace::split_directions`] uses it to
    /// make a one-direction property required in the type of its direction.
    pub spec_required: bool,
    /// The own `example` and `examples` of the property as (JSON pointer, value).
    pub examples: Vec<(String, Value)>,
    /// An optional property whose `null` reads as absent. Every optional property of a Swagger 2.0 description is one.
    /// 2.0 has no `nullable` to say that a property may be `null`. Still many 2.0 services send `null` for a property
    /// that they leave unset (every gRPC-gateway service). Not tri-state, because 2.0 cannot tell `null` from absent.
    pub null_as_absent: bool,
}

/// The direction a property travels in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Both directions.
    ReadWrite,
    /// `readOnly`: responses only.
    ReadOnly,
    /// `writeOnly`: requests only.
    WriteOnly,
}

#[derive(Debug, Clone)]
/// A `oneOf` / `anyOf` type.
pub(crate) struct Union {
    /// Variants in document order.
    pub variants: Vec<Variant>,
    /// How a value picks its variant.
    pub mode: UnionMode,
    /// A branch takes any value (`{}` or 3.0 `{nullable: true}`). `Other` holds such a value, so it is no violation.
    pub open: bool,
    /// A value of a kind that no variant covers is valid and `Other` holds it (a schema of `properties` without
    /// `type` takes any non-object). A value of a covered kind must fit its variant.
    pub rest: bool,
}

#[derive(Debug, Clone)]
/// One variant of a union.
pub(crate) struct Variant {
    /// Variant name.
    pub name: Name,
    /// The variant's payload type.
    pub ty: Ty,
    /// Discriminator values selecting this variant (Tagged mode).
    pub tags: Vec<String>,
    /// The validation keywords of an inline branch of a type without a name of its own (`{type: integer,
    /// maximum: 10}`). A named type checks its own.
    pub constraints: Constraints,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// How a union picks its variant.
pub(crate) enum UnionMode {
    /// Select by the string value of a property.
    Tagged {
        /// The discriminating property.
        property: String,
        /// Variant for absent or unknown values (3.2 `defaultMapping`).
        default: Option<usize>,
    },
    /// Variants have distinct JSON kinds (string, number, object, ...).
    ByKind,
    /// Try each variant in order.
    Untagged,
}

/// Coarse JSON kind of a type. It tells unions and parameters apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum JsonKind {
    /// `null`.
    Null,
    /// A boolean.
    Bool,
    /// An integer.
    Integer,
    /// A number with a fraction.
    Number,
    /// A string.
    String,
    /// An array.
    Array,
    /// An object.
    Object,
    /// Unknown: any kind.
    Any,
}

/// Every named type of an API.
#[derive(Debug, Clone)]
pub(crate) struct TypeSpace {
    /// Indexed by `TypeId`.
    pub defs: Vec<TypeDef>,
}

/// A string `format` with a well-known type of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Format {
    /// `format: uuid`.
    Uuid,
    /// `format: date-time` (RFC 3339).
    DateTime,
    /// `format: date` (`YYYY-MM-DD`).
    Date,
    /// `format: ipv4`: `std::net::Ipv4Addr`.
    Ipv4,
    /// `format: ipv6`: `std::net::Ipv6Addr`.
    Ipv6,
    /// `format: time` (RFC 3339 `full-time`).
    Time,
    /// `contentEncoding: base64url`: bytes sent in the URL-safe alphabet.
    Base64Url,
    /// `format: char`: one character.
    Char,
    /// `format: date-time-local` (RFC 3339 without an offset).
    DateTimeLocal,
    /// `format: time-local` (RFC 3339 time without an offset).
    TimeLocal,
    /// `format: uri` (absolute).
    Uri,
    /// `format: ipv4-cidr`.
    Ipv4Cidr,
    /// `format: ipv6-cidr`.
    Ipv6Cidr,
    /// `{type: string, format: decimal}`: a `String` that keeps every digit and is checked as a decimal.
    /// `rust_decimal` holds 28 significant digits and `decimal` has no limit. So no library type takes it.
    Decimal,
    /// `{type: string, format: decimal128}`: text checked as a decimal that IEEE 754 decimal128 holds exactly. That is
    /// at most 34 significant digits within its exponent range.
    Decimal128,
    /// `format: email` (RFC 5321 Mailbox): `runtime::Email` (the runtime's `idn` feature).
    Email,
    /// `format: idn-email` (RFC 6531 with UTF-8 allowed): `runtime::IdnEmail`.
    IdnEmail,
    /// `format: hostname` (RFC 1123; punycode labels must decode): `runtime::Hostname`.
    Hostname,
    /// `format: idn-hostname` (IDNA2008): `runtime::IdnHostname`.
    IdnHostname,
    /// `format: http-date` (RFC 9110 section 5.6.7): a `String` checked as one of the three forms.
    HttpDate,
    /// `format: duration` (RFC 3339 Appendix A): a `String` checked as an ISO 8601 duration.
    Duration,
    /// `format: uri-reference` (RFC 3986): a URI or a relative reference.
    UriReference,
    /// `format: iri` (RFC 3987): a URI that may hold non-ASCII characters.
    Iri,
    /// `format: iri-reference` (RFC 3987).
    IriReference,
    /// `format: json-pointer` (RFC 6901).
    JsonPointer,
    /// `format: relative-json-pointer`.
    RelativeJsonPointer,
    /// `format: regex`: an ECMA-262 regular expression (the runtime's `regress` feature).
    Regex,
    /// `format: uri-template` (RFC 6570).
    UriTemplate,
}

impl Format {
    /// Every format to look one up by its name.
    pub(crate) const ALL: [Self; 28] = [
        Self::Uuid,
        Self::DateTime,
        Self::Date,
        Self::Ipv4,
        Self::Ipv6,
        Self::Time,
        Self::Base64Url,
        Self::Decimal,
        Self::Char,
        Self::DateTimeLocal,
        Self::TimeLocal,
        Self::Uri,
        Self::Ipv4Cidr,
        Self::Ipv6Cidr,
        Self::Email,
        Self::IdnEmail,
        Self::Hostname,
        Self::IdnHostname,
        Self::HttpDate,
        Self::Duration,
        Self::UriReference,
        Self::Iri,
        Self::IriReference,
        Self::JsonPointer,
        Self::RelativeJsonPointer,
        Self::Regex,
        Self::UriTemplate,
        Self::Decimal128,
    ];

    /// Returns the `format` (or `contentEncoding`) a description writes for it.
    #[must_use]
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Uuid => "uuid",
            Self::DateTime => "date-time",
            Self::Date => "date",
            Self::Ipv4 => "ipv4",
            Self::Ipv6 => "ipv6",
            Self::Time => "time",
            Self::Base64Url => "base64url",
            Self::Decimal => "decimal",
            Self::Decimal128 => "decimal128",
            Self::Char => "char",
            Self::DateTimeLocal => "date-time-local",
            Self::TimeLocal => "time-local",
            Self::Uri => "uri",
            Self::Ipv4Cidr => "ipv4-cidr",
            Self::Ipv6Cidr => "ipv6-cidr",
            Self::Email => "email",
            Self::IdnEmail => "idn-email",
            Self::Hostname => "hostname",
            Self::IdnHostname => "idn-hostname",
            Self::HttpDate => "http-date",
            Self::Duration => "duration",
            Self::UriReference => "uri-reference",
            Self::Iri => "iri",
            Self::IriReference => "iri-reference",
            Self::JsonPointer => "json-pointer",
            Self::RelativeJsonPointer => "relative-json-pointer",
            Self::Regex => "regex",
            Self::UriTemplate => "uri-template",
        }
    }
}

impl TypeSpace {
    /// Splits request types for `readOnly` / `writeOnly` properties.
    ///
    /// Every struct with such a property and every type that reaches one
    /// gets a `<Name>Request` copy. The original keeps what responses carry.
    /// `writeOnly` properties are dropped and required `readOnly` ones stay
    /// required. The copy is the reverse. Returns the request type of each type.
    pub(crate) fn split_directions(&mut self) -> Vec<Option<TypeId>> {
        let n = self.defs.len();
        let mut split = vec![false; n];
        for (i, d) in self.defs.iter().enumerate() {
            split[i] = d.rust_type.is_none()
                && matches!(&d.kind, Kind::Struct(s) if s.fields.iter().any(|f| f.access != Access::ReadWrite));
        }
        // Types reaching a split type split too (your own `x-rust-type`s stay as they are).
        loop {
            let mut changed = false;
            for i in 0..n {
                if split[i] || self.defs[i].rust_type.is_some() {
                    continue;
                }
                let mut refs = vec![];
                for t in def_types(&self.defs[i].kind) {
                    all_refs(t, &mut refs);
                }
                if refs.iter().any(|r| split[r.0]) {
                    split[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut names: BTreeSet<String> = self.defs.iter().map(|d| d.name.key()).collect();
        let mut request_of: Vec<Option<TypeId>> = vec![None; n];
        for i in (0..n).filter(|i| split[*i]) {
            let name = unique(self.defs[i].name.and(&Name::new("Request")), &mut names);
            request_of[i] = Some(TypeId(self.defs.len()));
            let base = &self.defs[i];
            self.defs.push(TypeDef { name, examples: vec![], ..base.clone() });
        }
        for i in 0..n {
            let Some(req) = request_of[i] else { continue };
            let mut kind = self.defs[req.0].kind.clone();
            map_kind(&mut kind, &|t| map_named(t, &request_of));
            self.defs[req.0].kind = direction(kind, Access::WriteOnly);
            let base = std::mem::replace(&mut self.defs[i].kind, Kind::Pending);
            self.defs[i].kind = direction(base, Access::ReadOnly);
        }
        request_of
    }

    /// Returns the type a request carries for `t`.
    #[must_use]
    pub(crate) fn request_ty(t: &Ty, request_of: &[Option<TypeId>]) -> Ty {
        let mut t = t.clone();
        map_named(&mut t, request_of);
        t
    }

    /// Returns the definition of a named type.
    pub(crate) fn get(&self, id: TypeId) -> &TypeDef {
        &self.defs[id.0]
    }

    /// Creates a space of these definitions.
    pub(crate) const fn new(defs: Vec<TypeDef>) -> Self {
        Self { defs }
    }

    /// Returns `ty` with its aliases followed.
    pub(crate) fn unalias<'t>(&'t self, ty: &'t Ty) -> &'t Ty {
        build::unalias(&self.defs, ty)
    }

    /// Aliases followed to find whether a type accepts `null`: a chain may loop.
    const MAX_NULLABLE_HOPS: usize = 32;

    /// Returns the type without its `null` when it accepts one. Looks through aliases.
    pub(crate) fn nullable<'a>(&'a self, ty: &'a Ty) -> Option<&'a Ty> {
        let mut cur = ty;
        for _ in 0..Self::MAX_NULLABLE_HOPS {
            if let Ty::Option(t) = cur {
                return Some(t);
            }
            let Ty::Named(id) = cur else { return None };
            let Kind::Alias(t) = &self.get(*id).kind else { return None };
            cur = t;
        }
        None
    }

    /// Resolves aliases and wrappers down to the JSON kind on the wire.
    pub(crate) fn kind_of(&self, ty: &Ty) -> JsonKind {
        kind_in(&self.defs, ty)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_element_node_type_wraps_an_array_reached_through_a_reference() {
        // An upgraded `wrapped: true` beside `allOf: [{$ref: Buckets}]` (S3): no `items` of its own.
        let hint = super::XmlHint::of(&serde_json::json!({"allOf": [{"$ref": "#/x"}], "xml": {"nodeType": "element"}}));
        assert!(hint.wrapped);
    }
}
