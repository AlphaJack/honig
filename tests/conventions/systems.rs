//! The calls of each system are in a file of their own. A `cfg` that picks a system goes only over the declaration
//! that picks such a file, such as `mod wasm;` or `use wasm as imp;`. Every one of those files defines the same
//! functions. So no other code tells the systems apart. A `target_*` key picks a system. So do `unix` and
//! `windows`.

use syn::{
    Attribute, ItemMod, ItemUse, Macro, Meta, Token, UseTree,
    punctuated::Punctuated,
    spanned::Spanned,
    visit::{self, Visit},
};

/// The lines where a `cfg` picks a system except over the declaration of a file per system.
#[derive(Default)]
struct Systems {
    lines: Vec<usize>,
}

impl Systems {
    /// Whether a `cfg` or `cfg_attr` attribute's predicate picks a system.
    fn picks(attr: &Attribute) -> bool {
        let predicate = if attr.path().is_ident("cfg") {
            attr.parse_args::<Meta>().ok()
        } else if attr.path().is_ident("cfg_attr") {
            attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .ok()
                .and_then(|p| p.into_iter().next())
        } else {
            None
        };
        predicate.is_some_and(|p| Self::system(&p))
    }

    /// Whether `predicate` or any predicate within `any`, `all` or `not` picks a system.
    fn system(predicate: &Meta) -> bool {
        match predicate {
            Meta::Path(path) => path.get_ident().is_some_and(|i| i == "unix" || i == "windows"),
            Meta::NameValue(pair) => pair.path.get_ident().is_some_and(|i| i.to_string().starts_with("target_")),
            Meta::List(list) => list
                .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
                .is_ok_and(|inner| inner.iter().any(Self::system)),
        }
    }
}

impl<'ast> Visit<'ast> for Systems {
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        if Self::picks(attr) {
            self.lines.push(attr.span().start().line);
        }
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        if mac.path.is_ident("cfg") && mac.parse_body::<Meta>().is_ok_and(|p| Self::system(&p)) {
            self.lines.push(mac.span().start().line);
        }
    }

    /// `mod wasm;` picks a file.
    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        if item.content.is_some() {
            visit::visit_item_mod(self, item);
        }
    }

    /// `use wasm as imp;` refers to the file picked.
    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        if !matches!(item.tree, UseTree::Rename(_)) {
            visit::visit_item_use(self, item);
        }
    }
}

fn picked(source: &str) -> Vec<usize> {
    let mut systems = Systems::default();
    systems.visit_file(&syn::parse_file(source).unwrap());
    systems.lines
}

#[test]
fn each_systems_calls_are_in_a_file_of_their_own() {
    let mut found: Vec<String> = super::all_sources()
        .into_iter()
        .flat_map(|(file, source)| picked(&source).into_iter().map(move |line| format!("{file}:{line}")))
        .collect();
    found.sort();
    found.dedup();
    assert!(found.is_empty(), "a system's cfg outside the declaration of a file per system:\n{}", found.join("\n"));
}

#[test]
fn only_picking_a_file_may_name_a_system() {
    let source = "#[cfg(windows)]\nmod windows;\n#[cfg(windows)]\nuse windows as imp;\n#[cfg(unix)]\nfn a() {}\n";
    assert_eq!(picked(source), [5]);
}

#[test]
fn a_system_is_found_wherever_a_cfg_names_it() {
    let source = "#[cfg_attr(\n    any(target_os = \"linux\", unix),\n    expect(dead_code)\n)]\nfn a() {\n    if cfg!(all(unix, not(feature = \"x\"))) {}\n}\nenum E {\n    #[cfg(windows)]\n    V,\n}\n#[cfg(all(test, unix))]\nmod tests {}\n";
    assert_eq!(picked(source), [1, 6, 9, 12]);
}

#[test]
fn features_and_tests_pick_no_system() {
    assert_eq!(picked("#[cfg(feature = \"server\")]\nfn a() {}\n#[cfg(test)]\nmod tests {}\n"), [0_usize; 0]);
}
