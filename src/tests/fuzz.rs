//! Version-aware property tests. proptest builds a random API and writes it out as Swagger 2.0, `OpenAPI` 3.0, 3.1
//! and 3.2 by the rules of each version. For every API:
//!
//! 1. each written document is valid and loads with no error diagnostic
//! 2. the typed model roundtrips losslessly
//! 3. every version yields the same IR (operations, parameters, bodies, responses, security), so the upgraders
//!    preserve meaning
//! 4. the generator does not panic
//!
//! Profiles:
//!
//! - `common`: what 2.0 can express, compared across all four versions
//! - `rich`: 3.x only with oneOf and discriminator, nullable, cookies, deepObject and status ranges
//! - `v32`: query, additionalOperations, querystring, itemSchema streams and defaultMapping
//! - `composition`: 3.1 and 3.2 with anyOf, undiscriminated and nested oneOf, allOf members that declare the same
//!   property, `unevaluatedProperties`, `const`, bounded and unique arrays, recursive unions, every format, and random
//!   generator options
//!
//! The generator draws its choices from a list of numbers that proptest draws. A failing API shrinks with that list.
//! Every choice moves toward zero, the smallest one (one schema, one operation, no option). `PROPTEST_CASES` runs more.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{RngSeed, TestCaseError, TestRunner};

use crate::diag::Level;
use serde_json::{Map, Value, json};

/// The choices of the generator as proptest drew them. Past the end every choice is zero.
#[derive(Debug, Clone)]
struct Rng<'c> {
    choices: &'c [u64],
    at: usize,
}

impl Rng<'_> {
    fn next(&mut self) -> u64 {
        let choice = self.choices.get(self.at).copied().unwrap_or(0);
        self.at += 1;
        choice
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n.max(1)).unwrap_or(1)).unwrap_or(0)
    }

    /// True `percent` times in 100 and never for a zero choice. So shrinking turns options off.
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 >= 100 - percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Profile {
    Common,
    Rich,
    V32,
    Composition,
}

#[derive(Debug, Clone)]
enum S {
    Str {
        values: Option<Vec<String>>,
        format: Option<&'static str>,
    },
    Int {
        format: Option<&'static str>,
        min: Option<i64>,
        exclusive: bool,
    },
    Num,
    Bool,
    Arr(Box<Self>),
    Map(Box<Self>),
    Obj(Vec<Prop>),
    /// Reference to a named schema.
    Ref(usize),
    /// `allOf: [Ref, Obj]`.
    Extends(usize, Vec<Prop>),
    /// Discriminated `oneOf` of named object schemas; with `default` (3.2).
    OneOf {
        variants: Vec<usize>,
        default: bool,
    },
    /// `anyOf` of its branches.
    AnyOf(Vec<Self>),
    /// `oneOf` with no discriminator.
    Union(Vec<Self>),
    /// `allOf` of two objects. The second declares at least one property of the first.
    Shared(Vec<Prop>, Vec<Prop>),
    /// `anyOf` of its branches with `unevaluatedProperties: false`.
    Unevaluated(Vec<Self>),
    Const(String),
    /// An array with `minItems`, `maxItems` and `uniqueItems`.
    Bounded {
        items: Box<Self>,
        min: Option<u64>,
        max: Option<u64>,
        unique: bool,
    },
    /// A number with `format: decimal` or `decimal128`.
    Decimal(&'static str),
}

#[derive(Debug, Clone)]
struct Prop {
    name: String,
    s: S,
    required: bool,
    nullable: bool,
}

#[derive(Debug, Clone)]
struct Named {
    name: String,
    s: S,
    /// Variant of a discriminated union: carries `kind` with this tag.
    tag: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Loc {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone)]
struct Param {
    name: String,
    loc: Loc,
    s: S,
    required: bool,
    /// Query style for arrays / objects as (style, explode).
    style: Option<(&'static str, bool)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Media {
    Json,
    Form,
    Multipart,
    Text,
    Sse,
    JsonLines,
}

#[derive(Debug, Clone)]
struct Body {
    media: Media,
    s: S,
    required: bool,
}

#[derive(Debug, Clone)]
struct Resp {
    code: String,
    body: Option<(Media, S)>,
    headers: Vec<(String, S)>,
}

#[derive(Debug, Clone)]
struct Op {
    id: String,
    method: String,
    path: String,
    params: Vec<Param>,
    body: Option<Body>,
    responses: Vec<Resp>,
    security: Vec<Vec<usize>>,
    querystring: Option<S>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    ApiKeyHeader,
    ApiKeyQuery,
    Basic,
    OAuth,
    Bearer,
}

#[derive(Debug, Clone)]
struct Api {
    schemas: Vec<Named>,
    ops: Vec<Op>,
    schemes: Vec<Scheme>,
    opts: crate::emit::Options,
}

const WORDS: &[&str] =
    &["alpha", "beta", "gamma", "delta", "kind_x", "value", "item", "owner", "label", "count", "next"];

fn word(r: &mut Rng<'_>, i: usize) -> String {
    format!("{}{i}", r.pick(WORDS))
}

fn gen_prim(r: &mut Rng<'_>) -> S {
    match r.below(5) {
        0 => S::Str { values: None, format: r.chance(30).then(|| *r.pick(&["date-time", "uuid", "email"])) },
        1 => {
            let values = (0..=r.below(3)).map(|i| format!("v{i}")).collect();
            S::Str { values: Some(values), format: None }
        }
        2 => S::Int {
            format: r.chance(50).then(|| *r.pick(&["int32", "int64"])),
            min: r.chance(30).then_some(0),
            exclusive: r.chance(50),
        },
        3 => S::Num,
        _ => S::Bool,
    }
}

fn gen_schema(r: &mut Rng<'_>, depth: usize, named: usize, profile: Profile) -> S {
    if profile == Profile::Composition && depth <= 2 && r.chance(40) {
        return gen_composition(r, depth, named);
    }
    let roll = if depth > 2 { 0 } else { r.below(10) };
    match roll {
        0..=3 => gen_prim(r),
        4 => S::Arr(Box::new(gen_schema(r, depth + 1, named, profile))),
        5 => S::Map(Box::new(gen_prim(r))),
        6 | 7 => S::Obj(gen_props(r, depth + 1, named, profile)),
        _ if named > 0 => S::Ref(r.below(named)),
        _ => gen_prim(r),
    }
}

fn gen_composition(r: &mut Rng<'_>, depth: usize, named: usize) -> S {
    let profile = Profile::Composition;
    let n = 2 + r.below(2);
    match r.below(8) {
        0 => S::AnyOf((0..n).map(|_| gen_schema(r, depth + 1, named, profile)).collect()),
        1 => S::Union((0..n).map(|_| gen_schema(r, depth + 1, named, profile)).collect()),
        2 => {
            let first = gen_props(r, depth + 1, named, profile);
            let mut second = gen_props(r, depth + 1, named, profile);
            let mut shared = r.pick(&first).clone();
            if r.chance(30) {
                shared.s = gen_schema(r, depth + 1, named, profile);
            }
            shared.required = r.chance(50);
            second.retain(|p| p.name != shared.name);
            second.push(shared);
            S::Shared(first, second)
        }
        3 => S::Unevaluated(
            (0..n)
                .map(|_| {
                    if named > 0 && r.chance(50) {
                        S::Ref(r.below(named))
                    } else {
                        S::Obj(gen_props(r, depth + 1, named, profile))
                    }
                })
                .collect(),
        ),
        4 => S::Const(word(r, 0)),
        5 => {
            let min = r.chance(50).then(|| r.below(3) as u64);
            let max = r.chance(50).then(|| min.unwrap_or(0) + 1 + r.below(4) as u64);
            S::Bounded { items: Box::new(gen_schema(r, depth + 1, named, profile)), min, max, unique: r.chance(50) }
        }
        6 => S::Decimal(r.pick(&["decimal", "decimal128"])),
        _ => S::Str { values: None, format: Some(r.pick(&crate::ir::types::Format::ALL).name()) },
    }
}

fn gen_props(r: &mut Rng<'_>, depth: usize, named: usize, profile: Profile) -> Vec<Prop> {
    (0..=r.below(4))
        .map(|i| Prop {
            name: word(r, i),
            s: gen_schema(r, depth, named, profile),
            required: r.chance(50),
            nullable: profile != Profile::Common && r.chance(25),
        })
        .collect()
}

fn gen_api(choices: &[u64], profile: Profile) -> Api {
    let mut r = Rng { choices, at: 0 };
    let mut schemas: Vec<Named> = vec![];
    for i in 0..=r.below(5) {
        let s = match r.below(6) {
            0 if i > 0 => S::Extends(r.below(i), gen_props(&mut r, 1, i, profile)),
            _ => S::Obj(gen_props(&mut r, 1, i, profile)),
        };
        schemas.push(Named { name: format!("Model{i}"), s, tag: None });
    }
    // Self-recursion through an optional array: `children: [Self]`.
    let me = schemas.len() - 1;
    if r.chance(40)
        && let Some(S::Obj(props)) = schemas.last_mut().map(|n| &mut n.s)
    {
        props.push(Prop { name: "children".into(), s: S::Arr(Box::new(S::Ref(me))), required: false, nullable: false });
    }
    if profile != Profile::Common && r.chance(60) {
        let base = schemas.len();
        let n = 2 + r.below(2);
        for v in 0..n {
            let mut props = gen_props(&mut r, 2, base, profile);
            props.retain(|p| p.name != "kind");
            props.push(Prop {
                name: "kind".into(),
                s: S::Str { values: None, format: None },
                required: true,
                nullable: false,
            });
            schemas.push(Named { name: format!("Variant{v}"), s: S::Obj(props), tag: Some(format!("t{v}")) });
        }
        let variants = (base..base + n).collect();
        schemas.push(Named {
            name: "Union".into(),
            s: S::OneOf { variants, default: profile == Profile::V32 && r.chance(50) },
            tag: None,
        });
    }
    let schemes = {
        let mut all = vec![Scheme::ApiKeyHeader, Scheme::ApiKeyQuery, Scheme::Basic, Scheme::OAuth];
        if profile != Profile::Common {
            all.push(Scheme::Bearer);
        }
        all.retain(|_| r.chance(50));
        all
    };
    let mut ops = vec![];
    let schemas = if profile == Profile::Composition { recursive_unions(&mut r, schemas) } else { schemas };
    let named = schemas.len();
    for i in 0..=r.below(5) {
        ops.push(gen_op(&mut r, i, named, &schemes, profile));
    }
    if profile == Profile::V32 {
        let mut q = gen_op(&mut r, ops.len(), named, &schemes, profile);
        q.method = "query".into();
        q.params.retain(|p| p.loc != Loc::Query);
        q.querystring = Some(S::Obj(
            (0..=r.below(3))
                .map(|i| Prop { name: word(&mut r, i), s: gen_prim(&mut r), required: false, nullable: false })
                .collect(),
        ));
        q.body = None;
        ops.push(q);
        let mut extra = gen_op(&mut r, ops.len(), named, &schemes, profile);
        extra.method = "PURGE".into();
        ops.push(extra);
    }
    let opts = if profile == Profile::Composition { gen_options(&mut r) } else { crate::emit::Options::default() };
    Api { schemas, ops, schemes, opts }
}

/// A JSON-like `anyOf` of text or an array of itself, and a `oneOf` tree whose branch contains its own children.
fn recursive_unions(r: &mut Rng<'_>, mut schemas: Vec<Named>) -> Vec<Named> {
    if r.chance(50) {
        let me = schemas.len();
        let text = S::Str { values: None, format: None };
        schemas.push(Named { name: "Node".into(), s: S::AnyOf(vec![text, S::Arr(Box::new(S::Ref(me)))]), tag: None });
    }
    if r.chance(50) {
        let me = schemas.len();
        let leaf = Prop { name: "value".into(), s: S::Num, required: true, nullable: false };
        let kids = Prop { name: "kids".into(), s: S::Arr(Box::new(S::Ref(me))), required: true, nullable: false };
        schemas.push(Named {
            name: "Tree".into(),
            s: S::Union(vec![S::Obj(vec![leaf]), S::Obj(vec![kids])]),
            tag: None,
        });
    }
    schemas
}

/// Every choice at zero gives the default options.
fn gen_options(r: &mut Rng<'_>) -> crate::emit::Options {
    use crate::settings::DateLib;
    let mut o = crate::emit::Options {
        client: !r.chance(20),
        server: !r.chance(30),
        keep_unknown: r.chance(50),
        strict_client: !r.chance(50),
        allow_unimplemented: r.chance(30),
        ..crate::emit::Options::default()
    };
    let libs = &mut o.rust.libs;
    libs.uuid = r.chance(50);
    libs.url = r.chance(50);
    libs.ipnet = r.chance(50);
    libs.dates = [None, Some(DateLib::Chrono), Some(DateLib::Jiff)][r.below(3)];
    o
}

fn gen_op(r: &mut Rng<'_>, i: usize, named: usize, schemes: &[Scheme], profile: Profile) -> Op {
    let methods: &[&str] = &["get", "post", "put", "delete", "patch"];
    let method = (*r.pick(methods)).to_owned();
    let mut params = vec![];
    let mut path = format!("/r{i}");
    for p in 0..r.below(3) {
        let name = format!("p{p}");
        path.push_str(&format!("/{{{name}}}"));
        params.push(Param {
            name,
            loc: Loc::Path,
            s: S::Str { values: None, format: None },
            required: true,
            style: None,
        });
    }
    path.push_str(&format!("/leaf{i}"));
    for q in 0..r.below(4) {
        let mut locs = vec![Loc::Query, Loc::Header];
        if profile != Profile::Common {
            locs.push(Loc::Cookie);
        }
        let loc = *r.pick(&locs);
        let (s, style) = if loc == Loc::Query && r.chance(40) {
            let styles: &[(&str, bool)] =
                &[("form", true), ("form", false), ("pipeDelimited", false), ("spaceDelimited", false)];
            (S::Arr(Box::new(prim_no_enum(r))), Some(*r.pick(styles)))
        } else if loc == Loc::Query && profile != Profile::Common && r.chance(20) {
            let props = (0..=r.below(3))
                .map(|j| Prop { name: format!("f{j}"), s: prim_no_enum(r), required: false, nullable: false })
                .collect();
            (S::Obj(props), Some(("deepObject", true)))
        } else {
            (prim_no_enum(r), None)
        };
        let name = match loc {
            Loc::Header => format!("X-Q{q}"),
            Loc::Path | Loc::Query | Loc::Cookie => format!("q{q}"),
        };
        params.push(Param { name, loc, s, required: r.chance(40), style });
    }
    let body = (method != "get" && method != "delete" && r.chance(70)).then(|| {
        let media = *r.pick(&[Media::Json, Media::Json, Media::Form, Media::Multipart]);
        let s = match media {
            Media::Json if named > 0 && r.chance(60) => S::Ref(r.below(named)),
            Media::Json => gen_schema(r, 1, named, profile),
            Media::Form | Media::Multipart | Media::Text | Media::Sse | Media::JsonLines => S::Obj(
                (0..=r.below(3))
                    .map(|j| Prop {
                        name: format!("f{j}"),
                        s: prim_no_enum(r),
                        required: r.chance(50),
                        nullable: false,
                    })
                    .collect(),
            ),
        };
        Body { media, s, required: r.chance(60) }
    });
    let responses = gen_responses(r, named, profile);
    let security = if schemes.is_empty() || r.chance(40) {
        vec![]
    } else {
        (0..=r.below(2)).map(|_| vec![r.below(schemes.len())]).collect()
    };
    Op { id: format!("op{i}"), method, path, params, body, responses, security, querystring: None }
}

fn gen_responses(r: &mut Rng<'_>, named: usize, profile: Profile) -> Vec<Resp> {
    let mut codes: Vec<String> = vec!["200".into()];
    for c in ["201", "204", "400", "404", "default"] {
        if r.chance(30) {
            codes.push(c.into());
        }
    }
    if profile != Profile::Common && r.chance(30) {
        codes.push("5XX".into());
    }
    codes
        .into_iter()
        .map(|code| {
            let body = (code != "204").then_some(()).and_then(|()| {
                if !r.chance(70) {
                    return None;
                }
                let media = match profile {
                    Profile::V32 if r.chance(30) => *r.pick(&[Media::Sse, Media::JsonLines]),
                    Profile::Rich | Profile::V32 | Profile::Composition if r.chance(15) => Media::Text,
                    Profile::Common | Profile::Rich | Profile::V32 | Profile::Composition => Media::Json,
                };
                let s = match media {
                    Media::Sse => S::Obj(vec![
                        Prop {
                            name: "event".into(),
                            s: S::Str { values: None, format: None },
                            required: false,
                            nullable: false,
                        },
                        Prop {
                            name: "data".into(),
                            s: S::Str { values: None, format: None },
                            required: true,
                            nullable: false,
                        },
                    ]),
                    Media::Text => S::Str { values: None, format: None },
                    Media::Json | Media::JsonLines | Media::Form | Media::Multipart if named > 0 && r.chance(60) => {
                        S::Ref(r.below(named))
                    }
                    Media::Json | Media::JsonLines | Media::Form | Media::Multipart => gen_schema(r, 1, named, profile),
                };
                Some((media, s))
            });
            let headers = (0..r.below(2)).map(|h| (format!("X-R{h}"), prim_no_enum(r))).collect();
            Resp { code, body, headers }
        })
        .collect()
}

fn prim_no_enum(r: &mut Rng<'_>) -> S {
    match r.below(4) {
        0 => S::Str { values: None, format: None },
        1 => S::Int { format: Some("int64"), min: None, exclusive: false },
        2 => S::Num,
        _ => S::Bool,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum V {
    V20,
    V30,
    V31,
    V32,
}

impl V {
    fn schemas_ptr(self) -> &'static str {
        if self == Self::V20 { "#/definitions/" } else { "#/components/schemas/" }
    }
}

fn schema(api: &Api, s: &S, v: V) -> Value {
    match s {
        S::Str { values, format } => {
            let mut o = json!({"type": "string"});
            if let Some(vs) = values {
                o["enum"] = json!(vs);
            }
            if let Some(f) = format {
                o["format"] = json!(f);
            }
            o
        }
        S::Int { format, min, exclusive } => {
            let mut o = json!({"type": "integer"});
            if let Some(f) = format {
                o["format"] = json!(f);
            }
            if let Some(m) = min {
                // 2.0 / 3.0: boolean exclusiveMinimum; 3.1+: numeric.
                match (exclusive, v) {
                    (true, V::V20 | V::V30) => {
                        o["minimum"] = json!(m);
                        o["exclusiveMinimum"] = json!(true);
                    }
                    (true, V::V31 | V::V32) => o["exclusiveMinimum"] = json!(m),
                    (false, _) => o["minimum"] = json!(m),
                }
            }
            o
        }
        S::Num => json!({"type": "number"}),
        S::Bool => json!({"type": "boolean"}),
        S::Arr(i) => json!({"type": "array", "items": schema(api, i, v)}),
        S::Map(i) => json!({"type": "object", "additionalProperties": schema(api, i, v)}),
        S::Obj(props) => object(api, props, v),
        S::Ref(i) => json!({"$ref": format!("{}{}", v.schemas_ptr(), api.schemas[*i].name)}),
        S::Extends(base, props) => {
            json!({"allOf": [{"$ref": format!("{}{}", v.schemas_ptr(), api.schemas[*base].name)}, object(api, props, v)]})
        }
        S::AnyOf(branches) => json!({"anyOf": branches.iter().map(|b| schema(api, b, v)).collect::<Vec<_>>()}),
        S::Union(branches) => json!({"oneOf": branches.iter().map(|b| schema(api, b, v)).collect::<Vec<_>>()}),
        S::Shared(first, second) => json!({"allOf": [object(api, first, v), object(api, second, v)]}),
        S::Unevaluated(branches) => json!({
            "anyOf": branches.iter().map(|b| schema(api, b, v)).collect::<Vec<_>>(),
            "unevaluatedProperties": false,
        }),
        S::Const(c) => json!({"const": c}),
        S::Bounded { items, min, max, unique } => {
            let mut o = json!({"type": "array", "items": schema(api, items, v)});
            if let Some(m) = min {
                o["minItems"] = json!(m);
            }
            if let Some(m) = max {
                o["maxItems"] = json!(m);
            }
            if *unique {
                o["uniqueItems"] = json!(true);
            }
            o
        }
        S::Decimal(format) => json!({"type": "number", "format": format}),
        S::OneOf { variants, default } => {
            let refs: Vec<Value> = variants
                .iter()
                .map(|i| json!({"$ref": format!("{}{}", v.schemas_ptr(), api.schemas[*i].name)}))
                .collect();
            let mapping: Map<String, Value> = variants
                .iter()
                .map(|i| {
                    let n = &api.schemas[*i];
                    (n.tag.clone().unwrap_or_default(), json!(format!("{}{}", v.schemas_ptr(), n.name)))
                })
                .collect();
            let mut d = json!({"propertyName": "kind", "mapping": mapping});
            if *default {
                d["defaultMapping"] = json!(format!("{}{}", v.schemas_ptr(), api.schemas[variants[0]].name));
            }
            json!({"oneOf": refs, "discriminator": d})
        }
    }
}

fn object(api: &Api, props: &[Prop], v: V) -> Value {
    let mut properties = Map::new();
    let mut required = vec![];
    for p in props {
        let mut s = schema(api, &p.s, v);
        if p.nullable {
            s = nullable(s, v);
        }
        properties.insert(p.name.clone(), s);
        if p.required {
            required.push(json!(p.name));
        }
    }
    let mut o = json!({"type": "object", "properties": properties});
    if !required.is_empty() {
        o["required"] = Value::Array(required);
    }
    o
}

/// Makes a schema nullable in the version's own idiom.
fn nullable(mut s: Value, v: V) -> Value {
    let is_ref = s.get("$ref").is_some();
    match v {
        V::V20 => {
            s["x-nullable"] = json!(true);
            s
        }
        V::V30 if is_ref => json!({"allOf": [s], "nullable": true}),
        V::V30 => {
            s["nullable"] = json!(true);
            if let Some(Value::Array(e)) = s.get_mut("enum") {
                e.push(Value::Null);
            }
            s
        }
        V::V31 | V::V32 if is_ref || s.get("type").is_none() => json!({"anyOf": [s, {"type": "null"}]}),
        V::V31 | V::V32 => {
            let t = s["type"].clone();
            s["type"] = json!([t, "null"]);
            if let Some(Value::Array(e)) = s.get_mut("enum") {
                e.push(Value::Null);
            }
            s
        }
    }
}

fn media_type(m: Media) -> &'static str {
    match m {
        Media::Json => "application/json",
        Media::Form => "application/x-www-form-urlencoded",
        Media::Multipart => "multipart/form-data",
        Media::Text => "text/plain",
        Media::Sse => "text/event-stream",
        Media::JsonLines => "application/jsonl",
    }
}

fn scheme_name(s: Scheme) -> &'static str {
    match s {
        Scheme::ApiKeyHeader => "keyHeader",
        Scheme::ApiKeyQuery => "keyQuery",
        Scheme::Basic => "basic",
        Scheme::OAuth => "oauth",
        Scheme::Bearer => "bearer",
    }
}

fn write(api: &Api, v: V) -> Value {
    let mut doc = Map::new();
    match v {
        V::V20 => {
            doc.insert("swagger".into(), json!("2.0"));
            doc.insert("host".into(), json!("api.example.com"));
            doc.insert("basePath".into(), json!("/v1"));
            doc.insert("schemes".into(), json!(["https"]));
        }
        V::V30 | V::V31 | V::V32 => {
            let version = match v {
                V::V30 => "3.0.4",
                V::V31 => "3.1.2",
                V::V20 | V::V32 => "3.2.0",
            };
            doc.insert("openapi".into(), json!(version));
            doc.insert("servers".into(), json!([{"url": "https://api.example.com/v1"}]));
        }
    }
    doc.insert("info".into(), json!({"title": "Fuzz", "version": "1"}));
    let defs: Map<String, Value> = api
        .schemas
        .iter()
        .map(|n| {
            let mut s = schema(api, &n.s, v);
            if let Some(tag) = &n.tag {
                s["properties"]["kind"]["enum"] = json!([tag]);
            }
            (n.name.clone(), s)
        })
        .collect();
    let schemes: Map<String, Value> =
        api.schemes.iter().map(|s| (scheme_name(*s).to_owned(), security_scheme(*s, v))).collect();
    let mut paths = Map::new();
    for op in &api.ops {
        let item = paths.entry(op.path.clone()).or_insert_with(|| json!({}));
        let (key, value) = operation(api, op, v);
        if key == "additionalOperations" {
            item["additionalOperations"][op.method.as_str()] = value;
        } else {
            item[key.as_str()] = value;
        }
    }
    doc.insert("paths".into(), Value::Object(paths));
    if v == V::V20 {
        doc.insert("definitions".into(), Value::Object(defs));
        if !schemes.is_empty() {
            doc.insert("securityDefinitions".into(), Value::Object(schemes));
        }
    } else {
        let mut comps = json!({"schemas": defs});
        if !schemes.is_empty() {
            comps["securitySchemes"] = Value::Object(schemes);
        }
        doc.insert("components".into(), comps);
    }
    Value::Object(doc)
}

fn security_scheme(s: Scheme, v: V) -> Value {
    match (s, v) {
        (Scheme::ApiKeyHeader, _) => json!({"type": "apiKey", "in": "header", "name": "X-Key"}),
        (Scheme::ApiKeyQuery, _) => json!({"type": "apiKey", "in": "query", "name": "key"}),
        (Scheme::Basic, V::V20) => json!({"type": "basic"}),
        (Scheme::Basic, _) => json!({"type": "http", "scheme": "basic"}),
        (Scheme::Bearer, _) => json!({"type": "http", "scheme": "bearer"}),
        (Scheme::OAuth, V::V20) => {
            json!({"type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a.example.com", "tokenUrl": "https://t.example.com", "scopes": {"read": "r"}})
        }
        (Scheme::OAuth, _) => {
            json!({"type": "oauth2", "flows": {"authorizationCode": {"authorizationUrl": "https://a.example.com", "tokenUrl": "https://t.example.com", "scopes": {"read": "r"}}}})
        }
    }
}

fn operation(api: &Api, op: &Op, v: V) -> (String, Value) {
    let mut o = json!({"operationId": op.id});
    let mut params: Vec<Value> = op.params.iter().map(|p| param(api, p, v)).collect();
    if let Some(qs) = &op.querystring {
        params.push(json!({"name": "qs", "in": "querystring", "content": {"application/x-www-form-urlencoded": {"schema": schema(api, qs, v)}}}));
    }
    if let Some(b) = &op.body {
        write_body(api, b, v, &mut o, &mut params);
    }
    if !params.is_empty() {
        o["parameters"] = Value::Array(params);
    }
    o["responses"] = Value::Object(write_responses(api, &op.responses, v, &mut o));
    if !op.security.is_empty() {
        let reqs: Vec<Value> = op
            .security
            .iter()
            .map(|alt| {
                let m: Map<String, Value> = alt
                    .iter()
                    .map(|i| {
                        let s = api.schemes[*i];
                        let scopes = if s == Scheme::OAuth { json!(["read"]) } else { json!([]) };
                        (scheme_name(s).to_owned(), scopes)
                    })
                    .collect();
                Value::Object(m)
            })
            .collect();
        o["security"] = Value::Array(reqs);
    }
    let key = match op.method.as_str() {
        "PURGE" => "additionalOperations".to_owned(),
        m => m.to_owned(),
    };
    (key, o)
}

fn write_body(api: &Api, b: &Body, v: V, o: &mut Value, params: &mut Vec<Value>) {
    if v == V::V20 {
        match b.media {
            Media::Json => {
                params.push(
                    json!({"name": "body", "in": "body", "required": b.required, "schema": schema(api, &b.s, v)}),
                );
                o["consumes"] = json!(["application/json"]);
            }
            Media::Form | Media::Multipart | Media::Text | Media::Sse | Media::JsonLines => {
                if let S::Obj(props) = &b.s {
                    for p in props {
                        let mut fp = prim_param(api, &p.s, v);
                        fp["name"] = json!(p.name);
                        fp["in"] = json!("formData");
                        // 2.0 has no body-level `required`: a body is required when a field is.
                        fp["required"] = json!(p.required && b.required);
                        params.push(fp);
                    }
                }
                o["consumes"] = json!([media_type(b.media)]);
            }
        }
    } else if let (Media::Form | Media::Multipart, S::Obj(props)) = (b.media, &b.s) {
        // Mirror 2.0, where each form field says if it is required. A field
        // counts as required only when the body is. The body is
        // required when one of its fields is.
        let fields: Vec<Prop> =
            props.iter().map(|p| Prop { required: p.required && b.required, ..p.clone() }).collect();
        let any_required = fields.iter().any(|p| p.required);
        o["requestBody"] =
            json!({"required": any_required, "content": {media_type(b.media): {"schema": object(api, &fields, v)}}});
    } else {
        o["requestBody"] =
            json!({"required": b.required, "content": {media_type(b.media): {"schema": schema(api, &b.s, v)}}});
    }
}

fn write_responses(api: &Api, responses: &[Resp], v: V, o: &mut Value) -> Map<String, Value> {
    let mut out = Map::new();
    for r in responses {
        let mut resp = json!({"description": "d"});
        if let Some((m, s)) = &r.body {
            if v == V::V20 {
                resp["schema"] = schema(api, s, v);
                o["produces"] = json!(["application/json"]);
            } else if matches!(m, Media::Sse | Media::JsonLines) {
                resp["content"] = json!({media_type(*m): {"itemSchema": schema(api, s, v)}});
            } else {
                resp["content"] = json!({media_type(*m): {"schema": schema(api, s, v)}});
            }
        }
        if !r.headers.is_empty() {
            let hs: Map<String, Value> = r
                .headers
                .iter()
                .map(|(n, s)| {
                    let h = if v == V::V20 { prim_param(api, s, v) } else { json!({"schema": schema(api, s, v)}) };
                    (n.clone(), h)
                })
                .collect();
            resp["headers"] = Value::Object(hs);
        }
        out.insert(r.code.clone(), resp);
    }
    out
}

/// A 2.0 non-body parameter (or header) carries its type inline.
fn prim_param(api: &Api, s: &S, v: V) -> Value {
    let mut p = schema(api, s, v);
    if let S::Arr(_) = s {
        p["collectionFormat"] = json!("csv");
    }
    p
}

fn param(api: &Api, p: &Param, v: V) -> Value {
    let loc = match p.loc {
        Loc::Path => "path",
        Loc::Query => "query",
        Loc::Header => "header",
        Loc::Cookie => "cookie",
    };
    let mut o = json!({"name": p.name, "in": loc});
    if p.required || p.loc == Loc::Path {
        o["required"] = json!(true);
    }
    if v == V::V20 {
        let inline = prim_param(api, &p.s, v);
        for (k, val) in inline.as_object().into_iter().flatten() {
            o[k.as_str()] = val.clone();
        }
        if let Some((style, explode)) = p.style {
            o["collectionFormat"] = json!(match (style, explode) {
                ("form", true) => "multi",
                ("pipeDelimited", _) => "pipes",
                ("spaceDelimited", _) => "ssv",
                _ => "csv",
            });
        }
    } else {
        o["schema"] = schema(api, &p.s, v);
        if let Some((style, explode)) = p.style {
            o["style"] = json!(style);
            o["explode"] = json!(explode);
        } else if p.loc == Loc::Query && matches!(p.s, S::Arr(_)) {
            o["style"] = json!("form");
            o["explode"] = json!(false);
        }
    }
    o
}

/// Version-independent summary of what the generator will emit.
fn summary(api: &crate::ir::Api) -> String {
    let mut out = String::new();
    let mut ops: Vec<&crate::ir::http::Operation> = api.http.operations.iter().collect();
    ops.sort_by(|a, b| a.name.cmp(&b.name));
    for op in ops {
        out.push_str(&format!("{} {} {}", crate::emit::rust::names::snake(&op.name), op.method, op.path));
        out.push('\n');
        let mut ps: Vec<String> = op
            .params
            .iter()
            .map(|p| {
                format!(
                    "  param {} {:?} {:?}/{} kind={:?}",
                    p.wire,
                    p.location,
                    p.style,
                    p.explode,
                    api.types.kind_of(&p.ty)
                )
            })
            .collect();
        ps.sort();
        for p in ps {
            out.push_str(&p);
            out.push('\n');
        }
        if let Some(b) = &op.body {
            let media: Vec<String> = b.contents.iter().map(|c| format!("{}:{:?}", c.media, c.kind)).collect();
            out.push_str(&format!("  body req={} {}", b.required, media.join(",")));
            out.push('\n');
        }
        for r in &op.responses {
            let media: Vec<String> =
                r.contents.iter().map(|c| format!("{}:{:?}:{:?}", c.media, c.kind, api.types.kind_of(&c.ty))).collect();
            let hs: Vec<&str> = r.headers.iter().map(|h| h.wire.as_str()).collect();
            out.push_str(&format!("  resp {:?} [{}] headers={hs:?}", r.status, media.join(",")));
            out.push('\n');
        }
        out.push_str(&format!("  security {:?}", op.security));
        out.push('\n');
    }
    out
}

fn check(choices: &[u64], profile: Profile) -> Result<(), String> {
    let api = gen_api(choices, profile);
    let versions: &[V] = match profile {
        Profile::Common => &[V::V20, V::V30, V::V31, V::V32],
        Profile::Rich => &[V::V30, V::V31, V::V32],
        Profile::V32 => &[V::V32],
        Profile::Composition => &[V::V31, V::V32],
    };
    let mut summaries: Vec<(V, String)> = vec![];
    // The IR of each version that was emitted. Emitting is a function of the IR, so an IR seen before is not emitted
    // again. Versions before 3.1 always give the same IR, and later ones often do.
    let mut emitted: Vec<String> = vec![];
    for &v in versions {
        let doc = write(&api, v);
        let text = serde_json::to_string(&doc).map_err(|e| e.to_string())?;
        let loaded = crate::load::load_str(&text);
        let errors: Vec<String> =
            loaded.diagnostics.iter().filter(|d| d.level == Level::Error).map(ToString::to_string).collect();
        if !errors.is_empty() {
            let text = serde_json::to_string_pretty(&doc).unwrap_or(text);
            return Err(format!("{v:?}: valid document rejected: {errors:?}\n{text}"));
        }
        let Some(model) = loaded.model.clone() else { return Err(format!("{v:?}: no model")) };
        if Some(crate::spec::to_value(&model)) != loaded.value {
            return Err(format!("{v:?}: model roundtrip is lossy"));
        }
        let rdoc = loaded.doc().ok_or("no doc")?;
        let generated = std::panic::catch_unwind(|| {
            let mut d = crate::diag::Diagnostics::default();
            let ir = crate::ir::build(&rdoc, &mut d);
            let key = format!("{ir:?}");
            let files = if emitted.contains(&key) {
                Ok(0)
            } else {
                crate::emit::rust::print(crate::emit::rust::trees(&ir, &api.opts), false).map(|files| files.len())
            };
            (ir, key, files)
        });
        match generated {
            Err(_) => return Err(format!("{v:?}: the generator panicked")),
            Ok((_, _, Err(e))) => return Err(format!("{v:?}: {e}")),
            Ok((ir, key, Ok(_))) => {
                summaries.push((v, summary(&ir)));
                if !emitted.contains(&key) {
                    emitted.push(key);
                }
            }
        }
    }
    if let Some((first_v, first)) = summaries.first() {
        for (v, s) in &summaries[1..] {
            if s != first {
                let diff: Vec<String> = first
                    .lines()
                    .zip(s.lines())
                    .filter(|(a, b)| a != b)
                    .take(4)
                    .map(|(a, b)| format!("  {first_v:?}: {a}\n  {v:?}: {b}"))
                    .collect();
                return Err(format!("IR differs between {first_v:?} and {v:?}:\n{}", diff.join("\n")));
            }
        }
    }
    Ok(())
}

/// The most choices one API uses: more are never drawn.
const CHOICES: usize = 1024;

fn choices() -> impl Strategy<Value = Vec<u64>> {
    proptest::collection::vec(any::<u64>(), 0..CHOICES)
}

/// Cases of each profile, split into shards that nextest runs side by side. Each shard has a fixed seed of its own,
/// so CI fails only on a change. A failure is saved in `proptest-regressions` and replayed first.
const CASES: u32 = 256;
const SHARDS: u32 = 4;

fn fuzz(profile: Profile, shard: u32) {
    let config = ProptestConfig {
        cases: CASES / SHARDS,
        rng_seed: RngSeed::Fixed(u64::from(shard)),
        source_file: Some(file!()),
        ..ProptestConfig::default()
    };
    TestRunner::new(config).run(&choices(), |c| check(&c, profile).map_err(TestCaseError::fail)).unwrap();
}

/// One module per profile with a test per shard.
macro_rules! shards {
    ($($name:ident: $profile:expr;)*) => {$(
        mod $name {
            use super::{Profile, fuzz};

            #[test]
            fn shard_0() {
                fuzz($profile, 0);
            }

            #[test]
            fn shard_1() {
                fuzz($profile, 1);
            }

            #[test]
            fn shard_2() {
                fuzz($profile, 2);
            }

            #[test]
            fn shard_3() {
                fuzz($profile, 3);
            }
        }
    )*};
}

shards! {
    common_profile_across_v20_v30_v31_v32: Profile::Common;
    rich_profile_across_v30_v31_v32: Profile::Rich;
    v32_only_features: Profile::V32;
    composition_across_v31_v32: Profile::Composition;
}

/// Compiles the generated crates of a few APIs with their test of the documented examples and runs it.
#[test]
#[ignore = "compiles generated crates"]
fn generated_crates_compile_and_pass_their_tests() {
    let mut runner = TestRunner::deterministic();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("fuzz-crates");
    let mut members = vec![];
    for (profile, v) in [(Profile::Composition, V::V31), (Profile::Composition, V::V32)] {
        for index in 0..32 {
            let name = format!("fuzz-{profile:?}-{v:?}-{index}").to_lowercase();
            let dir = root.join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            let spec = dir.join("spec.json");
            let api = gen_api(&choices().new_tree(&mut runner).unwrap().current(), profile);
            std::fs::write(&spec, serde_json::to_string(&write(&api, v)).unwrap()).unwrap();
            let opts = crate::emit::Options { tests: true, ..api.opts };
            super::common::new_crate(&spec, &dir, &name, &opts);
            super::common::use_local_runtime(&dir.join("Cargo.toml"));
            add_libs(&dir.join("Cargo.toml"), &opts.rust.libs);
            members.push(name);
        }
    }
    let manifest: String = members.iter().map(|m| format!("  \"{m}\",\n")).collect();
    std::fs::write(root.join("Cargo.toml"), format!("[workspace]\nresolver = \"3\"\nmembers = [\n{manifest}]\n"))
        .unwrap();
    let status = std::process::Command::new(env!("CARGO"))
        .args(["test", "--workspace", "--quiet"])
        .current_dir(&root)
        .env("RUST_MIN_STACK", "67108864")
        .status()
        .unwrap();
    assert!(status.success(), "generated fuzz crates failed their tests");
}

/// Adds the libraries that the options give formats to.
fn add_libs(manifest: &std::path::Path, libs: &crate::settings::FormatLibs) {
    use crate::settings::DateLib;
    let mut doc: toml_edit::DocumentMut = std::fs::read_to_string(manifest).unwrap().parse().unwrap();
    let dates = match libs.dates {
        Some(DateLib::Jiff) => Some(("jiff", "1")),
        Some(DateLib::Chrono) => Some(("chrono", "0.4")),
        None => None,
    };
    let wanted = [(libs.uuid, "uuid", "1"), (libs.url, "url", "2"), (libs.ipnet, "ipnet", "2")]
        .into_iter()
        .filter_map(|(on, name, version)| on.then_some((name, version)))
        .chain(dates);
    for (name, version) in wanted {
        let mut dep = toml_edit::InlineTable::new();
        dep.insert("version", version.into());
        dep.insert("features", toml_edit::Array::from_iter(["serde"]).into());
        doc["dependencies"][name] = toml_edit::value(dep);
    }
    std::fs::write(manifest, doc.to_string()).unwrap();
}
