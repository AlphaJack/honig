//! Every body codec that honig offers is a path of `honig/benches/paths`. `scaling` checks those for growth. A
//! codec that it leaves out could grow quadratically unseen.

use std::fs;

use syn::visit::Visit;

use super::{root, sources};

/// Collects the names of public free functions that encode or decode a body.
struct Codecs(Vec<String>);

impl Visit<'_> for Codecs {
    fn visit_item_fn(&mut self, f: &syn::ItemFn) {
        let name = f.sig.ident.to_string();
        if matches!(f.vis, syn::Visibility::Public(_)) && (name.ends_with("_encode") || name.ends_with("_decode")) {
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
    let missing: Vec<&String> = codecs.0.iter().filter(|n| !timed(n)).collect();
    assert!(!codecs.0.is_empty(), "no codecs found: the scan is broken");
    assert!(missing.is_empty(), "add to honig/benches/support/paths.rs: {missing:?}");
}
