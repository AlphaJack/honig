//! The text that the generator writes around the code stays lean. There are no banner comments,
//! no docs that only restate a name and no sample domain that leaks from
//! the own examples of the generator. Fast because it generates every fixture in memory.

use std::path::{Path, PathBuf};

/// Doc lines that say nothing the item's name does not.
const BOILERPLATE: &[(&str, &str)] = &[
    ("/// The `", "` property."),
    ("/// The `", "` parameter."),
    ("/// The `", "` query parameter."),
    ("/// The `", "` header parameter."),
    ("/// The `", "` path parameter."),
    ("/// The `", "` cookie parameter."),
    ("/// The `", "` response header."),
    ("/// Schema `", "`."),
    ("/// Variant `", "`."),
    ("/// HTTP method.", ""),
    ("/// Path template.", ""),
    ("/// Everything the client sends.", ""),
    ("/// Every documented response.", ""),
    ("/// HTTP status code.", ""),
    ("/// Request body.", ""),
    ("/// Response body.", ""),
    ("/// The value as sent on the wire.", ""),
    ("/// A value this version of the API description does not list.", ""),
    ("/// A value that matches no known variant.", ""),
];

fn fixtures() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
    let mut out = vec![];
    for dir in ["v31", "v32", "v30"] {
        for entry in std::fs::read_dir(root.join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "yaml") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// A whole word `product` or `products` in any case.
fn mentions_products(line: &str) -> bool {
    line.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|w| w.eq_ignore_ascii_case("product") || w.eq_ignore_ascii_case("products"))
}

#[test]
fn generated_text_has_no_banners_boilerplate_docs_or_products() {
    let mut hits = vec![];
    for spec in fixtures() {
        let spec_mentions_products = mentions_products(&std::fs::read_to_string(&spec).unwrap());
        let generated = crate::generate::generate(&spec, &crate::emit::Options::default())
            .unwrap_or_else(|d| panic!("{}: {:?}", spec.display(), d.0));
        for (file, text) in &generated.files {
            for (i, line) in text.lines().enumerate() {
                let t = line.trim();
                let at = || format!("{}: {file}:{}: {t}", spec.display(), i + 1);
                if t.starts_with("// ===") || t.starts_with("// ---") {
                    hits.push(format!("banner: {}", at()));
                }
                if BOILERPLATE.iter().any(|(start, end)| t.starts_with(start) && t.ends_with(end)) {
                    hits.push(format!("boilerplate doc: {}", at()));
                }
                // honig is the generator's own text; other files may echo the spec.
                if (file == "runtime.rs" || !spec_mentions_products) && t.starts_with("//") && mentions_products(t) {
                    hits.push(format!("products: {}", at()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "{} lines:\n{}", hits.len(), hits.join("\n"));
}
