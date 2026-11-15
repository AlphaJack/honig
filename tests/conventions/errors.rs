//! A function of the generator that fails returns a variant of an error enum and not a string. That is `error::Error`
//! in the library and the own enum of the command in its binary. Problems in a description are diagnostics instead.

use syn::{
    GenericArgument, ImplItemFn, ItemFn, PathArguments, ReturnType, Signature, Type,
    visit::{self, Visit},
};

/// The generator's sources: the runtime's decoders hand their messages to generated code.
const GENERATOR: &str = "src/";

/// The lines of functions returning `Result<_, String>`.
#[derive(Default)]
struct StringErrors(Vec<usize>);

impl StringErrors {
    fn check(&mut self, sig: &Signature) {
        let ReturnType::Type(_, ty) = &sig.output else { return };
        let Type::Path(path) = &**ty else { return };
        let Some(last) = path.path.segments.last().filter(|s| s.ident == "Result") else { return };
        let PathArguments::AngleBracketed(args) = &last.arguments else { return };
        let error = args.args.iter().nth(1);
        if matches!(error, Some(GenericArgument::Type(Type::Path(e))) if e.path.is_ident("String")) {
            self.0.push(sig.ident.span().start().line);
        }
    }
}

impl<'ast> Visit<'ast> for StringErrors {
    fn visit_item_fn(&mut self, f: &'ast ItemFn) {
        self.check(&f.sig);
        visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast ImplItemFn) {
        self.check(&f.sig);
        visit::visit_impl_item_fn(self, f);
    }
}

fn string_errors(source: &str) -> Vec<usize> {
    let mut found = StringErrors::default();
    found.visit_file(&syn::parse_file(source).unwrap());
    found.0
}

#[test]
fn failures_are_variants_not_strings() {
    let found: Vec<String> = super::all_sources()
        .into_iter()
        .filter(|(file, _)| file.starts_with(GENERATOR) && !super::is_test_file(file))
        .flat_map(|(file, source)| string_errors(&source).into_iter().map(move |line| format!("{file}:{line}")))
        .collect();
    assert!(
        found.is_empty(),
        "a function returning `Result<_, String>`, instead of an error enum:\n{}",
        found.join("\n")
    );
}

#[test]
fn only_string_errors_are_found() {
    let source = "fn a() -> Result<u8, String> { todo!() }\nfn b() -> Result<u8, Error> { todo!() }\n";
    assert_eq!(string_errors(source), [1]);
}
