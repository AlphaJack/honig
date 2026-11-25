//! Rust code emission. Output is a module directory (`mod.rs` + submodules)
//! that lives inside the user's crate.
//!
//! A spec type named like a prelude item (`String`, `Result`, `Option`) gets a suffix
//! (`names::off_prelude`). So it cannot shadow it.
//! Text from the spec only reaches code through `lit()` (string literals) or
//! sanitized identifiers.

mod checks;
mod http;
pub(crate) mod macro_text;
pub(crate) mod names;
pub(crate) mod needs;
mod space;
pub(crate) mod syntax;
mod types;

use proc_macro2::TokenStream;
use quote::quote;

use crate::error::Error;
use crate::ir::Api;
use space::Space;

/// Version of honig the generated code needs: always the generator's own.
pub(crate) const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Default)]
/// What only Rust output needs. It comes from the `Cargo.toml` of the target crate.
pub(crate) struct Options {
    /// Extra derives from the settings as (Rust type name or `*`, derive paths).
    pub derives: Vec<(String, Vec<String>)>,
    /// Libraries the target crate provides for string formats.
    pub libs: crate::settings::FormatLibs,
    /// The crate is a binary: items are `pub(crate)` and may go unused.
    pub binary: bool,
    /// The crate's dependencies by name and by package name: an object-form `x-rust-type` refers to one of them.
    pub crates: Vec<String>,
}

/// Returns what only the emitter sees in a description it can generate: the required headers of error responses.
pub(crate) fn warnings(api: &Api, opts: &super::Options) -> Vec<crate::diag::Diagnostic> {
    if !opts.server {
        return vec![];
    }
    http::unset_error_headers(api)
}

/// Every file of a generated module and what it contains. The book lists them on its Generated module page, and its
/// test checks the list against a generated module.
#[cfg(test)]
pub(crate) const FILES: [(&str, &str); 6] = [
    (
        "mod.rs",
        "The root of the module, or `<output>.rs` with `mod_rs = false`. It declares the other files and re-exports the client, the server and the items of honig that the generated signatures refer to.",
    ),
    ("types.rs", "The schemas of the description as Rust types: structs, enums and newtypes."),
    ("operations.rs", "One module per operation with its `Params`, its `Response` enum and their body types."),
    (
        "client.rs",
        "The `Client` with one method per operation, and the `WebhookClient` when the description has webhooks.",
    ),
    ("server.rs", "The `Api` trait with one method per operation, the router and the `Server` of this API."),
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

/// Returns `api` with only the listed `operations`, and every type that none of them reaches left out. `None` without a
/// list: then every one stays.
fn kept(api: &Api, opts: &super::Options) -> Option<Api> {
    use crate::ir::http::OpKind;
    use crate::ir::types::{Constraints, Kind, TypeId};
    let names = opts.operations.as_deref()?;
    let keeps = |op: &crate::ir::http::Operation| http::listed(names, op);
    let mut api = api.clone();
    let parents: std::collections::BTreeSet<String> = api
        .http
        .operations
        .iter()
        .filter(|op| matches!(op.kind, OpKind::Path) && keeps(op))
        .map(|op| op.name.key())
        .collect();
    api.http.operations.retain(|op| match &op.kind {
        OpKind::Callback { parent, .. } => parents.contains(&parent.key()),
        OpKind::Path | OpKind::Webhook { .. } => keeps(op),
    });
    let mut stack: Vec<TypeId> = Vec::new();
    let mut subschemas: Vec<usize> = Vec::new();
    for op in &api.http.operations {
        let contents = op.body.iter().flat_map(|b| &b.contents).chain(op.responses.iter().flat_map(|r| &r.contents));
        let headers = op.responses.iter().flat_map(|r| &r.headers).map(|h| &h.ty);
        for t in op.params.iter().map(|p| &p.ty).chain(contents.map(|c| &c.ty)).chain(headers) {
            crate::ir::types::all_refs(t, &mut stack);
        }
        subschemas.extend(op.params.iter().flat_map(|p| p.constraints.subschemas()));
        if let Some(b) = &op.body {
            subschemas.extend(b.constraints.subschemas());
            subschemas.extend(b.variant_constraints.iter().flat_map(Constraints::subschemas));
        }
    }
    let mut reached = vec![false; api.types.defs.len()];
    let mut seen = vec![false; api.types.subschemas.len()];
    loop {
        if let Some(i) = subschemas.pop() {
            if !std::mem::replace(&mut seen[i], true) {
                let (t, c) = &api.types.subschemas[i];
                crate::ir::types::all_refs(t, &mut stack);
                subschemas.extend(c.subschemas());
            }
            continue;
        }
        let Some(id) = stack.pop() else { break };
        if std::mem::replace(&mut reached[id.0], true) {
            continue;
        }
        let kind = &api.types.defs[id.0].kind;
        for t in crate::ir::types::def_types(kind) {
            crate::ir::types::all_refs(t, &mut stack);
        }
        match kind {
            Kind::Checked(c) => subschemas.extend(c.constraints.subschemas()),
            Kind::Struct(s) => subschemas.extend(s.fields.iter().flat_map(|f| f.constraints.subschemas())),
            Kind::Union(u) => subschemas.extend(u.variants.iter().flat_map(|v| v.constraints.subschemas())),
            Kind::Alias(_)
            | Kind::Newtype(_)
            | Kind::StrEnum(_)
            | Kind::IntEnum(_)
            | Kind::Const(_)
            | Kind::Pending => {}
        }
    }
    for (def, reached) in api.types.defs.iter_mut().zip(reached) {
        if !reached {
            def.kind = Kind::Pending;
            def.rust_type = None;
        }
    }
    Some(api)
}

/// Returns how many operations the module has: those that `operations` lists, or every one.
pub(crate) fn operation_count(api: &Api, opts: &super::Options) -> usize {
    kept(api, opts).as_ref().unwrap_or(api).http.operations.len()
}

/// Returns the names in `operations` that no operation of `api` has, by `operationId` or generated name.
pub(crate) fn unknown_operations<'a>(api: &Api, opts: &'a super::Options) -> Vec<&'a str> {
    let listed = |name: &String| api.http.operations.iter().any(|op| http::listed(std::slice::from_ref(name), op));
    opts.operations.iter().flatten().filter(|n| !listed(n)).map(String::as_str).collect()
}

/// Returns the keys of `derives` that name no generated type: a renamed schema would lose its derive without a word.
pub(crate) fn unknown_derives<'a>(api: &Api, opts: &'a super::Options) -> Vec<&'a str> {
    let known: std::collections::BTreeSet<String> = api.types.defs.iter().map(|d| names::pascal(&d.name)).collect();
    let keys = opts.rust.derives.iter().map(|(key, _)| key.as_str());
    keys.filter(|key| *key != "*" && !known.contains(*key)).collect()
}

pub(crate) fn trees(api: &Api, opts: &super::Options) -> Trees {
    let kept = kept(api, opts);
    let api = kept.as_ref().unwrap_or(api);
    // A type that its twins alias gets the name they share. The names change before anything is emitted, so every
    // file uses the same one.
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
    let root = root(api, opts, examples.is_some());
    let mut trees = vec![("types.rs", types::emit(api, space, opts))];
    trees.push(("operations.rs", http::operations(api, space, opts)));
    if opts.client {
        trees.push(("client.rs", http::client(api, space, opts)));
    }
    if opts.server {
        trees.push(("server.rs", http::server(api, space, opts.allow_unimplemented)));
    }
    if let Some(examples) = examples {
        trees.push(("documented_examples.rs", examples));
    }
    let library = library_names(&trees, &root);
    let reexport = (!library.is_empty()).then(|| quote!(pub use ::honig::{#(#library),*};));
    trees.insert(0, ("mod.rs", quote!(#root #reexport)));
    Trees { files: trees }
}

/// Returns the names of honig that `trees` refer to by `::honig::<name>` and that `root` does not declare: the library
/// items that your code meets in the generated signatures. Macros and the hidden `__private` stay out.
fn library_names(trees: &[(&str, TokenStream)], root: &TokenStream) -> Vec<proc_macro2::Ident> {
    use proc_macro2::TokenTree;
    use std::collections::BTreeSet;
    /// Adds to `used` each name after `honig ::` and to `own` every identifier of `ts`.
    fn walk(ts: TokenStream, used: &mut BTreeSet<String>, own: &mut BTreeSet<String>) {
        let tokens: Vec<TokenTree> = ts.into_iter().collect();
        for (i, t) in tokens.iter().enumerate() {
            match t {
                TokenTree::Group(g) => walk(g.stream(), used, own),
                TokenTree::Ident(id) => {
                    own.insert(id.to_string());
                }
                TokenTree::Punct(_) | TokenTree::Literal(_) => {}
            }
            if let Some(
                [TokenTree::Ident(h), TokenTree::Punct(_), TokenTree::Punct(_), TokenTree::Ident(id), rest @ ..],
            ) = tokens.get(i..)
                && h == "honig"
                && id != "__private"
                && !matches!(rest.first(), Some(TokenTree::Punct(p)) if p.as_char() == '!')
            {
                used.insert(id.to_string());
            }
        }
    }
    let (mut used, mut declared) = (BTreeSet::new(), BTreeSet::new());
    for (_, tokens) in trees {
        walk(tokens.clone(), &mut used, &mut BTreeSet::new());
    }
    walk(root.clone(), &mut BTreeSet::new(), &mut declared);
    used.difference(&declared).map(|n| syntax::ident(n)).collect()
}

/// Prints the files of `trees` as Rust source. `binary`: for a binary crate.
///
/// # Errors
/// Tokens that do not form a Rust file. That is a bug in the generator. The message points to it.
pub(crate) fn print(trees: Trees, binary: bool) -> Result<Vec<(String, String)>, Error> {
    // Every file: the `@generated` marker (code review tools collapse such files in diffs).
    let header = "// @generated by honig. Do not edit: change the OpenAPI document and rerun `cargo honig generate`.\n";
    let mut files = vec![];
    for (name, tokens) in trees.files {
        let code = syntax::print(name, tokens, binary)?;
        files.push((name.to_owned(), format!("{header}{code}")));
    }
    Ok(files)
}

/// Returns the generated `mod.rs`: the docs of the module, its files and the re-exports that your code refers to.
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
        // In a binary too, so `api::Client` resolves the same in every crate. `mod.rs` of a binary allows the unused
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
        // honig's types typed by the generated `Operation`. They shadow honig's own names that the root
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
    // The root re-exports the items of honig that the generated signatures refer to (`trees`), so your code refers to
    // `api::ApiError` as it refers to `api::types`. A binary may refer to none of them there: rustc reports that as an
    // unused import.
    let unused = opts.rust.binary.then(
        || quote!(#![allow(unused_imports, reason = "a binary may refer to honig's items by their path in honig, and the client or the server by its path")]),
    );
    // The `#![allow]`s of `syntax::file_allow`, each with its reason, apply to every file of the module. Otherwise the
    // code passes strict clippy and rustdoc lint sets.
    let allow = syntax::file_allow(syntax::has_deprecated(api));
    // `rustfmt::skip`, on `mod.rs` and on each child: prettyplease prints the files and its layout differs from that
    // of rustfmt. So the `cargo fmt --check` of the crate leaves them alone.
    quote! {
        #title
        #description
        #![cfg_attr(rustfmt, rustfmt::skip)]
        #allow
        #unused
        #[rustfmt::skip]
        pub mod operations;
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
