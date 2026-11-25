//! Every honig function that reads input is a path of `honig/benches/paths`, or [`BOUNDED`] says why its input cannot
//! grow. `scaling` checks the paths for growth. A function that it leaves out could grow quadratically unseen.

use std::fs;

use syn::visit::Visit;

use super::{root, sources};

/// The argument types of input: text, bytes, query pairs, a streamed body.
const INPUT: [&str; 4] = ["& str", "& [u8]", "Query", "ByteStream"];

/// Collects the names of public free functions that encode or decode a body or read input.
struct Codecs(Vec<String>);

impl Visit<'_> for Codecs {
    fn visit_item_fn(&mut self, f: &syn::ItemFn) {
        let name = f.sig.ident.to_string();
        let reads = f.sig.inputs.iter().any(|a| {
            let text = quote::ToTokens::to_token_stream(a).to_string();
            INPUT.iter().any(|t| text.contains(t))
        });
        let codec = name.ends_with("_encode") || name.ends_with("_decode");
        if matches!(f.vis, syn::Visibility::Public(_)) && (codec || reads) {
            self.0.push(name);
        }
    }
}

#[test]
fn every_runtime_codec_is_a_bench_path() {
    let mut found = vec![];
    sources(&root().join("honig").join("src"), &mut found);
    let mut codecs = Codecs(vec![]);
    for (_, text) in &found {
        codecs.visit_file(&syn::parse_file(text).unwrap());
    }
    let scaling = fs::read_to_string(root().join("honig").join("benches").join("support").join("paths.rs")).unwrap();
    let timed = |name: &str| {
        scaling.match_indices(name).any(|(at, _)| {
            let before = scaling.get(..at).and_then(|s| s.chars().next_back());
            let after = scaling.get(at + name.len()..).and_then(|s| s.chars().next());
            let edge = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
            edge(before) && edge(after)
        })
    };
    let bounded = |n: &str| BOUNDED.iter().any(|(_, names)| names.contains(&n));
    let missing: Vec<&String> = codecs.0.iter().filter(|n| !timed(n) && !bounded(n)).collect();
    assert!(!codecs.0.is_empty(), "no codecs found: the scan is broken");
    assert!(missing.is_empty(), "add to honig/benches/support/paths.rs: {missing:?}");
}

/// Functions whose input never comes from a request, by why.
const BOUNDED: &[(&str, &[&str])] = &[
    (
        "the caller's own credentials and OAuth configuration",
        &[
            "apply_auth",
            "authorization_url",
            "client_credentials",
            "device_authorization",
            "device_token",
            "discover",
            "exchange_code",
            "implicit_url",
            "password_grant",
            "refresh_token",
        ],
    ),
    ("text of the description, written into the generated code", &["server_url", "template"]),
];
