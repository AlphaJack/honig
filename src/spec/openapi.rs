//! `OpenAPI` 3.2 objects (everything except the Schema Object).
//!
//! Field order follows the 3.2.0 specification. Collections are `Option` so an
//! explicit empty value (`parameters: []`) survives a roundtrip.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Entries, ExternalDocs, Extra, Map, RefOr, Schema, present};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenApi {
    pub openapi: String,
    #[serde(rename = "$self", default, skip_serializing_if = "Option::is_none")]
    pub self_uri: Option<String>,
    pub info: Info,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json_schema_dialect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<Server>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Paths>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub webhooks: Option<Map<PathItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<Components>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<SecurityRequirement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<Tag>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocs>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Info {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact: Option<Contact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<License>,
    pub version: String,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Contact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct License {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identifier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Server {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 3.2: a unique reference name for the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<Map<ServerVariable>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct ServerVariable {
    #[serde(rename = "enum", default, skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    pub default: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Components {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schemas: Option<Map<Schema>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses: Option<Map<RefOr<Response>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<RefOr<Parameter>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Map<RefOr<Example>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_bodies: Option<Map<RefOr<RequestBody>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<RefOr<Header<MediaType>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_schemes: Option<Map<RefOr<SecurityScheme>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Map<RefOr<Link>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<Map<RefOr<Callback>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_items: Option<Map<PathItem>>,
    /// 3.2: reusable Media Type Objects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_types: Option<Map<RefOr<MediaType>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// Paths Object: path template to Path Item, plus `x-*` extensions.
pub(crate) type Paths = Entries<PathItem>;

/// Callback Object: runtime expression to Path Item, plus `x-*` extensions.
pub(crate) type Callback = Entries<PathItem>;

/// Responses Object: status code / range / `default` to Response.
pub(crate) type Responses = Entries<RefOr<Response>>;

/// Security Requirement Object: scheme name (or URI in 3.2) to scopes / roles.
pub(crate) type SecurityRequirement = Map<Vec<String>>;

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PathItem {
    /// A Path Item may itself be a reference with sibling fields.
    #[serde(rename = "$ref", default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub put: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub patch: Option<Operation<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Operation<Self>>,
    /// 3.2: the HTTP QUERY method.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Operation<Self>>,
    /// 3.2: any other method keyed by its exact (case-sensitive) name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_operations: Option<Map<Operation<Self>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<Server>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RefOr<Parameter>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl PathItem {
    /// Returns every operation with its HTTP method. Fixed fields come first in spec order.
    /// Then `additionalOperations` in document order.
    pub(crate) fn operations(&self) -> Vec<(&str, &Operation<Self>)> {
        let fixed = [
            ("GET", &self.get),
            ("PUT", &self.put),
            ("POST", &self.post),
            ("DELETE", &self.delete),
            ("OPTIONS", &self.options),
            ("HEAD", &self.head),
            ("PATCH", &self.patch),
            ("TRACE", &self.trace),
            ("QUERY", &self.query),
        ];
        let mut out: Vec<(&str, &Operation<Self>)> =
            fixed.into_iter().filter_map(|(m, op)| op.as_ref().map(|op| (m, op))).collect();
        if let Some(extra) = &self.additional_operations {
            out.extend(extra.iter().map(|(m, op)| (m.as_str(), op)));
        }
        out
    }
}

/// An Operation Object. `P` is the type of the path items of its callbacks: [`PathItem`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(bound(deserialize = "P: serde::de::DeserializeOwned"))]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields are named as the OpenAPI fields they hold")]
pub(crate) struct Operation<P> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Vec<RefOr<Parameter>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RefOr<RequestBody>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses: Option<Responses>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callbacks: Option<Map<RefOr<Entries<P>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<Vec<SecurityRequirement>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub servers: Option<Vec<Server>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ParameterLocation {
    Query,
    /// 3.2: the entire query string described by `content`.
    Querystring,
    Header,
    Path,
    Cookie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ParameterStyle {
    Matrix,
    Label,
    Simple,
    Form,
    SpaceDelimited,
    PipeDelimited,
    DeepObject,
    /// 3.2: RFC 6265 cookie serialization.
    Cookie,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Parameter {
    pub name: String,
    #[serde(rename = "in")]
    pub location: ParameterLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_empty_value: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ParameterStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_reserved: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Map<RefOr<Example>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Map<RefOr<MediaType>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// A Header Object. `M` is the type of the media types in its `content`: [`MediaType`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(bound(deserialize = "M: serde::de::DeserializeOwned"))]
#[serde(rename_all = "camelCase")]
pub(crate) struct Header<M> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ParameterStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Map<RefOr<Example>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Map<RefOr<M>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct RequestBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Required by the spec. `Option` so a document missing it still roundtrips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Map<RefOr<MediaType>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaType {
    /// Not in 3.2.0; added on the OAI `v3.2-dev` branch (used by components.mediaTypes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
    /// 3.2: schema of each item in a sequential media type (JSON Lines, SSE, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_schema: Option<Schema>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub example: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<Map<RefOr<Example>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Map<Encoding<Header<Self>>>>,
    /// 3.2: positional encodings for `multipart/mixed` style bodies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_encoding: Option<Vec<Encoding<Header<Self>>>>,
    /// 3.2: encoding for every item after `prefixEncoding`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_encoding: Option<Encoding<Header<Self>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// An Encoding Object. `H` is the type of its headers: [`Header`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(bound(deserialize = "H: serde::de::DeserializeOwned"))]
#[serde(rename_all = "camelCase")]
#[expect(clippy::struct_field_names, reason = "fields are named as the OpenAPI fields they hold")]
pub(crate) struct Encoding<H> {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<RefOr<H>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<ParameterStyle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_reserved: Option<bool>,
    /// 3.2: nested encodings for multipart parts that are themselves multipart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<Map<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix_encoding: Option<Vec<Self>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_encoding: Option<Box<Self>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct Response {
    /// 3.2: short summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Required before 3.2 and optional since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<Map<RefOr<Header<MediaType>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Map<RefOr<MediaType>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Map<RefOr<Link>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Example {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 3.2: the example as a data value (before serialization).
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub data_value: Option<Value>,
    /// 3.2: the example exactly as it appears on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serialized_value: Option<String>,
    /// Deprecated in 3.2 in favour of `dataValue` / `serializedValue`.
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_value: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Link {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Map<Value>>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub request_body: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<Server>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Tag {
    pub name: String,
    /// 3.2: short display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_docs: Option<ExternalDocs>,
    /// 3.2: name of the parent tag for nesting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// 3.2: free-form category such as `nav`, `badge` or `audience`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum SecuritySchemeType {
    #[serde(rename = "apiKey")]
    ApiKey,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "mutualTLS")]
    MutualTls,
    #[serde(rename = "oauth2")]
    OAuth2,
    #[serde(rename = "openIdConnect")]
    OpenIdConnect,
}

/// Flat on purpose: one struct for all scheme types keeps the model lossless
/// even for documents that set fields belonging to another type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecurityScheme {
    #[serde(rename = "type")]
    pub kind: SecuritySchemeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// apiKey: name of the header, query or cookie parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// apiKey: `query`, `header` or `cookie`.
    #[serde(rename = "in", default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// http: RFC 9110 scheme name such as `bearer` or `basic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flows: Option<OAuthFlows>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_id_connect_url: Option<String>,
    /// 3.2: RFC 8414 authorization server metadata URL.
    #[serde(rename = "oauth2MetadataUrl", default, skip_serializing_if = "Option::is_none")]
    pub oauth2_metadata_url: Option<String>,
    /// 3.2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated: Option<bool>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthFlows {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implicit: Option<OAuthFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<OAuthFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_credentials: Option<OAuthFlow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_code: Option<OAuthFlow>,
    /// 3.2: RFC 8628 device authorization grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_authorization: Option<OAuthFlow>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OAuthFlow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
    /// 3.2
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_authorization_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_url: Option<String>,
    /// Required by the spec. `Option` so a document missing it still roundtrips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Map<String>>,
    #[serde(flatten)]
    pub extra: Extra,
}
