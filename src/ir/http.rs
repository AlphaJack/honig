//! HTTP operations. There is one per (path, method) with typed params, bodies and responses.

use std::collections::BTreeSet;

use serde_json::Value;

use super::media::MediaKind;
use super::names::{Name, reserved, unique, unique_sorted};
use super::refs::{MAX_REF_HOPS, resolve_value};
use super::security::{Requirement, SecurityScheme, requirements};
use super::types::{Builder, Ty, TypeId, TypeSpace, doc_of};
use crate::json;
use crate::json::pointer;
use crate::json::{escape_token, join};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
/// Parameter location.
pub(crate) enum Location {
    /// `in: path`.
    Path,
    /// `in: query`.
    Query,
    /// 3.2: the whole query string as one value.
    QueryString,
    /// `in: header`.
    Header,
    /// `in: cookie`.
    Cookie,
    /// A header of one `multipart/form-data` part (`encoding.<property>.headers`).
    Part,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Parameter serialization style.
pub(crate) enum Style {
    /// `simple`: `a,b`.
    Simple,
    /// `label`: `.a.b`.
    Label,
    /// `matrix`: `;id=a`.
    Matrix,
    /// `form`: `id=a,b` or `id=a&id=b`.
    Form,
    /// `spaceDelimited`: `id=a%20b`.
    SpaceDelimited,
    /// `pipeDelimited`: `id=a|b`.
    PipeDelimited,
    /// `deepObject`: `id[key]=value`.
    DeepObject,
    /// `cookie` (3.2): RFC 6265 with no percent-encoding.
    Cookie,
    /// Swagger 2.0 `collectionFormat: tsv` (kept as `x-collection-format`): `id=a%09b`.
    TabDelimited,
}

#[derive(Debug, Clone)]
/// One parameter of an operation.
pub(crate) struct Param {
    /// Field name.
    pub name: Name,
    /// Name on the wire.
    pub wire: String,
    /// Where it travels.
    pub location: Location,
    /// Type, already `Option` when not required.
    pub ty: Ty,
    /// Effective style (defaults applied).
    pub style: Style,
    /// Effective `explode`.
    pub explode: bool,
    /// `allowReserved`: reserved characters are sent unencoded.
    pub allow_reserved: bool,
    /// Parameter declared with `content`: the value is serialized with that media type.
    pub content: Option<MediaKind>,
    /// `description`.
    pub doc: Option<String>,
    /// Validation keywords of the parameter's schema.
    pub constraints: crate::ir::types::Constraints,
    /// Documented examples: (label, value).
    pub examples: Vec<(String, Value)>,
    /// `allowEmptyValue` (2.0/3.x query): `?flag` with no value is meaningful.
    pub allow_empty: bool,
    /// `deprecated: true`.
    pub deprecated: bool,
    /// `default` (literal) of an optional parameter: `ty` is a plain `T` and
    /// the server fills it in when the request leaves it out.
    pub default: Option<Value>,
    /// [`Location::Part`]: the multipart property whose part carries the header.
    pub part: Option<String>,
    /// Examples with both `dataValue` and `serializedValue` (3.2) as
    /// (JSON pointer, value, the value as sent on the wire).
    pub serialized: Vec<(String, Value, String)>,
}

/// The keywords of a sequential body as a whole. It may arrive an item at a time: they are checked as it does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ItemLimits {
    /// `minItems`.
    pub min: Option<u64>,
    /// `maxItems`.
    pub max: Option<u64>,
    /// `uniqueItems`.
    pub unique: bool,
}

impl ItemLimits {
    /// Is there anything to check?
    pub(crate) fn is_empty(self) -> bool {
        self == Self::default()
    }
}

#[derive(Debug, Clone)]
/// One media type of a body.
pub(crate) struct Content {
    /// The declared media type (may be a range like `image/*`).
    pub media: String,
    /// The Media Type Object's `description` (3.2).
    pub doc: Option<String>,
    /// How the body is serialized.
    pub kind: MediaKind,
    /// The type of the body (for sequences: a list of items).
    pub ty: Ty,
    /// Variant name when several media types are declared.
    pub variant: Name,
    /// Multipart / form: `encoding.<property>.contentType` per property.
    /// Positional multipart: `("0", ..)` per `prefixEncoding` entry and `("*", ..)`
    /// for `itemEncoding`. Also `("prefix", n)` when the body is `(P0, .., Vec<Item>)`.
    pub encoding: Vec<(String, String)>,
    /// A response body: sequential ones may be streamed by the server.
    pub response: bool,
    /// Positional multipart `(P0, .., Vec<Item>)`: the number of fixed parts.
    pub positional_prefix: Option<usize>,
    /// Documented examples of this media type as (label, value).
    pub examples: Vec<(String, Value)>,
    /// Form fields with an `encoding` `style`/`explode`/`allowReserved` as
    /// (property, style, explode, allowReserved, shape: `Array`/`Object`/`Prim`).
    pub form_styles: Vec<(String, Style, bool, bool, &'static str)>,
    /// The examples of an XML media type with both a `dataValue` and its serialized form (`serializedValue` or an
    /// `externalValue` file) as (JSON pointer, value, XML text). A generated test decodes one into the other.
    pub serialized: Vec<(String, Value, String)>,
    /// `multipart/form-data` properties whose schema says `contentEncoding: base64`: each part goes out base64
    /// with `Content-Transfer-Encoding: base64` (3.2: the two are equivalent).
    pub transfer: Vec<String>,
    /// A sequence (JSON Lines, JSON text sequences, SSE): `minItems`, `maxItems` and `uniqueItems` of its `schema`.
    pub item_limits: ItemLimits,
}

impl Content {
    /// Returns a sequential body typed `runtime::Items<T>`. Items are streamed out. For a request they are read as they
    /// arrive.
    pub(crate) fn streams(&self) -> bool {
        self.kind.is_sequence() && matches!(self.ty, Ty::Vec(_))
    }
}

#[derive(Debug, Clone)]
/// A request body.
pub(crate) struct Body {
    /// `requestBody.required`.
    pub required: bool,
    /// One entry per declared media type.
    pub contents: Vec<Content>,
    /// `requestBody.description`.
    pub doc: Option<String>,
    /// Constraints of the body schema (single media type only).
    pub constraints: crate::ir::types::Constraints,
    /// Untyped constraints of each entry of `contents` when there are several.
    /// They are checked on the value of the variant.
    pub variant_constraints: Vec<crate::ir::types::Constraints>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The status codes a response covers.
pub(crate) enum Status {
    /// One status code.
    Code(u16),
    /// `2XX` to `Range(2)`
    Range(u16),
    /// Any status not otherwise listed.
    Default,
}

#[derive(Debug, Clone)]
/// One documented response.
pub(crate) struct ResponseHeader {
    /// Field name.
    pub name: Name,
    /// Header name.
    pub wire: String,
    /// Type, already `Option` when not required.
    pub ty: Ty,
    /// Declared with `content` (a JSON media type): the value is JSON text.
    pub json: bool,
    /// Declared with `application/linkset` `content` (a `Link` header, RFC 9264). The value is its linkset model.
    pub linkset: bool,
    /// `description`.
    pub doc: Option<String>,
    /// `deprecated: true`.
    pub deprecated: bool,
    /// `explode` (style `simple`): objects as `k=v,k2=v2` instead of `k,v,k2,v2`.
    pub explode: bool,
    /// Documented examples: (JSON pointer, value).
    pub examples: Vec<(String, Value)>,
}

#[derive(Debug, Clone)]
/// One documented response.
pub(crate) struct Resp {
    /// Which status codes select it.
    pub status: Status,
    /// Variant name in the operation's `Response` enum.
    pub variant: Name,
    /// One entry per declared media type.
    pub contents: Vec<Content>,
    /// Declared headers, `Content-Type` excluded.
    pub headers: Vec<ResponseHeader>,
    /// `description`.
    pub doc: Option<String>,
    /// `links`: follow-up operations fed from this exchange.
    pub links: Vec<Link>,
    /// The `components/responses` entry that it is by name. Every operation that uses it shares its body type.
    pub shared: Option<Name>,
}

#[derive(Debug, Clone)]
/// A response link: values for another operation's parameters.
pub(crate) struct Link {
    /// Key in the response's `links`.
    pub name: String,
    /// `description`.
    pub doc: Option<String>,
    /// The linked operation.
    pub target: LinkTarget,
    /// (parameter name that may be `path.`-qualified, runtime expression or constant).
    pub params: Vec<(String, Value)>,
    /// `requestBody`: runtime expression or constant for the linked body.
    pub request_body: Option<Value>,
    /// `server.url`: where the linked operation is served if not on the servers of the API.
    pub server: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// How a link refers to its operation.
pub(crate) enum LinkTarget {
    /// `operationId`.
    Id(String),
    /// `operationRef` as a pointer into the document.
    Pointer(String),
}

#[derive(Debug, Clone)]
/// One operation: a method on a path.
#[expect(clippy::struct_field_names, reason = "`operation_id` is the OpenAPI field it holds")]
pub(crate) struct Operation {
    /// Name that is unique in the API.
    pub name: Name,
    /// HTTP method. It is uppercase for fixed fields and as written for `additionalOperations`.
    pub method: String,
    /// Path template such as `/orders/{id}`.
    pub path: String,
    /// Description or summary.
    pub doc: Option<String>,
    /// `deprecated: true`.
    pub deprecated: bool,
    /// Parameters with the path-level ones merged in.
    pub params: Vec<Param>,
    /// The request body if there is one.
    pub body: Option<Body>,
    /// Responses: explicit codes, then ranges and then default.
    pub responses: Vec<Resp>,
    /// Alternatives (OR) of scheme sets (AND). Empty = no auth.
    pub security: Vec<Vec<Requirement>>,
    /// Where the operation comes from.
    pub kind: OpKind,
    /// `operationId`.
    pub operation_id: Option<String>,
    /// `tags` in order.
    pub tags: Vec<String>,
    /// The `servers` of the operation or else of its path item. Empty means the servers of the API.
    pub servers: Vec<crate::ir::Server>,
    /// Pointer to the operation object.
    pub pointer: String,
}

impl Operation {
    /// Returns every media type of its request body and responses.
    pub(crate) fn contents(&self) -> impl Iterator<Item = &Content> {
        self.body.iter().flat_map(|b| &b.contents).chain(self.responses.iter().flat_map(|r| &r.contents))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Where an operation is declared.
pub(crate) enum OpKind {
    /// Under `paths`: the API serves it.
    Path,
    /// Under `webhooks`: the API sends it to a subscriber's URL.
    Webhook {
        /// Key in `webhooks`.
        name: String,
    },
    /// Under an operation's `callbacks`: the API sends it after that operation.
    Callback {
        /// The operation declaring the callback.
        parent: Name,
        /// Key in `callbacks`.
        name: String,
        /// Runtime expression of the target URL such as `{$request.body#/callbackUrl}`.
        expression: String,
    },
}

#[derive(Debug, Clone)]
/// The API's HTTP side.
pub(crate) struct Http {
    /// Every operation in document order.
    pub operations: Vec<Operation>,
}

impl Http {
    /// Builds every operation; `schemes` are the ones its `security` may name.
    pub(crate) fn build(b: &mut Builder<'_>, schemes: &[SecurityScheme]) -> Self {
        Self { operations: operations(b, schemes) }
    }

    /// Requests carry the request types of `readOnly` / `writeOnly` schemas. `request_of` maps each type to its
    /// request type if it has one.
    pub(crate) fn use_request_types(&mut self, request_of: &[Option<TypeId>]) {
        for op in &mut self.operations {
            for p in &mut op.params {
                p.ty = TypeSpace::request_ty(&p.ty, request_of);
            }
            for c in op.body.iter_mut().flat_map(|b| b.contents.iter_mut()) {
                c.ty = TypeSpace::request_ty(&c.ty, request_of);
            }
        }
    }
}

const FIXED: &[&str] = &["get", "put", "post", "delete", "options", "head", "patch", "trace", "query"];

fn operations(b: &mut Builder<'_>, schemes: &[SecurityScheme]) -> Vec<Operation> {
    let root = b.doc.root.clone();
    let global_security = root.get("security").cloned();
    let mut ops = vec![];
    let mut taken = b.reserved_operations.clone();
    let add_item = |b: &mut Builder<'_>,
                    path: &str,
                    item_ptr: &str,
                    kind: &OpKind,
                    ops: &mut Vec<Operation>,
                    taken: &mut BTreeSet<String>| {
        let Some(item) = resolve_obj(b, item_ptr) else { return };
        let (item, item_ptr) = item;
        let shared = item.get("parameters").cloned();
        let mut methods: Vec<(String, String)> = FIXED
            .iter()
            .filter(|m| item.get(**m).is_some())
            .map(|m| (m.to_ascii_uppercase(), join(&item_ptr, m)))
            .collect();
        if let Some(Value::Object(extra)) = item.get("additionalOperations") {
            for k in extra.keys() {
                methods.push((k.clone(), join(&join(&item_ptr, "additionalOperations"), k)));
            }
        }
        for (method, op_ptr) in methods {
            let op = pointer(&b.doc.root, &op_ptr).cloned().unwrap_or_default();
            let fallback = match kind {
                OpKind::Path => format!("{} {}", method.to_lowercase(), path),
                OpKind::Webhook { name } => format!("{name} {}", method.to_lowercase()),
                OpKind::Callback { parent, name, .. } => {
                    format!("{} {name} {}", parent.words().join(" "), method.to_lowercase())
                }
            };
            let base = Name::new(op.get("operationId").and_then(Value::as_str).unwrap_or(&fallback));
            let name = unique(base, taken);
            let security = op.get("security").or(global_security.as_ref());
            let site = Site {
                name: &name,
                method: &method,
                path,
                op: &op,
                op_ptr: &op_ptr,
                shared: shared.as_ref(),
                item_ptr: &item_ptr,
            };
            let mut built = operation(b, &site, security, schemes);
            built.kind = kind.clone();
            ops.push(built);
        }
    };
    let mut path_ops = vec![];
    if let Some(Value::Object(paths)) = root.get("paths") {
        for (path, _) in paths.iter().filter(|(k, _)| !k.starts_with("x-")) {
            let before = ops.len();
            add_item(b, path, &join("/paths", path), &OpKind::Path, &mut ops, &mut taken);
            path_ops.push((join("/paths", path), before..ops.len()));
        }
    }
    // Webhooks: path items keyed by name. The API sends them to subscribers.
    if let Some(Value::Object(hooks)) = root.get("webhooks") {
        for (name, _) in hooks.iter().filter(|(k, _)| !k.starts_with("x-")) {
            let kind = OpKind::Webhook { name: name.clone() };
            add_item(b, "", &join("/webhooks", name), &kind, &mut ops, &mut taken);
        }
    }
    for (ptr, kind) in callback_items(b, &root, &ops, path_ops) {
        add_item(b, "", &ptr, &kind, &mut ops, &mut taken);
    }
    ops
}

/// Returns callbacks: per path operation a map of runtime expressions to path
/// items. Returns (path item pointer, kind) for each.
fn callback_items(
    b: &Builder<'_>,
    root: &Value,
    ops: &[Operation],
    path_ops: Vec<(String, std::ops::Range<usize>)>,
) -> Vec<(String, OpKind)> {
    let mut callbacks = vec![];
    for (item_ptr, range) in path_ops {
        let item = resolve_obj(b, &item_ptr).map(|(_, p)| p).unwrap_or(item_ptr);
        for i in range {
            let parent = ops[i].name.clone();
            let op_ptr = join(&item, &ops[i].method.to_lowercase());
            let Some(Value::Object(cbs)) = pointer(root, &op_ptr).and_then(|o| o.get("callbacks")).cloned() else {
                continue;
            };
            for (cb_name, _) in cbs.iter().filter(|(k, _)| !k.starts_with("x-")) {
                let cb_ptr = join(&join(&op_ptr, "callbacks"), cb_name);
                let Some((Value::Object(exprs), cb_ptr)) = resolve_obj(b, &cb_ptr) else { continue };
                for expression in exprs.keys().filter(|k| !k.starts_with("x-")) {
                    let kind = OpKind::Callback {
                        parent: parent.clone(),
                        name: cb_name.clone(),
                        expression: expression.clone(),
                    };
                    callbacks.push((join(&cb_ptr, expression), kind));
                }
            }
        }
    }
    callbacks
}

/// Multipart parts nested inside multipart parts that get their own encoding.
const MAX_ENCODING_DEPTH: usize = 3;

fn resolve_obj(b: &Builder<'_>, ptr: &str) -> Option<(Value, String)> {
    let mut v = pointer(&b.doc.root, ptr)?.clone();
    let mut p = ptr.to_owned();
    for _ in 0..MAX_REF_HOPS {
        match v.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                p = b.doc.local_pointer(r)?;
                let target = pointer(&b.doc.root, &p)?.clone();
                // Path items merge sibling fields over the referenced item.
                if let (Value::Object(mut t), Value::Object(own)) = (target.clone(), v.clone()) {
                    for (k, val) in own {
                        if k != "$ref" {
                            t.insert(k, val);
                        }
                    }
                    v = Value::Object(t);
                } else {
                    v = target;
                }
            }
            None => return Some((v, p)),
        }
    }
    None
}

/// Returns the `summary` and `description` of an operation's path item.
fn path_item_doc(b: &Builder<'_>, site: &Site<'_>) -> Option<String> {
    let item = resolve_obj(b, site.item_ptr).map(|(v, _)| v)?;
    let text = |k: &str| item.get(k).and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(String::from);
    match (text("summary"), text("description")) {
        (Some(s), Some(d)) => Some(format!("{s}\n\n{d}")),
        (s, d) => s.or(d),
    }
}

/// Where an operation lives in the document.
struct Site<'a> {
    name: &'a Name,
    method: &'a str,
    path: &'a str,
    op: &'a Value,
    op_ptr: &'a str,
    shared: Option<&'a Value>,
    item_ptr: &'a str,
}

fn operation(b: &mut Builder<'_>, site: &Site<'_>, security: Option<&Value>, schemes: &[SecurityScheme]) -> Operation {
    let prefix = site.name.clone();
    let op = site.op;
    let mut params = params(b, site, &prefix);
    let parts = part_headers(b, site, &prefix, &params);
    params.extend(parts);
    let servers: Vec<crate::ir::Server> = op
        .get("servers")
        .or_else(|| pointer(&b.doc.root, site.item_ptr).and_then(|i| i.get("servers")))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(server).collect())
        .unwrap_or_default();
    // Of several servers the caller picks one by name: a position would follow the document order.
    let keys = reserved(&servers.iter().filter_map(|s| s.name.as_deref()).collect::<Vec<_>>());
    if servers.len() > 1 && keys.len() < servers.len() {
        b.diags.error(
            format!("{}/servers", site.op_ptr),
            "several servers need a distinct `name` (or `description`) each: the client picks one by it",
        );
    }
    Operation {
        name: site.name.clone(),
        method: site.method.to_owned(),
        path: site.path.to_owned(),
        doc: crate::ir::types::with_external_docs(
            crate::ir::types::text_of(op)
                .or_else(|| op.get("summary").and_then(Value::as_str).map(String::from))
                // The path item's summary and description apply to all its operations.
                .or_else(|| path_item_doc(b, site)),
            op,
        ),
        deprecated: op.get("deprecated") == Some(&Value::Bool(true)),
        params,
        body: body(b, site, &prefix),
        responses: responses(b, site, &prefix),
        security: requirements(security, schemes),
        kind: OpKind::Path,
        operation_id: op.get("operationId").and_then(Value::as_str).map(String::from),
        tags: op
            .get("tags")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        servers,
        pointer: site.op_ptr.to_owned(),
    }
}

/// Returns the type, `content` media type and constraints of a parameter. They come from
/// the schema that describes the value. That is `schema` or the schema of the one media type
/// under `content`. Its `$ref` (3.2 `components.mediaTypes`) is followed.
fn param_value(
    b: &mut Builder<'_>,
    p: &Value,
    pptr: &str,
    hint: &Name,
) -> (Ty, Option<MediaKind>, crate::ir::types::Constraints) {
    let media = p.get("content").and_then(Value::as_object).and_then(|c| c.iter().next()).and_then(|(media, m)| {
        let (m, mptr) = resolve_value(b, m, &join(&join(pptr, "content"), media))?;
        Some((media.clone(), m, mptr))
    });
    let content_schema = media.as_ref().and_then(|(_, m, _)| m.get("schema"));
    let constraints = p.get("schema").or(content_schema).map(|s| b.constraints(s)).unwrap_or_default();
    match &media {
        Some((media, m, mptr)) => {
            let s = json::schema(m);
            (b.ty(&s, &join(mptr, "schema"), hint), Some(MediaKind::of(media)), constraints)
        }
        None => {
            let s = json::schema(p);
            (b.ty(&s, &join(pptr, "schema"), hint), None, constraints)
        }
    }
}

/// Returns parameters: operation-level ones override path-level ones (name + in).
fn params(b: &mut Builder<'_>, site: &Site<'_>, prefix: &Name) -> Vec<Param> {
    let mut raw: Vec<(Value, String)> = vec![];
    let lists = [
        (site.op.get("parameters"), join(site.op_ptr, "parameters")),
        (site.shared, join(site.item_ptr, "parameters")),
    ];
    for (list, base) in lists {
        let Some(Value::Array(ps)) = list else { continue };
        for (i, p) in ps.iter().enumerate() {
            let Some((p, pptr)) = resolve_value(b, p, &format!("{base}/{i}")) else { continue };
            let key = (p.get("name").cloned(), p.get("in").cloned());
            if !raw.iter().any(|(q, _)| (q.get("name").cloned(), q.get("in").cloned()) == key) {
                raw.push((p, pptr));
            }
        }
    }
    let mut taken = reserved(&["body"]);
    let mut out = vec![];
    for (p, pptr) in raw {
        let wire = p.get("name").and_then(Value::as_str).unwrap_or("param").to_owned();
        let location = match p.get("in").and_then(Value::as_str) {
            Some("path") => Location::Path,
            Some("header") => Location::Header,
            Some("cookie") => Location::Cookie,
            Some("querystring") => Location::QueryString,
            _ => Location::Query,
        };
        // A path parameter missing from the template can never be sent.
        if location == Location::Path && !site.path.contains(&format!("{{{wire}}}")) {
            b.diags.warning(&pptr, format!("path parameter `{wire}` is not in `{}`; ignoring it", site.path));
            continue;
        }
        // Content-Type, Accept and Authorization headers are managed by the runtime.
        if location == Location::Header
            && ["content-type", "accept", "authorization"].contains(&wire.to_ascii_lowercase().as_str())
        {
            continue;
        }
        // A parameter of `components/parameters` takes its type name as a component schema does: after its key and not
        // after the first operation that uses it.
        let hint = match pptr.strip_prefix("/components/parameters/") {
            Some(key) if !key.contains('/') => Name::new(&json::unescape_token(key)),
            _ => prefix.and(&Name::new(&wire)),
        };
        let (ty, content, constraints) = param_value(b, &p, &pptr, &hint);
        let style = if p.get("x-collection-format").and_then(Value::as_str) == Some("tsv") {
            Style::TabDelimited
        } else {
            style_of(p.get("style").and_then(Value::as_str), location)
        };
        let explode = p.get("explode").and_then(Value::as_bool).unwrap_or(matches!(style, Style::Form | Style::Cookie));
        let required = location == Location::Path || json::flag(&p, "required");
        let default =
            p.get("schema").and_then(crate::ir::types::literal_default).filter(|_| !required && b.literal_type(&ty));
        out.push(Param {
            name: Name::new(&wire),
            wire,
            location,
            ty: if required || default.is_some() { ty } else { ty.option() },
            style,
            explode,
            allow_reserved: p.get("allowReserved").and_then(Value::as_bool).unwrap_or(false),
            content,
            doc: json::string(&p, "description"),
            constraints,
            examples: param_examples(b, &p, &pptr),
            allow_empty: p.get("allowEmptyValue") == Some(&Value::Bool(true)),
            deprecated: p.get("deprecated") == Some(&Value::Bool(true)),
            default,
            part: None,
            serialized: serialized_of(b, &p, &pptr),
        });
    }
    let names = unique_sorted(
        out.iter().map(|x| (x.name.clone(), (x.wire.len(), x.wire.clone(), x.location))).collect(),
        &mut taken,
    );
    for (x, n) in out.iter_mut().zip(names) {
        x.name = n;
    }
    out
}

/// Returns `encoding.<property>.headers` of a `multipart/form-data` request body.
/// There is one parameter per header. The part of that property carries it.
fn part_headers(b: &mut Builder<'_>, site: &Site<'_>, prefix: &Name, params: &[Param]) -> Vec<Param> {
    let mut out = vec![];
    let Some((rb, rbptr)) =
        site.op.get("requestBody").and_then(|rb| resolve_value(b, rb, &join(site.op_ptr, "requestBody")))
    else {
        return out;
    };
    let Some(Value::Object(content)) = rb.get("content") else { return out };
    let Some((media, m)) = content.iter().find(|(m, _)| MediaKind::of(m) == MediaKind::Multipart) else { return out };
    // With other media types the body may not be multipart: the headers may be absent.
    let only = content.len() == 1;
    let Some((m, mptr)) = resolve_value(b, m, &join(&join(&rbptr, "content"), media)) else { return out };
    let Some(Value::Object(encoding)) = m.get("encoding") else { return out };
    let mut taken: BTreeSet<String> = params.iter().map(|p| p.name.key()).chain([Name::new("body").key()]).collect();
    for (prop, e) in encoding {
        let Some(Value::Object(hs)) = e.get("headers") else { continue };
        let eptr = join(&join(&mptr, "encoding"), prop);
        // `Content-Type` is `encoding.contentType`'s; `Content-Disposition` is the runtime's.
        for (hname, h) in hs
            .iter()
            .filter(|(n, _)| !n.eq_ignore_ascii_case("content-type") && !n.eq_ignore_ascii_case("content-disposition"))
        {
            let Some((h, hptr)) = resolve_value(b, h, &join(&join(&eptr, "headers"), hname)) else { continue };
            let s = json::schema(&h);
            let ty = b.ty(&s, &join(&hptr, "schema"), &prefix.and(&Name::new(prop)).and(&Name::new(hname)));
            let required = only && json::flag(&h, "required");
            out.push(Param {
                name: Name::new(&format!("{prop}_{hname}")),
                wire: hname.clone(),
                location: Location::Part,
                ty: if required { ty } else { ty.option() },
                style: Style::Simple,
                explode: h.get("explode").and_then(Value::as_bool).unwrap_or(false),
                allow_reserved: false,
                content: None,
                doc: json::string(&h, "description"),
                constraints: h.get("schema").map(|s| b.constraints(s)).unwrap_or_default(),
                examples: examples_of(b, &h, &hptr, false),
                allow_empty: false,
                deprecated: h.get("deprecated") == Some(&Value::Bool(true)),
                default: None,
                part: Some(prop.clone()),
                serialized: serialized_of(b, &h, &hptr),
            });
        }
    }
    let names = unique_sorted(
        out.iter().map(|x| (x.name.clone(), (x.part.clone(), x.wire.len(), x.wire.clone()))).collect(),
        &mut taken,
    );
    for (x, n) in out.iter_mut().zip(names) {
        x.name = n;
    }
    out
}

/// Returns the documented examples of a parameter, with the `default` of its schema: the test of the examples checks
/// that too.
fn param_examples(b: &mut Builder<'_>, p: &Value, ptr: &str) -> Vec<(String, Value)> {
    let mut out = examples_of(b, p, ptr, false);
    if let Some(d) = p.get("schema").and_then(|s| s.get("default")) {
        out.push((join(&join(ptr, "schema"), "default"), d.clone()));
    }
    out
}

/// Returns the documented examples of a parameter or media type as (JSON pointer, value).
///
/// Reads `example` and `examples` (Example Objects with `$ref`s followed). 3.2
/// `dataValue` wins over `value`. For a JSON media type (`json: true`)
/// `serializedValue` and an `externalValue` file (relative to the
/// description) are parsed as JSON.
pub(crate) fn examples_of(b: &mut Builder<'_>, obj: &Value, ptr: &str, json: bool) -> Vec<(String, Value)> {
    let mut out = vec![];
    if let Some(v) = obj.get("example") {
        out.push((join(ptr, "example"), v.clone()));
    }
    if let Some(Value::Object(m)) = obj.get("examples") {
        for (k, e) in m {
            let at = join(&join(ptr, "examples"), k);
            let Some((e, _)) = resolve_value(b, e, &at) else { continue };
            if let Some(v) = e.get("dataValue").or_else(|| e.get("value")) {
                out.push((at, v.clone()));
            } else if let Some(text) = e.get("serializedValue").and_then(Value::as_str).filter(|_| json) {
                match serde_json::from_str(text) {
                    Ok(v) => out.push((join(&at, "serializedValue"), v)),
                    Err(err) => b.diags.warning(join(&at, "serializedValue"), format!("not JSON: {err}")),
                }
            } else if let Some(uri) = e.get("externalValue").and_then(Value::as_str).filter(|_| json) {
                let at = join(&at, "externalValue");
                let read = b.doc.externals.get(uri).cloned().unwrap_or_else(|| Err(format!("`{uri}` was not read")));
                let parsed =
                    read.and_then(|t| serde_json::from_str(&t).map_err(|err| format!("`{uri}` is not JSON: {err}")));
                match parsed {
                    Ok(v) => out.push((at, v)),
                    Err(err) => b.diags.warning(&at, format!("example not checked: {err}")),
                }
            }
        }
    }
    out
}

/// Examples of a parameter or header that give both `dataValue` and
/// `serializedValue` as (JSON pointer, value, serialized form).
fn serialized_of(b: &Builder<'_>, obj: &Value, ptr: &str) -> Vec<(String, Value, String)> {
    let Some(Value::Object(m)) = obj.get("examples") else { return vec![] };
    m.iter()
        .filter_map(|(k, e)| {
            let at = join(&join(ptr, "examples"), k);
            let (e, _) = resolve_value(b, e, &at)?;
            let data = e.get("dataValue")?.clone();
            if let Some(wire) = e.get("serializedValue").and_then(Value::as_str) {
                return Some((join(&at, "serializedValue"), data, wire.to_owned()));
            }
            // The serialized form in a file of its own (`./examples/orders.xml`). It was read when the document loaded.
            let uri = e.get("externalValue")?.as_str()?;
            let text = b.doc.externals.get(uri)?.as_ref().ok()?.clone();
            Some((join(&at, "externalValue"), data, text))
        })
        .collect()
}

/// Returns form fields whose `encoding` sets `style`, `explode` or `allowReserved`
/// with the shape of their property schema.
fn form_styles(
    b: &Builder<'_>,
    media_obj: &Value,
    schema: Option<&Value>,
    mptr: &str,
) -> Vec<(String, Style, bool, bool, &'static str)> {
    media_obj
        .get("encoding")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, e)| e.get("style").is_some() || e.get("explode").is_some() || e.get("allowReserved").is_some())
        .map(|(prop, e)| {
            let style = style_of(e.get("style").and_then(Value::as_str), Location::Query);
            let explode = e.get("explode").and_then(Value::as_bool).unwrap_or(style == Style::Form);
            let ps = schema
                .and_then(|sc| resolve_value(b, sc, mptr))
                .and_then(|(sc, _)| sc.pointer(&format!("/properties/{}", escape_token(prop))).cloned())
                .and_then(|ps| resolve_value(b, &ps, mptr))
                .map(|(ps, _)| ps);
            let shape = match ps.as_ref().and_then(|ps| ps.get("type")).and_then(Value::as_str) {
                Some("array") => "Array",
                Some("object") => "Object",
                _ => "Prim",
            };
            (prop.clone(), style, explode, e.get("allowReserved").and_then(Value::as_bool).unwrap_or(false), shape)
        })
        .collect()
}

fn style_of(style: Option<&str>, location: Location) -> Style {
    match style {
        Some("label") => Style::Label,
        Some("matrix") => Style::Matrix,
        Some("form") => Style::Form,
        Some("spaceDelimited") => Style::SpaceDelimited,
        Some("pipeDelimited") => Style::PipeDelimited,
        Some("deepObject") => Style::DeepObject,
        Some("cookie") => Style::Cookie,
        Some("simple") => Style::Simple,
        _ => match location {
            Location::Query | Location::QueryString | Location::Cookie => Style::Form,
            Location::Path | Location::Header | Location::Part => Style::Simple,
        },
    }
}

fn body(b: &mut Builder<'_>, site: &Site<'_>, prefix: &Name) -> Option<Body> {
    let rb = site.op.get("requestBody")?;
    let (rb, rbptr) = resolve_value(b, rb, &join(site.op_ptr, "requestBody"))?;
    let contents = contents(b, &rb, &rbptr, &prefix.and(&Name::new("Body")));
    if contents.is_empty() {
        return None;
    }
    // The body's type checks every keyword of its schema where it decodes: the body holds no constraints of its own.
    let constraints = crate::ir::types::Constraints::default();
    let variant_constraints = vec![];
    Some(Body {
        required: json::flag(&rb, "required"),
        contents,
        doc: json::string(&rb, "description"),
        constraints,
        variant_constraints,
    })
}

/// Returns `default`, `2XX` or `404`. Anything else is not a response key.
fn status_of(code: &str) -> Option<Status> {
    if code == "default" {
        return Some(Status::Default);
    }
    if let Ok(c) = code.parse::<u16>() {
        return Some(Status::Code(c));
    }
    let digit = code.strip_suffix("XX").or_else(|| code.strip_suffix("xx"))?;
    (digit.len() == 1).then(|| digit.parse().ok().map(Status::Range)).flatten()
}

/// Returns the `links` of a response with `$ref`s followed. Links without a local target are dropped.
fn links(b: &Builder<'_>, r: &Value, rptr: &str) -> Vec<Link> {
    let Some(Value::Object(ls)) = r.get("links") else { return vec![] };
    ls.iter()
        .filter(|(k, _)| !k.starts_with("x-"))
        .filter_map(|(name, l)| {
            let (l, _) = resolve_value(b, l, &join(&join(rptr, "links"), name))?;
            let target = if let Some(id) = l.get("operationId").and_then(Value::as_str) {
                LinkTarget::Id(id.to_owned())
            } else {
                let r = l.get("operationRef").and_then(Value::as_str)?;
                LinkTarget::Pointer(json::percent_decode(&b.doc.local_pointer(r)?))
            };
            let params = match l.get("parameters") {
                Some(Value::Object(ps)) => ps.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                _ => vec![],
            };
            Some(Link {
                name: name.clone(),
                doc: doc_of(&l),
                target,
                params,
                request_body: l.get("requestBody").cloned(),
                server: l.pointer("/server/url").and_then(Value::as_str).map(String::from),
            })
        })
        .collect()
}

/// Returns responses: explicit codes, then ranges and then default. `responses` is optional since 3.1. Without any
/// there is one `default` without a body. The handler then picks the status.
fn responses(b: &mut Builder<'_>, site: &Site<'_>, prefix: &Name) -> Vec<Resp> {
    let mut out = vec![];
    let rs = site.op.get("responses").and_then(Value::as_object);
    let mut taken = BTreeSet::new();
    for (code, r) in rs.into_iter().flatten().filter(|(k, _)| !k.starts_with("x-")) {
        let Some(status) = status_of(code) else { continue };
        let Some((r, rptr)) = resolve_value(b, r, &join(&join(site.op_ptr, "responses"), code)) else { continue };
        let variant = match status {
            Status::Code(c) => status_name(c),
            Status::Range(n) => range_name(n),
            Status::Default => Name::new("Default"),
        };
        let variant = unique(variant, &mut taken);
        let mut contents = contents(b, &r, &rptr, &prefix.and(&variant));
        for c in &mut contents {
            c.response = true;
        }
        let headers = response_headers(b, &r, &rptr, &prefix.and(&variant));
        // 3.2 `summary` leads; `description` follows as the body of the doc.
        let text = |k: &str| json::string(&r, k);
        let doc = match (text("summary"), text("description")) {
            (Some(s), Some(d)) if !d.is_empty() && d != s => Some(format!("{s}\n\n{d}")),
            (Some(s), _) => Some(s),
            (None, d) => d,
        };
        let links = links(b, &r, &rptr);
        let shared = rptr.strip_prefix("/components/responses/").filter(|n| !n.contains('/')).map(Name::new);
        out.push(Resp { status, variant, contents, headers, doc, links, shared });
    }
    if out.is_empty() {
        let variant = Name::new("Default");
        out.push(Resp {
            status: Status::Default,
            variant,
            contents: vec![],
            headers: vec![],
            doc: None,
            links: vec![],
            shared: None,
        });
    }
    out.sort_by_key(|r| match r.status {
        Status::Code(c) => (0, c),
        Status::Range(n) => (1, n),
        Status::Default => (2, 0),
    });
    out
}

fn response_headers(b: &mut Builder<'_>, r: &Value, rptr: &str, prefix: &Name) -> Vec<ResponseHeader> {
    let mut taken = reserved(&["status", "body"]);
    let mut out = vec![];
    let Some(Value::Object(hs)) = r.get("headers") else { return out };
    for (hname, h) in hs.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("content-type")) {
        let Some((h, hptr)) = resolve_value(b, h, &join(&join(rptr, "headers"), hname)) else { continue };
        // `content` (one media type) instead of `schema`. The header carries
        // the serialization of that media type typed by its schema. JSON is
        // written as JSON text. Other media types (`text/plain`) carry the
        // text of the value itself as the `simple` style does.
        let media = h.get("content").and_then(Value::as_object).and_then(|m| m.iter().next()).and_then(|(m, mo)| {
            let (mo, mptr) = resolve_value(b, mo, &join(&join(&hptr, "content"), m))?;
            Some((m.clone(), mo, mptr))
        });
        let json = media.as_ref().is_some_and(|(m, ..)| MediaKind::of(m) == MediaKind::Json);
        let linkset = media.as_ref().is_some_and(|(m, ..)| MediaKind::of(m) == MediaKind::Linkset);
        let (s, sptr) = match &media {
            Some((_, mo, mptr)) => (json::schema(mo), join(mptr, "schema")),
            None => (json::schema(&h), join(&hptr, "schema")),
        };
        let required = json::flag(&h, "required");
        let ty = b.ty(&s, &sptr, &prefix.and(&Name::new(hname)));
        out.push(ResponseHeader {
            name: Name::new(hname),
            wire: hname.clone(),
            ty: if required { ty } else { ty.option() },
            json,
            linkset,
            doc: json::string(&h, "description"),
            deprecated: h.get("deprecated") == Some(&Value::Bool(true)),
            explode: h.get("explode").and_then(Value::as_bool).unwrap_or(false),
            examples: examples_of(b, &h, &hptr, false),
        });
    }
    let names =
        unique_sorted(out.iter().map(|x| (x.name.clone(), (x.wire.len(), x.wire.clone()))).collect(), &mut taken);
    for (x, n) in out.iter_mut().zip(names) {
        x.name = n;
    }
    out
}

/// Returns variant name of a status code: its RFC 9110 reason phrase (`NotFound`),
/// or `Status{code}` for codes without a registered one.
fn status_name(c: u16) -> Name {
    Name::new(&runtime::__private::reason(c).map_or_else(|| format!("Status{c}"), str::to_owned))
}

/// Returns variant name of a `nXX` range: its class (`ClientError`).
fn range_name(n: u16) -> Name {
    Name::new(&runtime::__private::class(n).map_or_else(|| format!("Status{n}XX"), str::to_owned))
}

/// Returns the media type of each part of a content of `kind`: by position for `multipart/mixed`, by property
/// otherwise.
fn part_types(
    b: &Builder<'_>,
    kind: MediaKind,
    media_obj: &Value,
    schema: Option<&Value>,
    mptr: &str,
    open_prefix: Option<usize>,
) -> Vec<(String, String)> {
    if kind == MediaKind::MultipartMixed {
        positional_encoding(media_obj, open_prefix)
    } else if kind == MediaKind::Multipart {
        schema_part_types(b, media_obj, schema, mptr)
    } else {
        property_encoding(media_obj)
    }
}

/// Returns [`property_encoding`] of a `multipart/form-data` media type, with the type that a property's schema gives
/// where `encoding` gives none: its `contentMediaType` (3.2 section 4.14.4.1).
fn schema_part_types(b: &Builder<'_>, media_obj: &Value, schema: Option<&Value>, mptr: &str) -> Vec<(String, String)> {
    let mut out = property_encoding(media_obj);
    let props = schema.and_then(|sc| resolve_value(b, sc, mptr)).and_then(|(sc, _)| sc.get("properties").cloned());
    for (prop, ps) in props.as_ref().and_then(Value::as_object).into_iter().flatten() {
        let Some((ps, _)) = resolve_value(b, ps, mptr) else { continue };
        if let Some(media) = ps.get("contentMediaType").and_then(Value::as_str)
            && !out.iter().any(|(p, _)| p == prop)
        {
            out.push((prop.clone(), media.to_owned()));
        }
    }
    out
}

/// Returns `encoding.<property>.contentType` of a form or multipart media type. Inside a
/// `multipart/*` part also its own `encoding` (`meta/inner`),
/// `prefixEncoding` (`files/0`) and `itemEncoding` (`files/*`).
fn property_encoding(media_obj: &Value) -> Vec<(String, String)> {
    let mut out = vec![];
    for (prop, enc) in media_obj.get("encoding").and_then(Value::as_object).into_iter().flatten() {
        nested_encoding(prop, enc, &mut out, 0);
    }
    out
}

fn nested_encoding(path: &str, enc: &Value, out: &mut Vec<(String, String)>, depth: usize) {
    let ct = enc.get("contentType").and_then(Value::as_str);
    if let Some(ct) = ct {
        out.push((path.to_owned(), ct.to_owned()));
    }
    // Nested encodings apply to a part that is itself multipart.
    if depth >= MAX_ENCODING_DEPTH
        || !ct.is_some_and(|ct| ct.trim_start().to_ascii_lowercase().starts_with("multipart/"))
    {
        return;
    }
    for (k, e) in enc.get("encoding").and_then(Value::as_object).into_iter().flatten() {
        nested_encoding(&format!("{path}/{k}"), e, out, depth + 1);
    }
    for (i, e) in json::items(enc.get("prefixEncoding")) {
        nested_encoding(&format!("{path}/{i}"), e, out, depth + 1);
    }
    if let Some(e) = enc.get("itemEncoding") {
        nested_encoding(&format!("{path}/*"), e, out, depth + 1);
    }
}

fn contents(b: &mut Builder<'_>, holder: &Value, ptr: &str, hint: &Name) -> Vec<Content> {
    let Some(Value::Object(content)) = holder.get("content") else { return vec![] };
    let mut out = vec![];
    let mut taken = BTreeSet::new();
    for (media, media_obj) in content {
        let Some((media_obj, mptr)) = resolve_value(b, media_obj, &join(&join(ptr, "content"), media)) else {
            continue;
        };
        let mut kind = MediaKind::of(media);
        let item_schema = media_obj.get("itemSchema");
        let schema = media_obj.get("schema");
        // 3.2 itemSchema on a JSON media type also means a sequence.
        if item_schema.is_some() && kind == MediaKind::Json {
            kind = MediaKind::JsonLines;
        }
        let schema_ty = |b: &mut Builder<'_>| schema.map_or(Ty::Any, |s| b.ty(s, &join(&mptr, "schema"), hint));
        let item_hint = hint.and(&Name::new("Item"));
        let mut open_prefix = None;
        let ty = match kind {
            MediaKind::Text => Ty::String,
            MediaKind::Bytes => Ty::Bytes,
            MediaKind::JsonLines | MediaKind::JsonSeq | MediaKind::Sse => {
                let item_ty = match (item_schema, schema.and_then(|s| s.get("items"))) {
                    (Some(item), _) => b.ty(item, &join(&mptr, "itemSchema"), &item_hint),
                    (None, Some(items)) => b.ty(items, &join(&join(&mptr, "schema"), "items"), &item_hint),
                    // Before 3.2 (no `itemSchema`) the item type is in `schema`. A non-array schema describes one item.
                    (None, None) => match schema.map(|s| b.ty(s, &join(&mptr, "schema"), &item_hint)) {
                        Some(Ty::Vec(item)) => *item,
                        Some(item) => item,
                        None => Ty::Any,
                    },
                };
                Ty::Vec(Box::new(if kind == MediaKind::Sse { fields_if_any(item_ty) } else { item_ty }))
            }
            MediaKind::Json | MediaKind::Linkset => schema_ty(b),
            MediaKind::Multipart => match schema.and_then(|s| binary_parts(s, &media_obj)) {
                Some(s) => fields_if_any(b.ty(&s, &join(&mptr, "schema"), hint)),
                None => fields_if_any(schema_ty(b)),
            },
            MediaKind::Form => fields_if_any(schema_ty(b)),
            MediaKind::MultipartMixed => {
                let (ty, prefix) = mixed_ty(b, &media_obj, &mptr, hint);
                open_prefix = prefix;
                ty.unwrap_or_else(|| {
                    kind = MediaKind::Bytes;
                    Ty::Bytes
                })
            }
            // XML maps object types; anything else stays the raw text.
            MediaKind::Xml => {
                let ty = schema_ty(b);
                if b.is_struct(&ty) {
                    ty
                } else {
                    kind = MediaKind::Text;
                    Ty::String
                }
            }
        };
        let variant = unique(Name::new(&media_variant(media)), &mut taken);
        let encoding = part_types(b, kind, &media_obj, schema, &mptr, open_prefix);
        let examples = examples_of(b, &media_obj, &mptr, kind == MediaKind::Json);
        let form_styles = if kind == MediaKind::Form { form_styles(b, &media_obj, schema, &mptr) } else { vec![] };
        out.push(Content {
            doc: json::string(&media_obj, "description"),
            media: media.clone(),
            kind,
            ty,
            variant,
            encoding,
            response: false,
            positional_prefix: open_prefix,
            examples,
            form_styles,
            serialized: if kind == MediaKind::Xml { serialized_of(b, &media_obj, &mptr) } else { vec![] },
            transfer: if kind == MediaKind::Multipart { base64_parts(b, &media_obj, &mptr) } else { vec![] },
            item_limits: item_limits(b, kind, schema),
        });
    }
    out
}

/// Returns `minItems`, `maxItems` and `uniqueItems` of the `schema` of a sequence. None for any other kind of body.
fn item_limits(b: &Builder<'_>, kind: MediaKind, schema: Option<&Value>) -> ItemLimits {
    let Some(c) = schema.filter(|_| kind.is_sequence()).map(|s| b.constraints(s)) else {
        return ItemLimits::default();
    };
    ItemLimits { min: c.min_items, max: c.max_items, unique: c.unique_items }
}

/// Returns the properties of a multipart object whose schema says `contentEncoding: base64`.
fn base64_parts(b: &Builder<'_>, media_obj: &Value, mptr: &str) -> Vec<String> {
    let Some((schema, _)) = media_obj.get("schema").and_then(|s| resolve_value(b, s, &join(mptr, "schema"))) else {
        return vec![];
    };
    schema
        .get("properties")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, p)| {
            let p = resolve_value(b, p, "").map_or_else(|| (*p).clone(), |(v, _)| v);
            p.get("contentEncoding").and_then(Value::as_str) == Some("base64")
        })
        .map(|(k, _)| k.clone())
        .collect()
}

/// Returns every media type of an `encoding.contentType` list is binary (not JSON, text or XML).
fn all_binary(ct: &str) -> bool {
    ct.split(',').all(|m| crate::ir::types::binary_media(m.trim()))
}

/// Returns an empty schema (`{}`, `true` or annotations only). It allows any instance.
fn is_empty_schema(s: &Value) -> bool {
    *s == Value::Bool(true) || crate::ir::types::is_annotation_only(s)
}

/// Returns the parts of a multipart object with an empty schema. 3.2 sends them as `application/octet-stream`
/// (Encoding Object `contentType` defaults and "Multipart Form with Multiple Files": `items: {}`).
/// They are rewritten as binary strings so that they type as bytes. A part whose `encoding.contentType` is
/// JSON or text keeps its empty schema. Only an inline object is rewritten. A `$ref`'d one may be JSON elsewhere.
fn binary_parts(schema: &Value, media_obj: &Value) -> Option<Value> {
    let props = schema.get("properties")?.as_object()?;
    let binary = |prop: &str| {
        let ct = media_obj.pointer(&format!("/encoding/{}/contentType", escape_token(prop))).and_then(Value::as_str);
        ct.is_none_or(all_binary)
    };
    let bytes = || serde_json::json!({"type": "string", "contentMediaType": "application/octet-stream"});
    let mut out = schema.clone();
    let mut changed = false;
    for (name, p) in props {
        if !binary(name) {
            continue;
        }
        let at = format!("/properties/{}", escape_token(name));
        if is_empty_schema(p) {
            *out.pointer_mut(&at)? = bytes();
            changed = true;
        } else if p.get("type").and_then(Value::as_str) == Some("array") && p.get("items").is_some_and(is_empty_schema)
        {
            *out.pointer_mut(&format!("{at}/items"))? = bytes();
            changed = true;
        }
    }
    changed.then_some(out)
}

/// Returns form fields, multipart parts and SSE events are always named values on the
/// wire. Without a schema (`{}` or none) type them as a map. A bare JSON value cannot round-trip through them.
fn fields_if_any(ty: Ty) -> Ty {
    if ty == Ty::Any { Ty::Map(Box::new(Ty::Any)) } else { ty }
}

/// Returns the type of a positional multipart body. It is `Vec<Item>` for `itemSchema` or
/// `items` and a tuple for closed `prefixItems`. It is `(P0, .., Vec<Item>)` for
/// `prefixItems` followed by `items` (with the prefix length). `None` means not an array.
fn mixed_ty(b: &mut Builder<'_>, media_obj: &Value, mptr: &str, hint: &Name) -> (Option<Ty>, Option<usize>) {
    let (item_schema, schema) = (media_obj.get("itemSchema"), media_obj.get("schema"));
    // A part with an empty schema is `application/octet-stream` (Encoding Object defaults) unless its encoding
    // says JSON or text. So it is bytes, as the streaming and byte-range examples of 3.2 send images and video.
    let part = |b: &mut Builder<'_>, s: &Value, at: &str, name: &Name, enc: Option<&Value>| {
        let ct = enc.and_then(|e| e.get("contentType")).and_then(Value::as_str);
        let binary = ct.is_none_or(all_binary);
        if binary && is_empty_schema(s) { Ty::Binary } else { b.ty(s, at, name) }
    };
    if let Some(item) = item_schema {
        let ty = part(b, item, &join(mptr, "itemSchema"), &hint.and(&Name::new("Item")), media_obj.get("itemEncoding"));
        return (Some(Ty::Vec(Box::new(ty))), None);
    }
    let Some(s) = schema else { return (None, None) };
    let Some((sv, sptr)) = resolve_value(b, s, &join(mptr, "schema")) else { return (None, None) };
    if let (Some(Value::Array(prefix)), Some(items)) = (sv.get("prefixItems"), sv.get("items"))
        && *items != Value::Bool(false)
    {
        let encodings = media_obj.get("prefixEncoding").and_then(Value::as_array);
        let mut tys: Vec<Ty> = prefix
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let at = join(&join(&sptr, "prefixItems"), &i.to_string());
                part(b, p, &at, &hint.and(&Name::new(&format!("Part{i}"))), encodings.and_then(|e| e.get(i)))
            })
            .collect();
        let item = part(b, items, &join(&sptr, "items"), &hint.and(&Name::new("Item")), media_obj.get("itemEncoding"));
        tys.push(Ty::Vec(Box::new(item)));
        return (Some(Ty::Tuple(tys)), Some(prefix.len()));
    }
    let ty = b.ty(s, &join(mptr, "schema"), hint);
    (matches!(ty, Ty::Vec(_) | Ty::Tuple(_)).then_some(ty), None)
}

/// Returns `prefixEncoding` / `itemEncoding` content types keyed by position or `*`.
fn positional_encoding(media_obj: &Value, open_prefix: Option<usize>) -> Vec<(String, String)> {
    let ct = |e: &Value| e.get("contentType").and_then(Value::as_str).map(String::from);
    let mut out: Vec<(String, String)> = media_obj
        .get("prefixEncoding")
        .and_then(Value::as_array)
        .map(|es| es.iter().enumerate().filter_map(|(i, e)| Some((i.to_string(), ct(e)?))).collect())
        .unwrap_or_default();
    if let Some(c) = media_obj.get("itemEncoding").and_then(ct) {
        out.push(("*".to_owned(), c));
    }
    if let Some(n) = open_prefix {
        out.push(("prefix".to_owned(), n.to_string()));
    }
    out
}

fn media_variant(media: &str) -> String {
    let m = media.split(';').next().unwrap_or(media);
    match m {
        "*/*" => "Any".into(),
        _ => m.rsplit('/').next().unwrap_or(m).replace(['+', '.', '-', '*'], " "),
    }
}

/// Returns one Server Object (url, name, variables).
pub(crate) fn server(s: &Value) -> Option<super::Server> {
    let url = s.get("url")?.as_str()?.to_owned();
    let variables = s
        .get("variables")
        .and_then(Value::as_object)
        .map(|vars| {
            vars.iter()
                .map(|(name, v)| {
                    let default = v.get("default").and_then(Value::as_str).unwrap_or("").to_owned();
                    let allowed = v.get("enum").and_then(Value::as_array).map(|a| json::strings(a)).unwrap_or_default();
                    (name.clone(), default, allowed)
                })
                .collect()
        })
        .unwrap_or_default();
    let text = |k: &str| json::string(s, k);
    let mut lines: Vec<String> = text("description").into_iter().collect();
    for (name, v) in s.get("variables").and_then(Value::as_object).into_iter().flatten() {
        if let Some(d) = v.get("description").and_then(Value::as_str).filter(|d| !d.trim().is_empty()) {
            lines.push(format!("`{name}`: {}", d.trim()));
        }
    }
    let doc = (!lines.is_empty()).then(|| lines.join("\n\n"));
    Some(super::Server { url, name: text("name").or_else(|| text("description")), variables, doc })
}

#[cfg(test)]
mod tests {
    use crate::ir::names::Name;

    /// Variant names are API: the reason phrases of the `http` crate must keep giving the names users match on. A
    /// phrase that it renames fails here before it renames a variant in generated code.
    #[test]
    fn statuses_keep_their_variant_names() {
        let names = [
            (200, "Ok"),
            (203, "NonAuthoritativeInformation"),
            (207, "MultiStatus"),
            (226, "ImUsed"),
            (414, "UriTooLong"),
            (413, "PayloadTooLarge"),
            (418, "I M A Teapot"),
            (422, "UnprocessableEntity"),
            (505, "HttpVersionNotSupported"),
            (599, "Status599"),
        ];
        for (code, name) in names {
            assert_eq!(super::status_name(code), Name::new(name), "{code}");
        }
        assert_eq!(super::range_name(4), Name::new("ClientError"));
        assert_eq!(super::range_name(9), Name::new("Status9XX"));
    }
}
