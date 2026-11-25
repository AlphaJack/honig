//! The bodies of honig's macros as text. `prettyplease` prints a macro body as raw tokens with one space between each,
//! wrapped where the line ends: a builder of twenty fields reads as one block of tokens. These print each body as a
//! person would write it.

use proc_macro2::{Delimiter, Spacing, TokenStream, TokenTree};

/// The honig macros whose body is a list: each entry goes on a line of its own.
const LISTS: [&str; 3] = ["typestate_builder", "response_error", "union_serialize"];

/// The longest body that stays on the line of its call, as `prettyplease` prints it.
const INLINE: usize = 80;

/// Returns `file` printed by `prettyplease`, with the body of each honig macro printed as written: items for
/// `object!`, one entry per line for the list macros. A macro inside an inline module is printed at its indentation.
pub(crate) fn unparse(mut file: syn::File) -> String {
    let mut bodies = Vec::new();
    mark(&mut file.items, &mut bodies);
    let printed = prettyplease::unparse(&file);
    // The placeholders come out in the order they were made: one pass from the start replaces each.
    let (mut out, mut rest) = (String::with_capacity(printed.len()), printed.as_str());
    for (marker, text) in bodies {
        let Some((before, after)) = rest.split_once(&marker) else { continue };
        let indent = before.rsplit('\n').next().unwrap_or_default();
        out.push_str(before);
        for (i, line) in text.trim_end().lines().enumerate() {
            if i > 0 {
                out.push('\n');
                if !line.is_empty() {
                    out.push_str(indent);
                }
            }
            out.push_str(line);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Puts a placeholder item in place of each honig macro of `items` and their inline modules whose body prints better
/// than `prettyplease` prints it, and keeps the placeholder with that text.
fn mark(items: &mut [syn::Item], bodies: &mut Vec<(String, String)>) {
    for item in items {
        if let syn::Item::Mod(m) = item {
            if let Some((_, inner)) = &mut m.content {
                mark(inner, bodies);
            }
            continue;
        }
        let syn::Item::Macro(m) = item else { continue };
        let Some(text) = items_macro_text(m).or_else(|| list_macro_text(m)) else { continue };
        let marker = quote::format_ident!("__HONIG_MACRO_{}", bodies.len());
        let attrs = &m.attrs;
        let Ok(placeholder) = syn::parse2::<syn::Item>(quote::quote!(#(#attrs)* const #marker: () = ();)) else {
            continue;
        };
        *item = placeholder;
        bodies.push((format!("const {marker}: () = ();"), text));
    }
}

/// Is `path` that of a honig macro, `::honig::<name>`?
pub(super) fn honig_macro(path: &syn::Path) -> bool {
    path.leading_colon.is_some()
        && path.segments.len() == 2
        && path.segments.first().is_some_and(|s| s.ident == "honig")
}

/// Returns a call of a honig macro whose body is items, `::honig::object! { .. }`, with the body printed as those
/// items. `None` for any other macro or a body that does not parse as items.
fn items_macro_text(m: &syn::ItemMacro) -> Option<String> {
    let path = &m.mac.path;
    if !honig_macro(path) || !matches!(m.mac.delimiter, syn::MacroDelimiter::Brace(_)) {
        return None;
    }
    let items: syn::File = syn::parse2(m.mac.tokens.clone()).ok()?;
    let name = path.segments.last()?.ident.to_string();
    let body: String = prettyplease::unparse(&items)
        .lines()
        .map(|l| if l.is_empty() { "\n".to_owned() } else { format!("    {l}\n") })
        .collect();
    Some(format!("::honig::{name}! {{\n{body}}}\n"))
}

/// Returns a call of a honig list macro with one entry of each braced list per line. `None` for another macro, and for
/// a body short enough for the line of its call.
fn list_macro_text(m: &syn::ItemMacro) -> Option<String> {
    let path = &m.mac.path;
    let name = path.segments.last()?.ident.to_string();
    if !honig_macro(path) || !LISTS.contains(&name.as_str()) || inline(m.mac.tokens.clone()).len() <= INLINE {
        return None;
    }
    // The optional fields of a builder are `field(setter): Type = "wire" => wrap, start`: two pieces each.
    let pairs = name == "typestate_builder";
    let body: String = lines(&m.mac.tokens, pairs).iter().map(|l| format!("    {l}\n")).collect();
    Some(match m.mac.delimiter {
        syn::MacroDelimiter::Paren(_) => format!("::honig::{name}!(\n{body});\n"),
        syn::MacroDelimiter::Brace(_) | syn::MacroDelimiter::Bracket(_) => format!("::honig::{name}! {{\n{body}}}\n"),
    })
}

/// Returns the lines of a list macro's body: a header up to `;` on its own line, and each braced list as a block of one
/// entry per line. `pairs`: the list after `optional` has two pieces per entry.
fn lines(tokens: &TokenStream, pairs: bool) -> Vec<String> {
    let (mut out, mut words) = (vec![], Vec::<TokenTree>::new());
    for tt in tokens.clone() {
        match &tt {
            TokenTree::Punct(p) if p.as_char() == ';' => {
                words.push(tt);
                out.push(inline(std::mem::take(&mut words).into_iter().collect()));
            }
            TokenTree::Group(g) if g.delimiter() == Delimiter::Brace => {
                let head = inline(std::mem::take(&mut words).into_iter().collect());
                let entries = entries(&g.stream(), pairs && head == "optional");
                let open = if head.is_empty() { "{".to_owned() } else { format!("{head} {{") };
                if entries.is_empty() {
                    out.push(format!("{open}}}"));
                } else {
                    out.push(open);
                    out.extend(entries.iter().map(|e| format!("    {e},")));
                    out.push("}".to_owned());
                }
            }
            TokenTree::Punct(_) | TokenTree::Group(_) | TokenTree::Ident(_) | TokenTree::Literal(_) => words.push(tt),
        }
    }
    if !words.is_empty() {
        out.push(inline(words.into_iter().collect()));
    }
    out
}

/// Returns the entries of a list: its pieces between commas outside `<..>`, two pieces to an entry with `pairs`.
fn entries(tokens: &TokenStream, pairs: bool) -> Vec<String> {
    let (mut pieces, mut piece, mut depth) = (vec![], Vec::<TokenTree>::new(), 0_usize);
    let mut arrow = false;
    for tt in tokens.clone() {
        if let TokenTree::Punct(p) = &tt {
            match p.as_char() {
                '<' => depth += 1,
                '>' if !arrow => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    pieces.push(inline(std::mem::take(&mut piece).into_iter().collect()));
                    continue;
                }
                _ => {}
            }
            arrow = matches!(p.as_char(), '=' | '-') && p.spacing() == Spacing::Joint;
        } else {
            arrow = false;
        }
        piece.push(tt);
    }
    if !piece.is_empty() {
        pieces.push(inline(piece.into_iter().collect()));
    }
    if pairs { pieces.chunks(2).map(|c| c.join(", ")).collect() } else { pieces }
}

/// What came before a token: whether a space goes between it and a word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Before {
    /// The start, an opening bracket, a path separator or a prefix such as `&`: no space.
    Tight,
    /// A word, a literal or a closed group: a space before the next word.
    Word,
    /// A `,`, `;` or a lone `:`: a space before anything that follows.
    Separator,
    /// An operator printed with a space on each side: no further space.
    Spaced,
}

/// Returns `tokens` on one line with the spacing of Rust source: `Vec<types::Planet>`, `name(name): String`, `a => b`.
fn inline(tokens: TokenStream) -> String {
    let (mut out, mut before) = (String::new(), Before::Tight);
    let mut tokens = tokens.into_iter().peekable();
    while let Some(tt) = tokens.next() {
        let spaced = matches!(before, Before::Word | Before::Separator);
        match &tt {
            TokenTree::Ident(_) | TokenTree::Literal(_) => {
                if spaced {
                    out.push(' ');
                }
                out.push_str(&tt.to_string());
                before = Before::Word;
            }
            TokenTree::Group(g) => {
                let (open, close) = match g.delimiter() {
                    Delimiter::Parenthesis => ("(", ")"),
                    Delimiter::Bracket => ("[", "]"),
                    Delimiter::Brace => ("{ ", " }"),
                    Delimiter::None => ("", ""),
                };
                let call = before == Before::Word && g.delimiter() != Delimiter::Brace;
                if spaced && !call {
                    out.push(' ');
                }
                let inner = inline(g.stream());
                if inner.is_empty() && g.delimiter() == Delimiter::Brace {
                    out.push_str("{}");
                } else {
                    out.push_str(&format!("{open}{inner}{close}"));
                }
                before = Before::Word;
            }
            TokenTree::Punct(p) => {
                let next =
                    tokens.peek().and_then(|t| if let TokenTree::Punct(n) = t { Some(n.as_char()) } else { None });
                let joint = p.spacing() == Spacing::Joint;
                match (p.as_char(), joint, next) {
                    ('=' | '-', true, Some('>')) => {
                        tokens.next();
                        out.push_str(if p.as_char() == '=' { " => " } else { " -> " });
                        before = Before::Spaced;
                    }
                    (':', true, Some(':')) => {
                        tokens.next();
                        if before == Before::Separator {
                            out.push(' ');
                        }
                        out.push_str("::");
                        before = Before::Tight;
                    }
                    ('=', _, _) => {
                        out.push_str(" = ");
                        before = Before::Spaced;
                    }
                    (',' | ';' | ':', _, _) => {
                        out.push(p.as_char());
                        before = Before::Separator;
                    }
                    ('<' | '&' | '.' | '#', _, _) => {
                        if p.as_char() == '&' && spaced {
                            out.push(' ');
                        }
                        out.push(p.as_char());
                        before = Before::Tight;
                    }
                    (c, _, _) => {
                        out.push(c);
                        before = Before::Word;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::inline;

    #[test]
    fn tokens_read_as_rust() {
        let line = |ts: proc_macro2::TokenStream| inline(ts);
        assert_eq!(
            line(quote::quote!(tags(tags) : Vec<String> = "tags" => some)),
            r#"tags(tags): Vec<String> = "tags" => some"#
        );
        assert_eq!(line(quote::quote!(image : ::honig::Nullable::Absent)), "image: ::honig::Nullable::Absent");
        assert_eq!(
            line(quote::quote!(Self::Created { .. } => "status 201")),
            r#"Self::Created { .. } => "status 201""#
        );
        assert_eq!(
            line(quote::quote!(Planet => super::Planet, try_from super::P;)),
            "Planet => super::Planet, try_from super::P;"
        );
    }
}
