//! Rust syntax helpers shared by the emitters.

use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::{ToTokens, quote};

use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::Space;
use crate::ir::names::Name;
use crate::ir::types::{Format, Kind, Ty};
use crate::settings::{DateLib, FormatLibs};

/// Returns a Rust string literal with every special character escaped.
pub(crate) fn lit(s: &str) -> Literal {
    Literal::string(s)
}

/// Returns an identifier. Every name reaching here went through `ir::names`.
pub(crate) fn ident(s: &str) -> Ident {
    Ident::new(s, Span::call_site())
}

/// Returns a [`Name`] as a `snake_case` identifier for fields, parameters and functions.
pub(crate) fn snake_ident(n: &Name) -> Ident {
    ident(&snake(n))
}

/// Returns a [`Name`] as a `PascalCase` identifier: types and variants.
pub(crate) fn pascal_ident(n: &Name) -> Ident {
    ident(&pascal(n))
}

/// Returns `///` lines (as `#[doc]` attributes) from rustdoc-safe text.
pub(crate) fn docs(d: &str) -> TokenStream {
    let lines = d.lines().map(|l| {
        // Controls such as `\r` never reach a doc comment.
        let l: String = l.chars().filter(|c| !c.is_control()).collect();
        let text = if l.trim().is_empty() { String::new() } else { format!(" {l}") };
        quote!(#[doc = #text])
    });
    quote!(#(#lines)*)
}

/// Returns inner `//!` docs from rustdoc-safe text.
pub(crate) fn inner_docs(d: &str) -> TokenStream {
    let lines = d.lines().map(|l| {
        let l: String = l.chars().filter(|c| !c.is_control()).collect();
        let text = if l.trim().is_empty() { String::new() } else { format!(" {l}") };
        quote!(#![doc = #text])
    });
    quote!(#(#lines)*)
}

/// Docs from spec prose or none. A doc that restates the name of the item is noise.
pub(crate) fn doc_opt(d: Option<&str>) -> TokenStream {
    d.map(str::trim).filter(|d| !d.is_empty()).map(|d| docs(&rustdoc(d))).unwrap_or_default()
}

/// Returns rust type of `ty`; named types get `types::` when `qualify` is set.
pub(crate) fn ty(space: &Space<'_>, t: &Ty, qualify: bool) -> TokenStream {
    let named = |id: crate::ir::types::TypeId| {
        let n = pascal_ident(&space.types.get(id).name);
        if qualify { quote!(types::#n) } else { quote!(#n) }
    };
    ty_with(space, t, &named)
}

/// Returns [`ty`] with named types written by `named`.
fn ty_with(space: &Space<'_>, t: &Ty, named: &dyn Fn(crate::ir::types::TypeId) -> TokenStream) -> TokenStream {
    let inner = |x: &Ty| ty_with(space, x, named);
    match t {
        Ty::Bool => quote!(bool),
        Ty::I8 => quote!(i8),
        Ty::I16 => quote!(i16),
        Ty::I32 => quote!(i32),
        Ty::I64 => quote!(i64),
        Ty::U8 => quote!(u8),
        Ty::U16 => quote!(u16),
        Ty::U32 => quote!(u32),
        Ty::U64 => quote!(u64),
        Ty::F32 => quote!(f32),
        Ty::F64 => quote!(f64),
        Ty::Decimal => quote!(runtime::Decimal),
        Ty::String => quote!(String),
        Ty::Bytes => quote!(Vec<u8>),
        Ty::Binary => quote!(runtime::Binary),
        Ty::Format(f) => format_ty(space.libs, *f),
        Ty::Any => quote!(serde_json::Value),
        Ty::Unit => quote!(()),
        Ty::Named(id) => named(*id),
        Ty::Vec(x) => {
            let x = inner(x);
            quote!(Vec<#x>)
        }
        Ty::Array(x, n) => {
            let (x, n) = (inner(x), Literal::usize_unsuffixed(*n));
            quote!([#x; #n])
        }
        Ty::Map(x) => {
            let x = inner(x);
            quote!(std::collections::BTreeMap<String, #x>)
        }
        Ty::Option(x) => {
            let x = inner(x);
            quote!(Option<#x>)
        }
        Ty::Indirect(x) => {
            let x = inner(x);
            quote!(Box<#x>)
        }
        Ty::Tuple(xs) => {
            let xs = xs.iter().map(inner);
            quote!((#(#xs,)*))
        }
    }
}

/// Returns rust type of a string format. That is the library of the target crate or `String`.
pub(crate) fn format_ty(libs: FormatLibs, f: Format) -> TokenStream {
    library_ty(libs, f).unwrap_or_else(|| quote!(String))
}

/// Returns a format that the target crate has no library for. It is written as a `String`. Its text is checked at run
/// time instead.
pub(crate) fn untyped(libs: FormatLibs, f: Format) -> bool {
    library_ty(libs, f).is_none()
}

/// Returns the type that a format takes from the standard library, the runtime or the library of the target crate.
/// `None` for its text.
fn library_ty(libs: FormatLibs, f: Format) -> Option<TokenStream> {
    match (f, libs.uuid, libs.dates) {
        (Format::Char, ..) => Some(quote!(char)),
        (Format::Uri, ..) if libs.url => Some(quote!(url::Url)),
        (Format::Ipv4Cidr, ..) if libs.ipnet => Some(quote!(ipnet::Ipv4Net)),
        (Format::Ipv6Cidr, ..) if libs.ipnet => Some(quote!(ipnet::Ipv6Net)),
        (Format::DateTimeLocal, _, Some(DateLib::Chrono)) => Some(quote!(chrono::NaiveDateTime)),
        (Format::DateTimeLocal, _, Some(DateLib::Jiff)) => Some(quote!(jiff::civil::DateTime)),
        // The standard library's addresses (de)serialize as their text form.
        (Format::Ipv4, ..) => Some(quote!(std::net::Ipv4Addr)),
        (Format::Ipv6, ..) => Some(quote!(std::net::Ipv6Addr)),
        (Format::Base64Url, ..) => Some(quote!(runtime::Base64Url)),
        // Types that only a valid value can be. The `idn` feature of the runtime has them. The description turns it on.
        (Format::Email, ..) => Some(quote!(runtime::Email)),
        (Format::IdnEmail, ..) => Some(quote!(runtime::IdnEmail)),
        (Format::Hostname, ..) => Some(quote!(runtime::Hostname)),
        (Format::IdnHostname, ..) => Some(quote!(runtime::IdnHostname)),
        // `time` has an offset: neither library has a type for it. The runtime holds it in `jiff` types.
        (Format::Time, _, Some(DateLib::Jiff)) => Some(quote!(runtime::FullTime)),
        (Format::TimeLocal, _, Some(DateLib::Chrono)) => Some(quote!(chrono::NaiveTime)),
        (Format::TimeLocal, _, Some(DateLib::Jiff)) => Some(quote!(jiff::civil::Time)),
        (Format::Uuid, true, _) => Some(quote!(uuid::Uuid)),
        (Format::DateTime, _, Some(DateLib::Chrono)) => Some(quote!(chrono::DateTime<chrono::Utc>)),
        (Format::Date, _, Some(DateLib::Chrono)) => Some(quote!(chrono::NaiveDate)),
        (Format::DateTime, _, Some(DateLib::Jiff)) => Some(quote!(jiff::Timestamp)),
        (Format::Date, _, Some(DateLib::Jiff)) => Some(quote!(jiff::civil::Date)),
        // Without its library a format is its text. Nothing is lost and nothing is added to the crate.
        (Format::Uuid, false, _)
        | (
            Format::Decimal
            | Format::Decimal128
            | Format::HttpDate
            | Format::Duration
            | Format::UriReference
            | Format::Iri
            | Format::IriReference
            | Format::JsonPointer
            | Format::RelativeJsonPointer
            | Format::Regex
            | Format::UriTemplate
            | Format::Uri
            | Format::Ipv4Cidr
            | Format::Ipv6Cidr,
            ..,
        )
        | (Format::DateTime | Format::Date | Format::Time | Format::DateTimeLocal | Format::TimeLocal, _, None)
        | (Format::Time, _, Some(DateLib::Chrono)) => None,
    }
}

/// Returns the marker in `runtime::validated::format` of a format whose library type reads more than the format allows.
/// `jiff` reads `2024-06-19 08:30Z` as a `date-time` and `uuid` reads `{...}`. `None` for text, the standard library's
/// addresses and the runtime's own types: those read only valid values.
fn validated_format(libs: FormatLibs, f: Format) -> Option<Ident> {
    let marker = match (f, libs.dates) {
        (Format::DateTime, Some(_)) => "DateTime",
        (Format::DateTimeLocal, Some(_)) => "DateTimeLocal",
        (Format::TimeLocal, Some(_)) => "TimeLocal",
        (Format::Date, Some(_)) => "Date",
        (Format::Uuid, _) if libs.uuid => "Uuid",
        (Format::Uri, _) if libs.url => "Uri",
        (Format::Ipv4Cidr, _) if libs.ipnet => "Ipv4Cidr",
        (Format::Ipv6Cidr, _) if libs.ipnet => "Ipv6Cidr",
        _ => return None,
    };
    Some(ident(marker))
}

/// Returns how `t` holds the text of its checked formats: a `runtime::validated::Holds` that reads it with each text
/// checked before its library reads it. `None` when `t` holds none. Aliases are followed. A named type checks its own.
pub(crate) fn holds(space: &Space<'_>, t: &Ty) -> Option<TokenStream> {
    let c = quote!(runtime::validated);
    match space.types.unalias(t) {
        Ty::Format(f) => validated_format(space.libs, *f).map(|m| quote!(#c::Text<#c::format::#m>)),
        Ty::Vec(x) => holds(space, x).map(|h| quote!(#c::Many<#h>)),
        Ty::Array(x, n) => {
            let n = Literal::usize_unsuffixed(*n);
            holds(space, x).map(|h| quote!(#c::Exactly<#h, #n>))
        }
        Ty::Map(x) => holds(space, x).map(|h| quote!(#c::Values<#h>)),
        Ty::Option(x) => holds(space, x).map(|h| quote!(#c::Maybe<#h>)),
        Ty::Indirect(x) => holds(space, x).map(|h| quote!(#c::Boxed<#h>)),
        Ty::Tuple(xs) => {
            let each: Vec<Option<TokenStream>> = xs.iter().map(|x| holds(space, x)).collect();
            each.iter().any(Option::is_some).then(|| {
                let each = each.into_iter().map(|h| h.unwrap_or_else(|| quote!(#c::Plain)));
                quote!((#(#each,)*))
            })
        }
        Ty::Bool
        | Ty::I8
        | Ty::I16
        | Ty::I32
        | Ty::I64
        | Ty::U8
        | Ty::U16
        | Ty::U32
        | Ty::U64
        | Ty::F32
        | Ty::F64
        | Ty::Decimal
        | Ty::String
        | Ty::Bytes
        | Ty::Binary
        | Ty::Any
        | Ty::Unit
        | Ty::Named(_) => None,
    }
}

/// Does `t` mention a named type (needs `types`) or `runtime::Binary` (needs `runtime`)?
pub(crate) fn mentions(t: &Ty) -> (bool, bool) {
    let runtime = matches!(
        t,
        Ty::Binary
            | Ty::Decimal
            | Ty::Format(Format::Base64Url | Format::Email | Format::IdnEmail | Format::Hostname | Format::IdnHostname)
    );
    t.children().iter().map(mentions).fold((matches!(t, Ty::Named(_)), runtime), |a, b| (a.0 || b.0, a.1 || b.1))
}

/// Returns a file from its items printed by `prettyplease`. An error when the tokens do not form a file. That is a bug
/// in the generator. `binary`: the crate has no library. So every `pub` becomes `pub(crate)` (`unreachable_pub` of
/// rustc).
pub(crate) fn print(file: &str, tokens: TokenStream, binary: bool) -> Result<String, crate::error::Error> {
    use syn::visit_mut::VisitMut;
    let mut file: syn::File = syn::parse2(group_digits(tokens))
        .map_err(|source| crate::error::Error::Unparsable { file: file.to_owned(), source })?;
    share_identical_impls(&mut file);
    SelfInOwnFields.visit_file_mut(&mut file);
    if binary {
        CrateOnly.visit_file_mut(&mut file);
    }
    Ok(unparse(file))
}

/// The prefix of a metavariable of a macro body while it is printed as plain items: `$n` is `__honig_n`.
const METAVAR: &str = "__honig_";

/// Returns `file` printed by `prettyplease`, with the body of each top-level `macro_rules!` printed as the items it
/// expands to. `prettyplease` prints a macro body as raw tokens, one space between each: a body of trait impls would
/// read as one long line. A body that is not items stays as raw tokens.
pub(crate) fn unparse(mut file: syn::File) -> String {
    let mut bodies = Vec::new();
    for item in &mut file.items {
        let syn::Item::Macro(m) = item else { continue };
        let Some(text) = macro_rules_text(m) else { continue };
        let marker = quote::format_ident!("__HONIG_MACRO_{}", bodies.len());
        let attrs = &m.attrs;
        let Ok(placeholder) = syn::parse2::<syn::Item>(quote!(#(#attrs)* const #marker: () = ();)) else { continue };
        *item = placeholder;
        bodies.push((format!("const {marker}: () = ();"), text));
    }
    let mut out = prettyplease::unparse(&file);
    for (marker, text) in bodies {
        out = out.replacen(&marker, text.trim_end(), 1);
    }
    out
}

/// Returns a macro body with each metavariable `$x` as the identifier `__honig_x`. So it parses as items.
fn hide_metavars(tokens: TokenStream) -> TokenStream {
    use proc_macro2::TokenTree;
    let mut out = Vec::new();
    let mut it = tokens.into_iter().peekable();
    while let Some(t) = it.next() {
        match t {
            TokenTree::Punct(p) if p.as_char() == '$' && matches!(it.peek(), Some(TokenTree::Ident(_))) => {
                if let Some(TokenTree::Ident(i)) = it.next() {
                    out.push(TokenTree::Ident(quote::format_ident!("{METAVAR}{i}")));
                }
            }
            TokenTree::Group(g) => {
                let mut inner = proc_macro2::Group::new(g.delimiter(), hide_metavars(g.stream()));
                inner.set_span(g.span());
                out.push(TokenTree::Group(inner));
            }
            other @ (TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_)) => out.push(other),
        }
    }
    out.into_iter().collect()
}

/// Returns `macro_rules! name { (matcher) => { body }; }` with its body printed as items. `None` for any other macro
/// or for a body that does not parse as items.
fn macro_rules_text(m: &syn::ItemMacro) -> Option<String> {
    use proc_macro2::TokenTree;
    if !m.mac.path.is_ident("macro_rules") {
        return None;
    }
    let name = m.ident.as_ref()?;
    let tokens: Vec<TokenTree> = m.mac.tokens.clone().into_iter().collect();
    // One rule: a matcher, `=>`, a braced body and an optional `;`.
    let [TokenTree::Group(matcher), TokenTree::Punct(eq), TokenTree::Punct(gt), TokenTree::Group(body), rest @ ..] =
        tokens.as_slice()
    else {
        return None;
    };
    let semi = |t: &TokenTree| matches!(t, TokenTree::Punct(p) if p.as_char() == ';');
    if eq.as_char() != '=' || gt.as_char() != '>' || !rest.iter().all(semi) || rest.len() > 1 {
        return None;
    }
    let items: syn::File = syn::parse2(hide_metavars(body.stream())).ok()?;
    let printed = prettyplease::unparse(&items).replace(METAVAR, "$");
    let body: String =
        printed.lines().map(|l| if l.is_empty() { "\n".to_owned() } else { format!("        {l}\n") }).collect();
    let matcher = matcher.stream().to_string().replace("$ ", "$").replace(" : ", ":").replace(" , ", ", ");
    Some(format!("macro_rules! {name} {{\n    ({matcher}) => {{\n{body}    }};\n}}\n"))
}

/// Writes once a trait impl that several types share word for word. Examples are the `Validate` of the JSON,
/// form and multipart shapes of a payload and the `Serialize` of two unions with the same variants. A
/// `macro_rules!` at the top of the file writes it for any type. Each type invokes it. Generated code holds no two
/// identical function bodies.
fn share_identical_impls(file: &mut syn::File) {
    use syn::visit_mut::VisitMut;
    let mut count = CountImpls::default();
    count.visit_file_mut(file);
    let mut share = ShareImpls { count: count.0, shared: Vec::new(), cfg: Vec::new() };
    share.visit_file_mut(file);
    let macros: Vec<syn::Item> = share
        .shared
        .into_iter()
        .map(|shared| {
            let name = quote::format_ident!("{}", shared.name);
            let body = for_type(shared.shape);
            // Under the `cfg` of every use (`#[cfg(test)]` for samples). So a build without them has no unused macro.
            let cfg = shared.cfg.unwrap_or_default();
            let mut attrs = Vec::new();
            attrs.extend(cfg.into_iter().map(|tokens| {
                let path = syn::Path::from(quote::format_ident!("cfg"));
                let delimiter = syn::MacroDelimiter::Paren(syn::token::Paren::default());
                attribute(syn::Meta::List(syn::MetaList { path, delimiter, tokens }))
            }));
            syn::Item::Macro(syn::ItemMacro {
                attrs,
                ident: Some(name),
                mac: macro_call(
                    "macro_rules",
                    syn::MacroDelimiter::Brace(syn::token::Brace::default()),
                    quote!(($t:ty) => { #body };),
                ),
                semi_token: None,
            })
        })
        .collect();
    file.items.splice(0..0, macros);
}

/// The placeholder for the implementing type in a shared impl.
const SELF_TYPE: &str = "__HonigSelfType";

/// Returns a trait impl with its type replaced by [`SELF_TYPE`] and the type. [`share_identical_impls`] compares these.
fn impl_shape(item: &mut syn::Item) -> Option<(TokenStream, TokenStream)> {
    let syn::Item::Impl(i) = item else { return None };
    i.trait_.as_ref()?;
    let placeholder = Ident::new(SELF_TYPE, Span::call_site());
    let stand_in =
        syn::Type::Path(syn::TypePath { attrs: Vec::new(), qself: None, path: syn::Path::from(placeholder) });
    let ty = std::mem::replace(&mut *i.self_ty, stand_in);
    let shape = i.to_token_stream();
    *i.self_ty = ty;
    Some((shape, i.self_ty.to_token_stream()))
}

/// Returns an outer attribute `#[meta]`.
fn attribute(meta: syn::Meta) -> syn::Attribute {
    syn::Attribute {
        pound_token: syn::token::Pound::default(),
        style: syn::AttrStyle::Outer,
        bracket_token: syn::token::Bracket::default(),
        meta,
    }
}

/// Returns `name!(tokens)` with `delimiter`.
fn macro_call(name: &str, delimiter: syn::MacroDelimiter, tokens: TokenStream) -> syn::Macro {
    syn::Macro {
        path: syn::Path::from(Ident::new(name, Span::call_site())),
        bang_token: syn::token::Not::default(),
        delimiter,
        tokens,
    }
}

/// Every `pub` becomes `pub(crate)`.
struct CrateOnly;

impl syn::visit_mut::VisitMut for CrateOnly {
    fn visit_visibility_mut(&mut self, vis: &mut syn::Visibility) {
        if matches!(vis, syn::Visibility::Public(_)) {
            *vis = syn::Visibility::Restricted(syn::VisRestricted {
                pub_token: syn::token::Pub::default(),
                paren_token: syn::token::Paren::default(),
                in_token: None,
                path: Box::new(syn::Path::from(Ident::new("crate", Span::call_site()))),
            });
        }
    }
}

/// Groups the digits of number literals in threes (`clippy::unreadable_literal`). This includes macro arguments
/// because syn leaves those as tokens.
fn group_digits(tokens: TokenStream) -> TokenStream {
    use proc_macro2::{Group, TokenTree};
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut ng = Group::new(g.delimiter(), group_digits(g.stream()));
                ng.set_span(g.span());
                TokenTree::Group(ng)
            }
            TokenTree::Literal(l) => TokenTree::Literal(match syn::Lit::new(l.clone()) {
                syn::Lit::Int(n) => {
                    syn::LitInt::new(&format!("{}{}", grouped(n.base10_digits()), n.suffix()), l.span()).token()
                }
                syn::Lit::Float(n) => {
                    syn::LitFloat::new(&format!("{}{}", grouped(n.base10_digits()), n.suffix()), l.span()).token()
                }
                // `Lit` is non-exhaustive.
                syn::Lit::Str(_)
                | syn::Lit::ByteStr(_)
                | syn::Lit::CStr(_)
                | syn::Lit::Byte(_)
                | syn::Lit::Char(_)
                | syn::Lit::Bool(_)
                | syn::Lit::Verbatim(_)
                | _ => l,
            }),
            other @ (TokenTree::Ident(_) | TokenTree::Punct(_)) => other,
        })
        .collect()
}

/// Parts with fewer digits stay as they are.
const GROUPED_FROM: usize = 5;
/// Digits per group: thousands.
const GROUP: usize = 3;

/// Returns the digits of a decimal number in groups of three. The integer part goes from the point leftwards. The
/// fraction goes from the point rightwards. The sign and the exponent stay as they are.
fn grouped(number: &str) -> String {
    let (sign, unsigned) = number.split_at(usize::from(number.starts_with('-')));
    let (mantissa, exponent) = unsigned.split_at(unsigned.find(['e', 'E']).unwrap_or(unsigned.len()));
    let (int, fraction) = mantissa.split_once('.').map_or((mantissa, None), |(i, f)| (i, Some(f)));
    let fraction = fraction.map(|f| format!(".{}", in_threes(f, 0))).unwrap_or_default();
    format!("{sign}{}{fraction}{exponent}", in_threes(int, int.len() % GROUP))
}

/// Returns `digits` as a first group of `head` digits (none when 0) and then groups of three joined by `_`.
fn in_threes(digits: &str, head: usize) -> String {
    if digits.len() < GROUPED_FROM {
        return digits.to_owned();
    }
    let starts = std::iter::once(0).filter(|_| head > 0).chain((head..digits.len()).step_by(GROUP));
    let ends = starts.clone().skip(1).chain(std::iter::once(digits.len()));
    starts.zip(ends).map(|(a, b)| digits.get(a..b).unwrap_or_default()).collect::<Vec<_>>().join("_")
}

/// A struct or enum that holds itself names itself `Self` in its fields (`clippy::use_self`).
struct SelfInOwnFields;

impl syn::visit_mut::VisitMut for SelfInOwnFields {
    fn visit_item_struct_mut(&mut self, s: &mut syn::ItemStruct) {
        if s.generics.params.is_empty() {
            syn::visit_mut::VisitMut::visit_fields_mut(&mut OwnName(&s.ident), &mut s.fields);
        }
    }

    fn visit_item_enum_mut(&mut self, e: &mut syn::ItemEnum) {
        if e.generics.params.is_empty() {
            for v in &mut e.variants {
                syn::visit_mut::VisitMut::visit_fields_mut(&mut OwnName(&e.ident), &mut v.fields);
            }
        }
    }
}

/// Turns the type `.0` written without arguments into `Self`.
struct OwnName<'a>(&'a Ident);

impl syn::visit_mut::VisitMut for OwnName<'_> {
    fn visit_type_path_mut(&mut self, t: &mut syn::TypePath) {
        if t.qself.is_none()
            && t.path.leading_colon.is_none()
            && t.path.segments.len() == 1
            && let Some(seg) = t.path.segments.first_mut()
            && seg.ident == *self.0
            && seg.arguments.is_none()
        {
            seg.ident = Ident::new("Self", seg.ident.span());
            return;
        }
        syn::visit_mut::visit_type_path_mut(self, t);
    }
}

/// Replaces [`SELF_TYPE`] with the macro's `$t`.
fn for_type(tokens: TokenStream) -> TokenStream {
    use proc_macro2::{Group, Punct, Spacing, TokenTree};
    tokens
        .into_iter()
        .flat_map(|tt| -> Vec<TokenTree> {
            match tt {
                TokenTree::Group(g) => {
                    let mut ng = Group::new(g.delimiter(), for_type(g.stream()));
                    ng.set_span(g.span());
                    vec![TokenTree::Group(ng)]
                }
                TokenTree::Ident(i) if i == SELF_TYPE => {
                    vec![TokenTree::Punct(Punct::new('$', Spacing::Alone)), TokenTree::Ident(Ident::new("t", i.span()))]
                }
                other @ (TokenTree::Ident(_) | TokenTree::Punct(_) | TokenTree::Literal(_)) => vec![other],
            }
        })
        .collect()
}

/// How often each trait impl's shape occurs.
#[derive(Default)]
struct CountImpls(std::collections::BTreeMap<String, usize>);

impl syn::visit_mut::VisitMut for CountImpls {
    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        if let Some((shape, _)) = impl_shape(item) {
            *self.0.entry(shape.to_string()).or_default() += 1;
        }
        syn::visit_mut::visit_item_mut(self, item);
    }
}

/// One shared impl with its shape and the `cfg` attributes that every use sits under (`None` once two uses differ).
struct Shared {
    text: String,
    /// The macro: the trait in `snake_case` and `_impl`, then a number from the second one of a trait on.
    name: String,
    /// The trait as written.
    trait_name: String,
    shape: TokenStream,
    cfg: Option<Vec<TokenStream>>,
}

/// Replaces each impl whose shape occurs more than once by an invocation of its shared macro.
struct ShareImpls {
    count: std::collections::BTreeMap<String, usize>,
    /// Each shared macro in order.
    shared: Vec<Shared>,
    /// The `cfg` attributes of the modules being visited.
    cfg: Vec<TokenStream>,
}

impl syn::visit_mut::VisitMut for ShareImpls {
    fn visit_item_mod_mut(&mut self, m: &mut syn::ItemMod) {
        let depth = self.cfg.len();
        // The condition of each `#[cfg(...)]` to put back on the shared macro.
        self.cfg.extend(m.attrs.iter().filter_map(|a| match &a.meta {
            syn::Meta::List(l) if l.path.is_ident("cfg") => Some(l.tokens.clone()),
            syn::Meta::List(_) | syn::Meta::Path(_) | syn::Meta::NameValue(_) => None,
        }));
        syn::visit_mut::visit_item_mod_mut(self, m);
        self.cfg.truncate(depth);
    }

    fn visit_item_mut(&mut self, item: &mut syn::Item) {
        syn::visit_mut::visit_item_mut(self, item);
        let trait_name = if let syn::Item::Impl(i) = &*item {
            i.trait_.as_ref().and_then(|(path, _)| path.segments.last()).map(|s| s.ident.to_string())
        } else {
            None
        };
        let Some((shape, ty)) = impl_shape(item) else { return };
        let trait_name = trait_name.unwrap_or_default();
        let text = shape.to_string();
        if self.count.get(&text).copied().unwrap_or(0) < 2 {
            return;
        }
        let cfg_text = |c: &[TokenStream]| c.iter().map(ToString::to_string).collect::<Vec<_>>();
        let here = cfg_text(&self.cfg);
        let n = match self.shared.iter().position(|sh| sh.text == text) {
            Some(n) => {
                let same = self.shared[n].cfg.as_deref().is_some_and(|c| cfg_text(c) == here);
                if !same {
                    self.shared[n].cfg = None;
                }
                n
            }
            None => {
                let base = format!("{}_impl", snake(&Name::new(&trait_name)));
                let same_trait = self.shared.iter().filter(|sh| sh.trait_name == trait_name).count();
                let name = if same_trait == 0 { base } else { format!("{base}_{}", same_trait + 1) };
                self.shared.push(Shared { text, name, trait_name, shape, cfg: Some(self.cfg.clone()) });
                self.shared.len() - 1
            }
        };
        let name = quote::format_ident!("{}", self.shared[n].name);
        *item = syn::Item::Macro(syn::ItemMacro {
            attrs: Vec::new(),
            ident: None,
            mac: macro_call(&name.to_string(), syn::MacroDelimiter::Paren(syn::token::Paren::default()), ty),
            semi_token: Some(syn::token::Semi::default()),
        });
    }
}

/// Words clippy's `doc_markdown` accepts without backticks (its default list).
const VALID_IDENTS: &[&str] = &[
    "KiB",
    "MiB",
    "GiB",
    "TiB",
    "PiB",
    "EiB",
    "DirectX",
    "ECMAScript",
    "GPLv2",
    "GPLv3",
    "GitHub",
    "GitLab",
    "IPv4",
    "IPv6",
    "JavaScript",
    "PostScript",
    "TypeScript",
    "WebAssembly",
    "NaN",
    "NaNs",
    "OAuth",
    "GraphQL",
    "OCaml",
    "OpenGL",
    "OpenSSH",
    "OpenSSL",
    "OpenStreetMap",
    "OpenTelemetry",
    "OpenType",
    "WebGL",
    "WebGPU",
    "WebRTC",
    "WebSocket",
    "WebTransport",
    "TensorFlow",
    "TrueType",
    "iOS",
    "macOS",
    "FreeBSD",
    "NetBSD",
    "OpenBSD",
    "TeX",
    "LaTeX",
    "BibTeX",
    "MinGW",
];

/// Turns spec prose (`CommonMark`) into rustdoc that passes `clippy::doc_markdown` and
/// the rustdoc lints. Identifiers go in backticks and bare URLs in `<...>`. `[` and
/// `<` are escaped. Code blocks are marked `text` so that they never run as doctests.
/// Lazy list continuations are indented.
pub(crate) fn rustdoc(text: &str) -> String {
    let mut out: Vec<String> = vec![];
    let mut fence: Option<String> = None;
    let mut indented_code = false;
    let mut prev_blank = true;
    let mut list_indent: Option<usize> = None;
    for raw in text.trim().lines() {
        let line = raw.trim_end().replace('\t', "    ");
        let t = line.trim_start();
        let indent = line.len() - t.len();
        if let Some(marker) = &fence {
            if t.starts_with(marker.as_str()) {
                out.push(marker.clone());
                fence = None;
            } else {
                out.push(line.clone());
            }
            continue;
        }
        if indented_code {
            if t.is_empty() || indent >= CODE_INDENT {
                out.push(line.get(CODE_INDENT..).unwrap_or("").to_owned());
                continue;
            }
            while out.last().is_some_and(String::is_empty) {
                out.pop();
            }
            out.push("```".to_owned());
            out.push(String::new());
            indented_code = false;
        }
        if let Some(marker) = fence_marker(t) {
            let lang = t.get(marker.len()..).unwrap_or("").trim();
            let rusty = lang.is_empty()
                || lang.split([',', ' ']).any(|w| {
                    matches!(w.trim(), "rust" | "ignore" | "no_run" | "should_panic" | "compile_fail")
                        || w.trim().starts_with("edition")
                });
            out.push(format!("{marker}{}", if rusty { "text" } else { lang }));
            fence = Some(marker);
            prev_blank = false;
            continue;
        }
        if t.is_empty() {
            out.push(String::new());
            prev_blank = true;
            list_indent = None;
            continue;
        }
        if indent >= CODE_INDENT && prev_blank && list_indent.is_none() {
            out.push("```text".to_owned());
            out.push(line.get(CODE_INDENT..).unwrap_or("").to_owned());
            indented_code = true;
            continue;
        }
        let text_line = if let Some(marker_len) = list_marker(t) {
            list_indent = Some(indent + marker_len);
            format!("{}{}", " ".repeat(indent), inline(t))
        } else if let Some(ci) = list_indent.filter(|_| !prev_blank) {
            // A lazy continuation of a list item: indent it under the item.
            format!("{}{}", " ".repeat(ci), inline(t))
        } else {
            format!("{}{}", " ".repeat(indent.min(CODE_INDENT - 1)), inline(t))
        };
        out.push(text_line);
        prev_blank = false;
    }
    if fence.is_some() || indented_code {
        out.push(fence.unwrap_or_else(|| "```".to_owned()));
    }
    out.join("\n")
}

/// More arguments than this and clippy warns (`too_many_arguments`): a generated function says why it takes them.
pub(crate) const MAX_ARGUMENTS: usize = 7;

/// `CommonMark`: four spaces of indent make a code block. A fence is three or more backticks or tildes. The number
/// of an ordered list has nine digits at most.
const CODE_INDENT: usize = 4;
const FENCE: usize = 3;
const MAX_LIST_DIGITS: usize = 9;

fn fence_marker(t: &str) -> Option<String> {
    let first = t.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let n = t.chars().take_while(|c| *c == first).count();
    (n >= FENCE).then(|| first.to_string().repeat(n))
}

/// Returns the length of a list marker with its trailing space (`- ` or `12. `).
fn list_marker(t: &str) -> Option<usize> {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return Some("- ".len());
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    let rest = t.get(digits..)?;
    (digits > 0 && digits <= MAX_LIST_DIGITS && (rest.starts_with(". ") || rest.starts_with(") ")))
        .then_some(digits + ". ".len())
}

/// Returns one line of prose. Code spans are kept. Everything else is made lint-clean.
fn inline(line: &str) -> String {
    let line = if line.matches('`').count() % 2 == 1 { line.replace('`', "'") } else { line.to_owned() };
    let mut out = String::new();
    for (i, seg) in line.split('`').enumerate() {
        if i % 2 == 1 {
            out.push('`');
            out.push_str(seg);
            out.push('`');
        } else {
            out.push_str(&prose(seg));
        }
    }
    out
}

fn prose(seg: &str) -> String {
    let has_links = seg.contains("](");
    let mut out = String::new();
    for (i, token) in seg.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&word(token, has_links));
    }
    out
}

fn word(token: &str, has_links: bool) -> String {
    // URLs: bare ones become autolinks. Markdown link targets stay as they are.
    if let Some(start) = token.find("http://").or_else(|| token.find("https://")) {
        let (head, url) = token.split_at(start);
        if head.ends_with("](") || head.ends_with('<') {
            return token.to_owned();
        }
        let end = url.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'', '*', '>']).len();
        let (url, tail) = url.split_at(end);
        return format!("{}<{url}>{}", escape(head, has_links), escape(tail, has_links));
    }
    let core_start = token.find(|c: char| c.is_alphanumeric() || c == ':' || c == '<').unwrap_or(token.len());
    let core_end = token
        .rfind(|c: char| c.is_alphanumeric() || c == ':' || c == '>')
        .map_or(core_start, |i| i + 1)
        .max(core_start);
    let head = token.get(..core_start).unwrap_or("");
    let core = token.get(core_start..core_end).unwrap_or("").trim_end_matches(':');
    let tail = token.get(core_start + core.len()..).unwrap_or("");
    if looks_code(core) {
        format!("{}`{core}`{}", escape(head, has_links), escape(tail, has_links))
    } else {
        escape(token, has_links)
    }
}

fn escape(s: &str, has_links: bool) -> String {
    let s = s.replace('<', "&lt;");
    if has_links { s } else { s.replace('[', "\\[").replace(']', "\\]") }
}

/// Returns whether clippy's `doc_markdown` wants this word in backticks.
fn looks_code(w: &str) -> bool {
    if w.is_empty() || VALID_IDENTS.contains(&w) {
        return false;
    }
    if w.contains("::") || w.contains('<') || w.contains('>') {
        return true;
    }
    w.split(['-', '/', '.', '\'']).any(|p| {
        let p = p.trim_matches('_');
        if VALID_IDENTS.contains(&p) {
            return false;
        }
        let ident = p.chars().all(|c| c.is_alphanumeric() || c == '_');
        let underscore = ident && p.contains('_');
        let s = p.strip_suffix('s').unwrap_or(p);
        let camel = s.starts_with(|c: char| c.is_uppercase())
            && s.chars().all(char::is_alphanumeric)
            && s.chars().filter(|c| c.is_uppercase()).count() > 1
            && s.chars().any(char::is_lowercase);
        underscore || camel
    })
}

/// Does the type contain a float (and so cannot derive `Eq`)?
pub(crate) fn has_float(space: &Space<'_>, ty: &Ty) -> bool {
    space.has_float(ty)
}

/// Sizes on a 64-bit target written out so that the generated code is the same on every host. That is a pointer or
/// a word, a `String`, `Vec` or `BTreeMap` (three words) and a `serde_json::Value`.
pub(crate) const WORD: usize = 8;
const COLLECTION: usize = 24;
const JSON_VALUE: usize = 32;
/// Types nested deeper than this count as a pointer.
const MAX_SIZE_DEPTH: usize = 16;
/// How much larger than the next the largest enum variant may be before it is boxed (`clippy::large_enum_variant`
/// warns past 200).
const BOX_MARGIN: usize = 128;

/// Returns the rough in-memory size in bytes on a 64-bit target. It decides which enum
/// variants to box (`clippy::large_enum_variant`).
pub(crate) fn approx_size(space: &Space<'_>, ty: &Ty) -> usize {
    size_in(space, ty, 0)
}

fn size_in(space: &Space<'_>, ty: &Ty, depth: usize) -> usize {
    if depth > MAX_SIZE_DEPTH {
        return WORD;
    }
    match ty {
        Ty::Unit => 0,
        Ty::Bool | Ty::I8 | Ty::U8 => size_of::<u8>(),
        Ty::I16 | Ty::U16 => size_of::<u16>(),
        Ty::I32 | Ty::U32 | Ty::F32 => size_of::<u32>(),
        Ty::I64 | Ty::U64 | Ty::F64 => size_of::<u64>(),
        Ty::Indirect(_) => WORD,
        Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Vec(_) | Ty::Map(_) | Ty::Decimal => COLLECTION,
        Ty::Any => JSON_VALUE,
        Ty::Option(t) => option_size(space, t, depth),
        Ty::Tuple(ts) => ts.iter().map(|t| size_in(space, t, depth + 1)).sum(),
        Ty::Array(t, n) => n * size_in(space, t, depth + 1),
        Ty::Named(id) => match &space.types.get(*id).kind {
            Kind::Struct(s) => {
                s.fields
                    .iter()
                    .map(|f| {
                        if f.required { size_in(space, &f.ty, depth + 1) } else { option_size(space, &f.ty, depth + 1) }
                    })
                    .sum::<usize>()
                    + if s.additional.is_some() { COLLECTION } else { 0 }
            }
            Kind::StrEnum(_) | Kind::IntEnum(_) => COLLECTION,
            Kind::Checked(c) => size_in(space, &c.base, depth + 1),
            Kind::Const(_) => 0,
            Kind::Union(u) => {
                u.variants.iter().map(|v| size_in(space, &v.ty, depth + 1)).max().unwrap_or(0).max(JSON_VALUE) + WORD
            }
            Kind::Alias(t) | Kind::Newtype(t) => size_in(space, t, depth + 1),
            Kind::Pending => WORD,
        },
    }
}

fn option_size(space: &Space<'_>, t: &Ty, depth: usize) -> usize {
    let inner = size_in(space, t, depth);
    if matches!(
        t,
        Ty::String | Ty::Bytes | Ty::Binary | Ty::Format(_) | Ty::Vec(_) | Ty::Map(_) | Ty::Indirect(_) | Ty::Named(_)
    ) {
        inner
    } else {
        inner + WORD
    }
}

/// Returns which variants to box so that the largest variant is no more than about
/// 128 bytes above the next one. `sizes` holds (unboxed size, boxable part size).
pub(crate) fn box_plan(sizes: &[(usize, Option<usize>)]) -> Vec<bool> {
    let mut boxed = vec![false; sizes.len()];
    if sizes.len() < 2 {
        return boxed;
    }
    loop {
        let eff: Vec<usize> =
            sizes.iter().zip(&boxed).map(|((s, part), b)| if *b { s - part.unwrap_or(0) + WORD } else { *s }).collect();
        let Some((max_i, max)) = eff.iter().copied().enumerate().max_by_key(|(_, s)| *s) else { return boxed };
        let second = eff.iter().enumerate().filter(|(i, _)| *i != max_i).map(|(_, s)| *s).max().unwrap_or(0);
        if max < second + BOX_MARGIN || boxed[max_i] || sizes[max_i].1.is_none_or(|p| p <= WORD) {
            return boxed;
        }
        boxed[max_i] = true;
    }
}

/// Returns a JSON value as an expression building it (no parsing at runtime).
pub(crate) fn json_tokens(v: &serde_json::Value) -> TokenStream {
    match v {
        serde_json::Value::Null => quote!(serde_json::Value::Null),
        serde_json::Value::Bool(b) => quote!(serde_json::Value::Bool(#b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let l = Literal::i64_suffixed(i);
                quote!(serde_json::Value::from(#l))
            } else if let Some(u) = n.as_u64() {
                let l = Literal::u64_suffixed(u);
                quote!(serde_json::Value::from(#l))
            } else {
                let l = Literal::f64_suffixed(n.as_f64().unwrap_or_default());
                quote!(serde_json::Value::from(#l))
            }
        }
        serde_json::Value::String(t) => {
            let t = lit(t);
            quote!(serde_json::Value::String(#t.to_owned()))
        }
        serde_json::Value::Array(xs) => {
            let xs = xs.iter().map(json_tokens);
            quote!(serde_json::Value::Array(vec![#(#xs),*]))
        }
        serde_json::Value::Object(m) if m.is_empty() => quote!(serde_json::Value::Object(serde_json::Map::new())),
        serde_json::Value::Object(m) => {
            let kv = m.iter().map(|(k, x)| {
                let (k, x) = (lit(k), json_tokens(x));
                quote!((#k.to_owned(), #x))
            });
            quote!(serde_json::Value::Object(serde_json::Map::from_iter([#(#kv),*])))
        }
    }
}

/// Returns the inner lint attributes that every generated file starts with. `deprecated` only when the description
/// deprecates something. Such items warn where you use them and not inside generated code. `too_many_lines` is the
/// one pedantic lint that the description decides. It depends on how many fields and operations a function walks.
/// `missing_errors_doc` because no generated doc is the generator's own.
pub(crate) fn file_allow(deprecated: bool) -> TokenStream {
    let deprecated = deprecated.then(|| quote!(deprecated,));
    quote! {
        #![allow(
            #deprecated
            missing_docs,
            reason = "generated from the OpenAPI description, which may leave items undescribed or deprecate them"
        )]
        #![allow(clippy::too_many_lines, reason = "as long as the description's schemas and operations")]
        #![allow(clippy::missing_errors_doc, reason = "generated docs hold only the description's own prose")]
    }
}

/// Does the description deprecate a schema, property, operation, parameter,
/// response header or security scheme?
pub(crate) fn has_deprecated(api: &crate::ir::Api) -> bool {
    use crate::ir::types::Kind;
    api.types
        .defs
        .iter()
        .any(|d| d.deprecated || matches!(&d.kind, Kind::Struct(s) if s.fields.iter().any(|f| f.deprecated)))
        || api.schemes.iter().any(|s| s.deprecated)
        || api.http.operations.iter().any(|o| {
            o.deprecated
                || o.params.iter().any(|p| p.deprecated)
                || o.responses.iter().any(|r| r.headers.iter().any(|h| h.deprecated))
        })
}

/// Returns one `match` arm with its patterns joined with `|` and then its body.
pub(crate) fn match_arm(patterns: &[TokenStream], body: &TokenStream) -> TokenStream {
    quote!(#(#patterns)|* => #body,)
}

/// Returns `match` arms from `(pattern, type, body)`: variants of one type (`Some`) write their body once,
/// `Self::A(v) | Self::B(v) => ...` (clippy `match_same_arms`). `None` shares with no other arm.
pub(crate) fn shared_arms(arms: Vec<(TokenStream, Option<Ty>, TokenStream)>) -> Vec<TokenStream> {
    let mut groups: Vec<(Option<Ty>, Vec<TokenStream>, TokenStream)> = vec![];
    for (pattern, same, body) in arms {
        // One type and the same checks: two variants of one type with different constraints keep their own arms.
        let found =
            groups.iter_mut().find(|(s, _, b)| same.is_some() && *s == same && b.to_string() == body.to_string());
        match found {
            Some((_, patterns, _)) => patterns.push(pattern),
            None => groups.push((same, vec![pattern], body)),
        }
    }
    groups.into_iter().map(|(_, patterns, body)| match_arm(&patterns, &body)).collect()
}

#[cfg(test)]
mod tests {
    use super::{grouped, lit, rustdoc};

    #[test]
    fn literals_escape_everything() {
        assert_eq!(lit("a\"b\\c\n*/").to_string(), r#""a\"b\\c\n*/""#);
    }

    #[test]
    fn long_numbers_are_grouped_in_threes_from_the_point() {
        assert_eq!(grouped("1234"), "1234");
        assert_eq!(grouped("12345"), "12_345");
        assert_eq!(grouped("9007199254740991"), "9_007_199_254_740_991");
        assert_eq!(grouped("-1048576"), "-1_048_576");
        assert_eq!(grouped("123456.1234567"), "123_456.123_456_7");
        assert_eq!(grouped("1.5e10"), "1.5e10");
    }

    #[test]
    fn prose_becomes_lint_clean_rustdoc() {
        assert_eq!(rustdoc("Uses OpenAPI and user_id."), "Uses `OpenAPI` and `user_id`.");
        assert_eq!(rustdoc("See https://example.com/x."), "See <https://example.com/x>.");
        assert_eq!(rustdoc("RFC (https://a.b/c)"), "RFC (<https://a.b/c>)");
        assert_eq!(rustdoc("[docs](https://a.b) and `x_y`"), "[docs](https://a.b) and `x_y`");
        assert_eq!(rustdoc("a [b] c<d"), "a \\[b\\] `c<d`");
        assert_eq!(rustdoc("a < b"), "a &lt; b");
        assert_eq!(rustdoc("List<String> here"), "`List<String>` here");
        assert_eq!(rustdoc("```\nlet x = 1;\n```"), "```text\nlet x = 1;\n```");
        assert_eq!(rustdoc("```json\n{}\n```"), "```json\n{}\n```");
        assert_eq!(rustdoc("text\n\n    code\n\nmore"), "text\n\n```text\ncode\n```\n\nmore");
        assert_eq!(rustdoc("- item\ncontinued"), "- item\n  continued");
        assert_eq!(rustdoc("GitHub is fine"), "GitHub is fine");
        assert_eq!(rustdoc("CamelCase's API"), "`CamelCase's` API");
    }
}
