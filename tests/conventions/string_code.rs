//! Generated code is built as syntax trees. Strings reach it only as escaped literals (`syntax::lit`) or
//! validated identifiers (`syntax::ident`). They never come from parsing text into tokens. `clippy.toml` bans the `syn`
//! and `FromStr` entry points. This test also catches `.parse::<TokenStream>()` and `parse_quote!`. clippy cannot
//! ban the first without banning every `str::parse`.

use syn::{
    ExprMethodCall, ExprPath, Macro,
    visit::{self, Visit},
};

/// The emitter. It writes the generated code.
const EMITTER: &str = "src/emit/";
/// The ways text becomes tokens.
const PARSERS: &[&str] = &["parse", "from_str", "parse_str", "parse_file", "parse_quote"];

/// The lines where text is parsed with the name of the parser.
#[derive(Default)]
struct Parsed(Vec<(usize, String)>);

impl Parsed {
    fn check(&mut self, name: &syn::Ident) {
        if PARSERS.iter().any(|p| name == p) {
            self.0.push((name.span().start().line, name.to_string()));
        }
    }
}

impl<'ast> Visit<'ast> for Parsed {
    fn visit_expr_method_call(&mut self, call: &'ast ExprMethodCall) {
        self.check(&call.method);
        visit::visit_expr_method_call(self, call);
    }

    fn visit_expr_path(&mut self, path: &'ast ExprPath) {
        if let Some(last) = path.path.segments.last() {
            self.check(&last.ident);
        }
        visit::visit_expr_path(self, path);
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        if let Some(last) = mac.path.segments.last() {
            self.check(&last.ident);
        }
        visit::visit_macro(self, mac);
    }
}

fn parsed(source: &str) -> Vec<(usize, String)> {
    let mut parsed = Parsed::default();
    parsed.visit_file(&syn::parse_file(source).unwrap());
    parsed.0
}

#[test]
fn the_emitter_never_parses_text_into_code() {
    let found: Vec<String> = super::all_sources()
        .into_iter()
        .filter(|(file, _)| file.starts_with(EMITTER))
        .flat_map(|(file, source)| {
            parsed(&source).into_iter().map(move |(line, name)| format!("{file}:{line}: {name}")).collect::<Vec<_>>()
        })
        .collect();
    assert!(found.is_empty(), "text parsed into code in the emitter:\n{}", found.join("\n"));
}

#[test]
fn every_parser_is_found() {
    let source = "fn a() { s.parse::<T>(); T::from_str(s); syn::parse_str(s); parse_quote!(x); }";
    let names: Vec<String> = parsed(source).into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, ["parse", "from_str", "parse_str", "parse_quote"]);
}
