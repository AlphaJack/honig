//! A function of generated code never takes two values of one type by position: where the description orders the
//! arguments, a reorder of it would move a value to the other argument, and the code would still compile. Aliases count
//! as the type that they name. Two kinds are the exception, as nothing in a description can reorder them: the client's
//! credential methods, whose order the generator fixes (`user`, then `password`), and the methods of a trait impl,
//! whose signatures the trait fixes.

use std::collections::BTreeMap;
use std::fs;

use quote::ToTokens;
use syn::visit::Visit;
use syn::visit_mut::VisitMut;

use super::root;

/// The client's credential methods, whose order the generator fixes: `with_<scheme>` and the OAuth flows.
const FIXED_ORDER: [&str; 9] = [
    "with_",
    "authorize_",
    "authorize_password_",
    "refresh_",
    "exchange_code_",
    "authorization_url_",
    "implicit_url_",
    "device_start_",
    "device_poll_",
];

/// Aliases that a generated module declares, by name: `Sku` to `String`.
type Aliases = BTreeMap<String, syn::Type>;

/// Replaces each alias in a type by the type that it names, as deep as the aliases go.
struct Expand<'a>(&'a Aliases, usize);

impl VisitMut for Expand<'_> {
    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        const MAX_DEPTH: usize = 32;
        if let syn::Type::Path(p) = ty
            && let Some(last) = p.path.segments.last()
            && last.arguments.is_none()
            && let Some(named) = self.0.get(&last.ident.to_string())
            && self.1 < MAX_DEPTH
        {
            *ty = named.clone();
            self.1 += 1;
            self.visit_type_mut(ty);
            self.1 -= 1;
            return;
        }
        syn::visit_mut::visit_type_mut(self, ty);
    }
}

/// Returns `ty` with its aliases expanded, as text without the module that it comes from.
fn expanded(ty: &syn::Type, aliases: &Aliases) -> String {
    let mut ty = ty.clone();
    Expand(aliases, 0).visit_type_mut(&mut ty);
    ty.to_token_stream().to_string().replace("super :: ", "").replace("types :: ", "")
}

/// Collects the aliases of a file and the functions with two arguments or more.
#[derive(Default)]
struct Found {
    aliases: Aliases,
    functions: Vec<(String, Vec<syn::Type>)>,
}

impl<'ast> Visit<'ast> for Found {
    fn visit_item_type(&mut self, item: &'ast syn::ItemType) {
        self.aliases.insert(item.ident.to_string(), (*item.ty).clone());
    }

    fn visit_item_impl(&mut self, item: &'ast syn::ItemImpl) {
        // A trait fixes the signatures of its impl: `route` of honig's `Operations`, the methods of a user's `Api`.
        if item.trait_.is_none() {
            syn::visit::visit_item_impl(self, item);
        }
    }

    fn visit_signature(&mut self, sig: &'ast syn::Signature) {
        let args: Vec<syn::Type> = (sig.inputs.iter())
            .filter_map(|a| if let syn::FnArg::Typed(t) = a { Some((*t.ty).clone()) } else { None })
            .collect();
        if args.len() > 1 {
            self.functions.push((sig.ident.to_string(), args));
        }
    }
}

#[test]
fn no_description_order_swaps_two_values_of_one_type() {
    let mut modules = vec![root().join("examples").join("shop").join("src").join("api")];
    for entry in fs::read_dir(root().join("tests").join("e2e").join("src")).unwrap() {
        let api = entry.unwrap().path().join("api");
        if api.is_dir() {
            modules.push(api);
        }
    }
    let mut swaps = vec![];
    for api in &modules {
        let mut found = Found::default();
        for name in ["types.rs", "operations.rs", "client.rs", "server.rs"] {
            let Ok(text) = fs::read_to_string(api.join(name)) else { continue };
            let file = syn::parse_file(&text).unwrap();
            found.visit_file(&file);
        }
        for (name, args) in found.functions.iter().filter(|(n, _)| !FIXED_ORDER.iter().any(|f| n.starts_with(f))) {
            let mut types: Vec<String> = args.iter().map(|t| expanded(t, &found.aliases)).collect();
            types.sort();
            if types.windows(2).any(|w| w.first() == w.get(1)) {
                swaps.push(format!("{}: fn {name}({})", api.display(), types.join(", ")));
            }
        }
    }
    assert!(swaps.is_empty(), "functions that take two values of one type by position:\n{}", swaps.join("\n"));
}
