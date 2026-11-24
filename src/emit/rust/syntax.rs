//! Rust syntax helpers shared by the emitters.

use proc_macro2::{Ident, Literal, Span, TokenStream};
use quote::{ToTokens, format_ident, quote};

use crate::emit::rust::names::{pascal, snake};
use crate::emit::rust::space::{Space, WORD};
use crate::ir::names::Name;
use crate::ir::types::{Format, Ty, TypeId};
use crate::settings::FormatLibs;

/// Returns a Rust string literal with every special character escaped.
pub(crate) fn lit(s: &str) -> Literal {
    Literal::string(s)
}

/// Returns an identifier. Every name reaching here went through `ir::names`.
pub(crate) fn ident(s: &str) -> Ident {
    Ident::new(s, Span::call_site())
}

/// Returns the type at `p`, a path of identifiers such as `crate::ids::OrderId` or `::time::Date`.
pub(crate) fn path(p: &str) -> TokenStream {
    let lead = p.starts_with("::").then(|| quote!(::));
    let segs = p.trim_start_matches("::").split("::").map(ident);
    quote!(#lead #(#segs)::*)
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

/// Returns the Rust type of `ty`. Named types get `types::` when `qualify` is set.
pub(crate) fn ty(space: &Space<'_>, t: &Ty, qualify: bool) -> TokenStream {
    let named = |id: TypeId| {
        let n = pascal_ident(&space.types.get(id).name);
        if qualify { quote!(types::#n) } else { quote!(#n) }
    };
    ty_with(space, t, &named)
}

/// Returns [`ty`] with named types written by `named`.
fn ty_with(space: &Space<'_>, t: &Ty, named: &dyn Fn(TypeId) -> TokenStream) -> TokenStream {
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

/// Returns the Rust type of a string format: the library of the target crate or `String`.
pub(crate) fn format_ty(libs: &FormatLibs, f: Format) -> TokenStream {
    library_ty(libs, f).unwrap_or_else(|| text_ty(f))
}

/// The runtime's checked text types of the formats: `runtime::UuidStr` and the rest.
pub(crate) const FORMAT_STRS: [&str; 23] = [
    "DateStr",
    "DateTimeLocalStr",
    "DateTimeStr",
    "Decimal128Str",
    "DecimalStr",
    "DurationStr",
    "EmailStr",
    "HostnameStr",
    "HttpDateStr",
    "IdnEmailStr",
    "IdnHostnameStr",
    "Ipv4CidrStr",
    "Ipv6CidrStr",
    "IriReferenceStr",
    "IriStr",
    "JsonPointerStr",
    "RegexStr",
    "RelativeJsonPointerStr",
    "TimeLocalStr",
    "UriReferenceStr",
    "UriStr",
    "UriTemplateStr",
    "UuidStr",
];

/// Returns the runtime type of the text of a format that the crate has no library type for: `runtime::UuidStr` and so
/// on. Only text of the format is one.
fn text_ty(f: Format) -> TokenStream {
    let name = match f {
        Format::Uuid => "UuidStr",
        Format::DateTime => "DateTimeStr",
        Format::Date => "DateStr",
        // The runtime's `FullTime` stores a time with its offset with any date library (`library_ty`).
        Format::Time => "FullTime",
        Format::DateTimeLocal => "DateTimeLocalStr",
        Format::TimeLocal => "TimeLocalStr",
        Format::Uri => "UriStr",
        Format::UriReference => "UriReferenceStr",
        Format::Iri => "IriStr",
        Format::IriReference => "IriReferenceStr",
        Format::Ipv4Cidr => "Ipv4CidrStr",
        Format::Ipv6Cidr => "Ipv6CidrStr",
        Format::Decimal => "DecimalStr",
        Format::Decimal128 => "Decimal128Str",
        Format::HttpDate => "HttpDateStr",
        Format::Duration => "DurationStr",
        Format::JsonPointer => "JsonPointerStr",
        Format::RelativeJsonPointer => "RelativeJsonPointerStr",
        Format::Regex => "RegexStr",
        Format::UriTemplate => "UriTemplateStr",
        // Each of these has a type of its own in every crate (`library_ty`).
        Format::Ipv4
        | Format::Ipv6
        | Format::Base64Url
        | Format::Char
        | Format::Email
        | Format::IdnEmail
        | Format::Hostname
        | Format::IdnHostname => return quote!(String),
    };
    let name = ident(name);
    quote!(runtime::#name)
}

/// Returns a format that the target crate has no library for. It is a `runtime::Formatted` text, which checks it.
pub(crate) fn untyped(libs: &FormatLibs, f: Format) -> bool {
    library_ty(libs, f).is_none()
}

/// Returns the type that a format gets from the standard library, the runtime or the library of the target crate.
/// `None` for its text.
fn library_ty(libs: &FormatLibs, f: Format) -> Option<TokenStream> {
    if let Some(named) = libs.named.get(&f) {
        return Some(path(named.path()));
    }
    match (f, libs.uuid, libs.jiff) {
        (Format::Char, ..) => Some(quote!(char)),
        // The type features of the runtime re-export their crates.
        (Format::Uri, ..) if libs.url => Some(quote!(runtime::url::Url)),
        (Format::Ipv4Cidr, ..) if libs.ipnet => Some(quote!(runtime::ipnet::Ipv4Net)),
        (Format::Ipv6Cidr, ..) if libs.ipnet => Some(quote!(runtime::ipnet::Ipv6Net)),
        (Format::DateTimeLocal, _, true) => Some(quote!(runtime::jiff::civil::DateTime)),
        // The standard library's addresses (de)serialize as their text form.
        (Format::Ipv4, ..) => Some(quote!(std::net::Ipv4Addr)),
        (Format::Ipv6, ..) => Some(quote!(std::net::Ipv6Addr)),
        (Format::Base64Url, ..) => Some(quote!(runtime::Base64Url)),
        // Types that only a valid value can be. The `idn` feature of the runtime has them. The description turns it on.
        (Format::Email, ..) => Some(quote!(runtime::EmailStr)),
        (Format::IdnEmail, ..) => Some(quote!(runtime::IdnEmailStr)),
        (Format::Hostname, ..) => Some(quote!(runtime::HostnameStr)),
        (Format::IdnHostname, ..) => Some(quote!(runtime::IdnHostnameStr)),
        // `jiff` has no time with an offset. The runtime's stores plain numbers, with or without `jiff`.
        (Format::Time, ..) => Some(quote!(runtime::FullTime)),
        (Format::TimeLocal, _, true) => Some(quote!(runtime::jiff::civil::Time)),
        (Format::Uuid, true, _) => Some(quote!(runtime::uuid::Uuid)),
        (Format::DateTime, _, true) => Some(quote!(runtime::jiff::Timestamp)),
        (Format::Date, _, true) => Some(quote!(runtime::jiff::civil::Date)),
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
        | (Format::DateTime | Format::Date | Format::DateTimeLocal | Format::TimeLocal, _, false) => None,
    }
}

/// Returns the marker in `runtime::formats::format` of a format whose library type accepts more than the format allows.
/// `jiff` accepts `2024-06-19 08:30Z` as a `date-time` and `uuid` accepts `{...}`. `None` for text, the standard
/// library's addresses and the runtime's own types: those accept only valid values. A type that the table sets parses
/// text that honig checked first.
fn validated_format(libs: &FormatLibs, f: Format) -> Option<Ident> {
    let checked = libs.named.contains_key(&f)
        || (libs.jiff && matches!(f, Format::DateTime | Format::DateTimeLocal | Format::TimeLocal | Format::Date))
        || (libs.uuid && f == Format::Uuid)
        || (libs.url && f == Format::Uri)
        || (libs.ipnet && matches!(f, Format::Ipv4Cidr | Format::Ipv6Cidr));
    f.marker().filter(|_| checked).map(ident)
}

/// Returns how `t` stores the text of its checked formats: a `runtime::formats::Holds` that decodes it with each text
/// checked before its library parses it. `None` when `t` contains none. Aliases are followed. A named type checks its
/// own.
pub(crate) fn holds(space: &Space<'_>, t: &Ty) -> Option<TokenStream> {
    let c = quote!(runtime::formats);
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
        | Ty::Unit => None,
        // A type of yours with limits decodes from JSON that they check first.
        Ty::Named(id) => {
            let def = space.types.get(*id);
            (def.rust_type.is_some() && def.own.limits() != crate::ir::types::Constraints::default()).then(|| {
                let check = format_ident!("{}Limits", pascal(&def.name));
                quote!(#c::Json<super::types::#check>)
            })
        }
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
    SelfInOwnFields.visit_file_mut(&mut file);
    if binary {
        CrateOnly.visit_file_mut(&mut file);
    }
    Ok(unparse(file))
}

/// Returns `file` printed by `prettyplease`, with the body of each top-level `macro_rules!` printed as the items it
/// expands to. `prettyplease` prints a macro body as raw tokens, one space between each: a body of trait impls would
/// look like one long line. A body that is not items stays as raw tokens.
pub(crate) fn unparse(mut file: syn::File) -> String {
    let mut bodies = Vec::new();
    for item in &mut file.items {
        let syn::Item::Macro(m) = item else { continue };
        let Some(text) = items_macro_text(m) else { continue };
        let marker = format_ident!("__HONIG_MACRO_{}", bodies.len());
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

/// Returns a call of a runtime macro whose body is items, `runtime::object! { .. }`, with the body printed as those
/// items. `None` for any other macro or a body that does not parse as items.
fn items_macro_text(m: &syn::ItemMacro) -> Option<String> {
    let path = &m.mac.path;
    let runtime = path.segments.len() == 2 && path.segments.first().is_some_and(|s| s.ident == "runtime");
    if !runtime || !matches!(m.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
        return None;
    }
    let items: syn::File = syn::parse2(m.mac.tokens.clone()).ok()?;
    let name = path.segments.last()?.ident.to_string();
    let body: String = prettyplease::unparse(&items)
        .lines()
        .map(|l| if l.is_empty() { "\n".to_owned() } else { format!("    {l}\n") })
        .collect();
    Some(format!("runtime::{name}! {{\n{body}}}\n"))
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

/// A struct or enum that contains itself refers to itself as `Self` in its fields (`clippy::use_self`).
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

    fn visit_item_macro_mut(&mut self, m: &mut syn::ItemMacro) {
        let path = &m.mac.path;
        if path.segments.len() == 2
            && path.segments.first().is_some_and(|s| s.ident == "runtime")
            && path.segments.last().is_some_and(|s| s.ident == "object")
            && let Ok(mut s) = syn::parse2::<syn::ItemStruct>(m.mac.tokens.clone())
        {
            self.visit_item_struct_mut(&mut s);
            m.mac.tokens = s.into_token_stream();
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

/// More arguments than this and clippy warns (`too_many_arguments`): a generated function says why it needs them.
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
/// How much larger than the next the largest enum variant may be before it is boxed (`clippy::large_enum_variant`
/// warns past 200).
const BOX_MARGIN: usize = 128;

/// Returns which variants to box so that the largest variant is no more than about
/// 128 bytes above the next one. `sizes` contains (unboxed size, boxable part size).
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
        // `String::new()` for the empty one (clippy `manual_string_new`).
        serde_json::Value::String(t) if t.is_empty() => quote!(serde_json::Value::String(String::new())),
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
        #![allow(clippy::missing_errors_doc, reason = "generated docs contain only the description's own prose")]
        #![allow(
            clippy::init_numbered_fields,
            reason = "a spec default builds its checked type as `Type { 0: value }`, which also works through the alias of a twin"
        )]
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

/// Returns the name of the field that stores the other properties of `s`.
pub(crate) fn extra_field(s: &crate::ir::types::Struct) -> String {
    free_field(s, "additional_properties")
}

/// Returns the field name of pattern `i` (from 0) of `s`: `pattern_1`, ..., with `_` added while a property has it.
pub(crate) fn pattern_field(s: &crate::ir::types::Struct, i: usize) -> Ident {
    ident(&free_field(s, &format!("pattern_{}", i + 1)))
}

/// Returns `base`, with `_` added while a property of `s` has that field name.
pub(crate) fn free_field(s: &crate::ir::types::Struct, base: &str) -> String {
    let mut name = base.to_owned();
    while s.fields.iter().any(|f| snake(&f.name) == name) {
        name.push('_');
    }
    name
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

    /// A type refers to a generated type or a runtime one when anything it contains does.
    #[test]
    fn a_type_mentions_what_it_holds() {
        use crate::ir::types::{Ty, TypeId};
        let named = Ty::Named(TypeId(0));
        assert_eq!(super::mentions(&Ty::Vec(Box::new(named.clone()))), (true, false));
        assert_eq!(super::mentions(&Ty::Vec(Box::new(Ty::Binary))), (false, true));
        assert_eq!(super::mentions(&Ty::Tuple(vec![named, Ty::Binary])), (true, true));
        assert_eq!(super::mentions(&Ty::Vec(Box::new(Ty::String))), (false, false));
    }

    /// A shared impl gets the `cfg` of its uses when they all have it, and none when one does not.
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
        assert_eq!(rustdoc("12. item\ncontinued"), "12. item\n    continued");
        assert_eq!(rustdoc("See a::b and c>d."), "See `a::b` and `c>d`.");
        assert_eq!(rustdoc(". no item\nnext"), ". no item\nnext");
        assert_eq!(rustdoc("GitHub is fine"), "GitHub is fine");
        assert_eq!(rustdoc("CamelCase's API"), "`CamelCase's` API");
    }
}
