//! Numbers are named constants or `match self` arms. The exceptions are -1, 0, 1, 2 and 100: identities, pairs and
//! halves, percent and the error return of C. Code elsewhere passes values. Generated code (what a `quote!` builds)
//! is left out. Its numbers come from the description.

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::{self, Visit};

use super::is_test;

/// Numbers outside the places they belong.
#[derive(Default)]
struct Numbers(Vec<String>);

impl Numbers {
    fn literal(&mut self, literal: &syn::Lit, line: usize) {
        match literal {
            syn::Lit::Int(n) if !matches!(n.base10_digits(), "-1" | "0" | "1" | "2" | "100") => {
                self.0.push(format!("{line}: {n}"));
            }
            syn::Lit::Float(n) if !matches!(n.base10_digits(), "0.0" | "1.0" | "100.0") => {
                self.0.push(format!("{line}: {n}"));
            }
            // `Lit` is non-exhaustive.
            syn::Lit::Str(_)
            | syn::Lit::ByteStr(_)
            | syn::Lit::CStr(_)
            | syn::Lit::Byte(_)
            | syn::Lit::Char(_)
            | syn::Lit::Int(_)
            | syn::Lit::Float(_)
            | syn::Lit::Bool(_)
            | syn::Lit::Verbatim(_)
            | _ => {}
        }
    }

    /// A constant names a number.
    fn constant(&mut self, visit: impl FnOnce(&mut Self)) {
        let before = self.0.len();
        visit(self);
        self.0.truncate(before);
    }

    fn tokens(&mut self, tokens: TokenStream) {
        let mut tokens = tokens.into_iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                // Attributes such as doc comments in the input of a macro.
                TokenTree::Punct(p) if p.as_char() == '#' => {
                    tokens.next_if(
                        |t| matches!(t, TokenTree::Group(g) if g.delimiter() == proc_macro2::Delimiter::Bracket),
                    );
                }
                TokenTree::Group(group) => self.tokens(group.stream()),
                TokenTree::Literal(literal) => {
                    if let Ok(parsed) = syn::parse_str::<syn::Lit>(&literal.to_string()) {
                        self.literal(&parsed, literal.span().start().line);
                    }
                }
                TokenTree::Ident(_) | TokenTree::Punct(_) => {}
            }
        }
    }
}

impl<'ast> Visit<'ast> for Numbers {
    fn visit_item_const(&mut self, c: &'ast syn::ItemConst) {
        self.constant(|v| visit::visit_item_const(v, c));
    }

    fn visit_item_static(&mut self, s: &'ast syn::ItemStatic) {
        self.constant(|v| visit::visit_item_static(v, s));
    }

    fn visit_impl_item_const(&mut self, c: &'ast syn::ImplItemConst) {
        self.constant(|v| visit::visit_impl_item_const(v, c));
    }

    /// Array lengths in types follow from the values.
    fn visit_type(&mut self, _: &'ast syn::Type) {}

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if !is_test(&m.attrs) {
            visit::visit_item_mod(self, m);
        }
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !is_test(&f.attrs) {
            visit::visit_item_fn(self, f);
        }
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if !is_test(&f.attrs) {
            visit::visit_impl_item_fn(self, f);
        }
    }

    /// A `match self` arm may name a number such as the status code of a variant.
    fn visit_expr_match(&mut self, m: &'ast syn::ExprMatch) {
        let on_self = matches!(&*m.expr, syn::Expr::Path(p) if p.path.is_ident("self"))
            || matches!(&*m.expr, syn::Expr::Unary(u) if matches!(&*u.expr, syn::Expr::Path(p) if p.path.is_ident("self")));
        let before = self.0.len();
        visit::visit_expr_match(self, m);
        if on_self {
            self.0.truncate(before);
        }
    }

    fn visit_lit(&mut self, literal: &'ast syn::Lit) {
        self.literal(literal, literal.span().start().line);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        // Messages for developers and generated code. What a `quote!` builds is the crate of the user. Its numbers
        // come from the description.
        let skipped = ["panic", "unreachable", "debug_assert", "assert", "assert_eq", "quote"];
        if !skipped.iter().any(|name| m.path.is_ident(name)) {
            self.tokens(m.tokens.clone());
        }
    }
}

#[test]
fn code_contains_no_magic_numbers() {
    let mut found = vec![];
    for (file, source) in super::all_sources().into_iter().filter(|(f, _)| !super::is_test_file(f)) {
        let mut numbers = Numbers::default();
        numbers.visit_file(&syn::parse_file(&source).unwrap());
        found.extend(numbers.0.into_iter().map(|n| format!("{file}: {n}")));
    }
    assert!(found.is_empty(), "{} numbers without a name:\n{}", found.len(), found.join("\n"));
}
