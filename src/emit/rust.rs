//! Rust code emission. Output is a module directory (`mod.rs` + submodules)
//! that lives inside the user's crate.
//!
//! Generated code names std and serde items by absolute path (`::std::...`).
//! So spec types called `String`, `Result` or `Option` cannot shadow them.
//! Text from the spec only reaches code through `lit()` (string literals) or
//! sanitized identifiers.

mod http;
pub(crate) mod names;
pub(crate) mod needs;
mod space;
pub(crate) mod syntax;
mod types;
mod validate;

use proc_macro2::TokenStream;
use quote::quote;

use crate::error::Error;
use crate::ir::Api;
use space::Space;

/// Version of the runtime crate the generated code needs: always the generator's own.
pub(crate) const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default)]
/// What only Rust output needs. It is read from the `Cargo.toml` of the target crate.
pub(crate) struct Options {
    /// Extra derives from the settings as (Rust type name or `*`, derive paths).
    pub derives: Vec<(String, Vec<String>)>,
    /// Libraries the target crate provides for string formats.
    pub libs: crate::settings::FormatLibs,
    /// The crate is a binary: items are `pub(crate)` and may go unused.
    pub binary: bool,
    /// The crate's dependencies by name and by package name: an object-form `x-rust-type` names one of them.
    pub crates: Vec<String>,
}

/// Returns what only the emitter sees in a description it can generate: the error bodies that the server cannot fill.
pub(crate) fn warnings(api: &Api, opts: &super::Options) -> Vec<crate::diag::Diagnostic> {
    if !opts.server {
        return vec![];
    }
    let mut out = http::unfillable_errors(api, &Space::new(&api.types, &opts.rust.libs));
    out.extend(http::unset_error_headers(api));
    out
}

/// Every file of a generated module and what it holds. The book lists them on its Generated module page, and its test
/// checks the list against a generated module.
#[cfg(test)]
pub(crate) const FILES: [(&str, &str); 7] = [
    (
        "mod.rs",
        "The root of the module, or `<output>.rs` with `mod_rs = false`. It declares the other files and re-exports the client, the server and the runtime.",
    ),
    ("types.rs", "The schemas of the description as Rust types: structs, enums and newtypes."),
    ("operations.rs", "One module per operation with its `Params`, its `Response` enum and their body types."),
    (
        "client.rs",
        "The `Client` with one method per operation, and the `WebhookClient` when the description has webhooks.",
    ),
    ("server.rs", "The `Api` trait with one method per operation, the router and the `Server` of this API."),
    ("runtime.rs", "The runtime items that the generated code calls, re-exported from `honig`."),
    (
        "documented_examples.rs",
        "With `tests = true`: a test that decodes each example of the description into its type.",
    ),
];

/// The files of a generated module as syntax trees, not yet printed.
#[derive(Debug)]
pub(crate) struct Trees {
    files: Vec<(&'static str, TokenStream)>,
}

/// Returns every file of a generated module as a syntax tree, as (path relative to the module dir, tree). Each is
/// printed by `prettyplease` ([`print()`]); the `runtime.rs` is a short hand-written template over the runtime crate.
pub(crate) fn trees(api: &Api, opts: &super::Options) -> Trees {
    // A type that its twins alias takes the name they share. The names change before anything is emitted, so every
    // file reads the same one.
    let renamed = {
        let space = Space::new(&api.types, &opts.rust.libs);
        types::shared_names(&space, &types::aliases(&space, &opts.rust.derives))
    };
    let mut owned = None;
    let types = if renamed.is_empty() {
        &api.types
    } else {
        let mut t = api.types.clone();
        for (i, name) in renamed {
            t.defs[i].name = name;
        }
        owned.insert(t)
    };
    let space = Space::new(types, &opts.rust.libs);
    let space = &space;
    let examples = opts.tests.then(|| http::tests(api, space)).flatten();
    let mut trees = vec![("mod.rs", root(api, opts, examples.is_some()))];
    // A type spelt out twice (one inline schema per operation) is declared once; the others alias it.
    let aliases = types::aliases(space, &opts.rust.derives);
    let deprecated = syntax::has_deprecated(api);
    trees.push((
        "types.rs",
        types::emit(space, http::has_xml(api), &opts.rust.derives, &aliases, deprecated, opts.rust.binary),
    ));
    trees.push((
        "operations.rs",
        http::operations(api, space, !opts.rust.binary || opts.client, opts.rust.binary, opts.server),
    ));
    if opts.client {
        trees.push(("client.rs", http::client(api, space, opts.client_operations.as_deref(), opts.rust.binary)));
    }
    if opts.server {
        trees.push(("server.rs", http::server(api, space, opts.allow_unimplemented, &aliases)));
    }
    if let Some(examples) = examples {
        trees.push(("documented_examples.rs", examples));
    }
    Trees { files: trees }
}

/// Prints the files of `trees` as Rust source, with `runtime.rs` first. `binary`: for a binary crate.
///
/// # Errors
/// Tokens that do not form a Rust file. That is a bug in the generator. The message names it.
pub(crate) fn print(trees: Trees, binary: bool) -> Result<Vec<(String, String)>, Error> {
    // Every file: the `@generated` marker (code review tools collapse such files in diffs).
    let header = "// @generated by honig. Do not edit: change the OpenAPI document and rerun `cargo honig generate`.\n";
    let mut files = vec![];
    for (name, tokens) in trees.files {
        let code = syntax::print(name, tokens, binary)?;
        files.push((name.to_owned(), format!("{header}{code}")));
    }
    let runtime = runtime_reexport(binary)?;
    files.insert(0, ("runtime.rs".to_owned(), format!("{header}{runtime}")));
    Ok(files)
}

/// Returns the generated `mod.rs`: the docs of the module, its files and the re-exports that your code names.
fn root(api: &Api, opts: &super::Options, tests: bool) -> TokenStream {
    let title = format!(
        "Generated by honig from `{}` {}. Do not edit: rerun `cargo honig generate`.",
        one_line(&api.info.title),
        one_line(&api.info.version)
    );
    let description = api
        .info
        .description
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| syntax::inner_docs(&format!("\n{}", syntax::rustdoc(d))));
    let title = syntax::inner_docs(&title);
    let hooks = http::has_hooks(api);
    let client = opts.client.then(|| {
        let extra = hooks.then(|| {
            quote!(
                #[rustfmt::skip]
                pub use client::WebhookClient;
            )
        });
        // In a binary too, so `api::Client` reads the same in every crate. `mod.rs` of a binary allows the unused
        // import.
        quote! { #[rustfmt::skip] pub mod client; #[rustfmt::skip] pub use client::Client; #extra }
    });
    let server = opts.server.then(|| {
        let extra = hooks.then(|| {
            quote!(
                #[rustfmt::skip]
                pub use server::{webhook, Webhooks, WebhookOperation, WebhookContext};
            )
        });
        // The runtime's types typed by the generated `Operation`. They shadow the runtime's own names that the root
        // re-exports, so `api::Context` and `api::RequestEvent` are this API's.
        let typed = api.http.operations.iter().any(http::is_path).then(|| {
            quote!(
                #[rustfmt::skip]
                pub use server::{Operation, Context, Rejection, RequestEvent, Server};
            )
        });
        quote! { #[rustfmt::skip] pub mod server; #[rustfmt::skip] pub use server::Api; #typed #extra }
    });
    let test_mod = tests.then(|| quote! { #[cfg(test)] #[rustfmt::skip] mod documented_examples; });
    // The API of the runtime at the root of the module. Your code names `api::Context` as it names `api::types`. A
    // binary may name none of it there (only through `runtime::`). rustc reports that as an unused import.
    // `runtime` is what generated code calls: hidden, so that each item has one path in the docs of your crate.
    let runtime_crate = syntax::ident("honig");
    let unused = opts.rust.binary.then(
        || quote!(#![allow(unused_imports, reason = "a binary may name the runtime's items only through `runtime`, and the client or the server by its path")]),
    );
    // No lint attributes except that one. The code is written to pass strict clippy and
    // rustdoc lint sets. Set levels for it where the module is declared.
    // `rustfmt::skip`: prettyplease prints the files and its layout
    // differs from that of rustfmt. So the `cargo fmt --check` of the crate leaves them alone.
    quote! {
        #title
        #description
        #unused
        #[rustfmt::skip]
        pub mod operations;
        #[doc(hidden)]
        #[rustfmt::skip]
        pub mod runtime;
        pub use #runtime_crate::*;
        #[rustfmt::skip]
        pub mod types;
        #client
        #server
        #test_mod
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r', '`'], " ")
}

/// Returns the generated `runtime.rs`: its docs and the runtime crate's items.
fn runtime_reexport(binary: bool) -> Result<String, Error> {
    let runtime = syntax::ident("honig");
    let reexport = quote!(pub use #runtime::*; #[doc(hidden)] pub use #runtime::__private::*;);
    syntax::print("runtime.rs", reexport, binary)
}
