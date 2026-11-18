//! Every fixture description in `tests/fixtures/<version>/<name>.yaml`, as `<version>_<name>::api`.
#![warn(clippy::pedantic)]
#![warn(clippy::allow_attributes)]
#![warn(clippy::allow_attributes_without_reason)]
#![warn(clippy::clone_on_ref_ptr)]
#![warn(clippy::indexing_slicing)]
#![warn(clippy::let_underscore_untyped)]
#![warn(clippy::iter_on_empty_collections)]
#![warn(clippy::iter_on_single_items)]
#![warn(clippy::map_with_unused_argument_over_ranges)]
#![warn(clippy::missing_const_for_fn)]
#![warn(clippy::too_long_first_doc_paragraph)]
#![warn(clippy::use_self)]
#![warn(clippy::wildcard_enum_match_arm)]
pub mod v20_collection_formats {
    pub mod api;
}
pub mod v30_edge {
    pub mod api;
}
pub mod v31_upgrade {
    pub mod api;
}
pub mod v32_catalog {
    pub mod api;
}
pub mod v32_conditional {
    pub mod api;
}
pub mod v32_conformance {
    pub mod api;
}
pub mod v32_coverage {
    pub mod api;
}
pub mod v32_download {
    pub mod api;
}
pub mod v32_formats {
    pub mod api;
}
pub mod v32_generics {
    pub mod api;
}
pub mod v32_kitchen_sink {
    pub mod api;
}
pub mod v32_lambda {
    pub mod api;
}
pub mod v32_lenient {
    pub mod api;
}
pub mod v32_mtls {
    pub mod api;
}
pub mod v32_multipart {
    pub mod api;
}
pub mod v32_named_formats {
    pub mod api;
}
pub mod v32_routing {
    pub mod api;
}
pub mod v32_scopes {
    pub mod api;
}
pub mod v32_shop {
    pub mod api;
}
pub mod v32_spec_examples {
    pub mod api;
}
pub mod v32_spec_xml {
    pub mod api;
}
pub mod v32_sse_items {
    pub mod api;
}
pub mod v32_styles {
    pub mod api;
}
pub mod v32_tuples {
    pub mod api;
}
pub mod v32_uploads {
    pub mod api;
}
pub mod v32_validation {
    pub mod api;
}
pub mod v32_webhooks {
    pub mod api;
}
pub mod v32_webhooks_only {
    pub mod api;
}
pub mod v32_xml {
    pub mod api;
}
