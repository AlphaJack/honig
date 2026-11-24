//! Matching a request path against the generated route table.

use std::borrow::Cow;

/// A piece of a path template.
#[derive(Debug, Clone, Copy)]
pub enum Part {
    /// Literal text.
    Lit(&'static str),
    /// A path variable.
    Var,
}

/// Returns the path with percent-encoded unreserved characters decoded (RFC 3986
/// section 6.2.2.2). `/a/%73b` matches `/a/sb`. `/a/%40` still does not match `/a/@`.
pub fn normalize_path(path: &str) -> Cow<'_, str> {
    if !path.contains('%') {
        return Cow::Borrowed(path);
    }
    let hex = crate::escape::hex_digit;
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(&h), Some(&l)) = (bytes.get(i + 1), bytes.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            let c = char::from(u8::try_from(h * crate::escape::HEX + l).unwrap_or(0));
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~') {
                out.push(c);
                i += crate::escape::PCT_ESCAPE;
                continue;
            }
        }
        let end = path.ceil_char_boundary(i + 1);
        out.push_str(path.get(i..end).unwrap_or_default());
        i = end;
    }
    Cow::Owned(out)
}

/// Returns the entry of `method` among the methods of a path. A `HEAD` uses the `GET` entry when the path declares no
/// `HEAD` of its own: a server supports `HEAD` wherever it supports `GET` (RFC 9110 section 9.1).
pub fn find_method<'m, T>(methods: &'m [(&'static str, T)], method: &str) -> Option<&'m (&'static str, T)> {
    let exact = |m: &str| methods.iter().find(|(own, _)| *own == m);
    exact(method).or_else(|| if method == "HEAD" { exact("GET") } else { None })
}

/// Returns the `Allow` of a path: its methods and `HEAD` when it has `GET`.
#[must_use]
pub fn allowed_methods<T>(methods: &[(&'static str, T)]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = methods.iter().map(|(m, _)| *m).collect();
    if out.contains(&"GET") && !out.contains(&"HEAD") {
        out.push("HEAD");
    }
    out
}

/// A node of the generated route trie: the path templates segment by segment.
#[derive(Debug)]
pub struct Node {
    /// Literal segments sorted by length and then bytes with their child.
    pub lits: &'static [(&'static str, u32)],
    /// Segments mixing text and variables (`{name}.{ext}`) and their child,
    /// most specific first.
    pub mixed: &'static [(&'static [Part], u32)],
    /// The `{name}` child or [`Node::NONE`].
    pub var: u32,
    /// The template that ends here or [`Node::NONE`].
    pub leaf: u32,
}

impl Node {
    /// No child or no template ends here.
    pub const NONE: u32 = u32::MAX;
}

/// Returns a [`Node`]: literal children, mixed children, the variable child and the
/// template that ends here. It is positional so that a generated table stays one line per node.
#[must_use]
pub const fn node(
    lits: &'static [(&'static str, u32)],
    mixed: &'static [(&'static [Part], u32)],
    var: u32,
    leaf: u32,
) -> Node {
    Node { lits, mixed, var, leaf }
}

/// Routes a request path through the trie with the root first.
///
/// It goes segment by segment.
/// A literal child is tried first, then mixed ones and then the variable one.
/// It backtracks on a dead end. Among mixed siblings that match the most
/// specific template wins. This gives the same template as trying every template in
/// specificity order. Returns the template and the raw variable
/// values in path order.
pub fn route<'p>(nodes: &[Node], path: &'p str) -> Option<(usize, Captures<'p>)> {
    let mut caps = Captures::default();
    let leaf = walk(nodes, 0, Some(path.strip_prefix('/').unwrap_or(path)), &mut caps)?;
    Some((leaf as usize, caps))
}

/// The raw path variables of a route in path order. They are stored inline (no
/// allocation) up to [`Captures::INLINE`] and on the heap past that.
#[derive(Debug, Clone)]
pub struct Captures<'p> {
    inline: [&'p str; Captures::INLINE],
    len: usize,
    heap: Option<Vec<&'p str>>,
}

impl Default for Captures<'_> {
    fn default() -> Self {
        Self { inline: [""; Captures::INLINE], len: 0, heap: None }
    }
}

impl<'p> Captures<'p> {
    /// Variables stored without allocating.
    pub const INLINE: usize = 8;

    fn push(&mut self, s: &'p str) {
        if let Some(h) = &mut self.heap {
            h.push(s);
        } else if self.len < Self::INLINE {
            self.inline[self.len] = s;
            self.len += 1;
        } else {
            let mut h = self.inline.to_vec();
            h.push(s);
            self.heap = Some(h);
        }
    }

    fn truncate(&mut self, n: usize) {
        if let Some(h) = &mut self.heap {
            h.truncate(n);
        } else {
            self.len = self.len.min(n);
        }
    }
}

impl<'p> std::ops::Deref for Captures<'p> {
    type Target = [&'p str];
    fn deref(&self) -> &[&'p str] {
        match &self.heap {
            Some(h) => h,
            None => &self.inline[..self.len],
        }
    }
}

/// Where the path variables of a route are: byte ranges of the path that was routed. The server keeps them from routing
/// to the operation, so that the path is routed once.
#[derive(Debug, Clone, Default)]
pub struct Spans(Vec<std::ops::Range<usize>>);

impl Spans {
    /// Returns where `caps`, slices of `path`, are in it.
    #[must_use]
    pub fn of(path: &str, caps: &[&str]) -> Self {
        let start = path.as_ptr().addr();
        Self(caps.iter().map(|c| c.as_ptr().addr() - start..c.as_ptr().addr() - start + c.len()).collect())
    }

    /// Returns the variables in `path`, the path that was routed.
    #[must_use]
    pub fn captures<'p>(&self, path: &'p str) -> Captures<'p> {
        let mut caps = Captures::default();
        for r in &self.0 {
            caps.push(path.get(r.clone()).unwrap_or_default());
        }
        caps
    }
}

/// Literal children this few are scanned; more are binary searched.
const SCAN: usize = 8;

fn find_lit(lits: &[(&'static str, u32)], seg: &str) -> Option<u32> {
    if lits.len() <= SCAN {
        lits.iter().find(|(l, _)| *l == seg).map(|(_, c)| *c)
    } else {
        // Sorted by length and then bytes. Most steps compare an integer.
        let k = lits.binary_search_by(|(l, _)| l.len().cmp(&seg.len()).then_with(|| (*l).cmp(seg))).ok()?;
        Some(lits[k].1)
    }
}

/// Walks the trie from node `i`. `rest` is the path after the last consumed `/`. It is `None` once the last
/// segment is consumed (`/a/` keeps its empty last segment). It loops while
/// there is one way on. It recurses only where a later child could still
/// match after a dead end.
fn walk<'p>(nodes: &[Node], mut i: u32, mut rest: Option<&'p str>, caps: &mut Captures<'p>) -> Option<u32> {
    loop {
        let node = nodes.get(i as usize)?;
        let Some(p) = rest else {
            return (node.leaf != Node::NONE).then_some(node.leaf);
        };
        let (seg, tail) = match p.find('/') {
            Some(k) => (p.get(..k).unwrap_or_default(), p.get(k + 1..)),
            None => (p, None),
        };
        // A variable spans at least one char.
        let var = node.var != Node::NONE && !seg.is_empty();
        if let Some(c) = find_lit(node.lits, seg) {
            if node.mixed.is_empty() && !var {
                (i, rest) = (c, tail);
                continue;
            }
            let n = caps.len();
            if let Some(leaf) = walk(nodes, c, tail, caps) {
                return Some(leaf);
            }
            caps.truncate(n);
        }
        // Mixed siblings can both match (`{a}.json` and `{a}.{b}`). The more
        // specific template may be under either. So every match is walked and
        // the lowest leaf wins. Leaves are numbered most specific first.
        let mut best: Option<(u32, Vec<&'p str>)> = None;
        for (parts, child) in node.mixed {
            let n = caps.len();
            if match_from(parts, seg, caps)
                && let Some(leaf) = walk(nodes, *child, tail, caps)
            {
                if node.mixed.len() == 1 {
                    return Some(leaf);
                }
                if best.as_ref().is_none_or(|(b, _)| leaf < *b) {
                    best = Some((leaf, caps[n..].to_vec()));
                }
            }
            caps.truncate(n);
        }
        if let Some((leaf, found)) = best {
            for c in found {
                caps.push(c);
            }
            return Some(leaf);
        }
        if !var {
            return None;
        }
        caps.push(seg);
        (i, rest) = (node.var, tail);
    }
}

/// Matches `seg` against `parts` from the end. Each variable starts after the last place of the literal before it that
/// leaves the text before a match, so the first variable is the longest. One search per part: linear in `seg`.
fn match_from<'p>(parts: &[Part], seg: &'p str, caps: &mut Captures<'p>) -> bool {
    match parts.split_last() {
        // Exact: `/a/` and `/a//` are different paths in a description.
        None => seg.is_empty(),
        Some((Part::Lit(l), rest)) => seg.strip_suffix(l).is_some_and(|s| match_from(rest, s, caps)),
        Some((Part::Var, rest)) => {
            // A variable spans at least one char.
            let Some((last, _)) = seg.char_indices().next_back() else { return false };
            let before = seg.get(..last).unwrap_or_default();
            let start = match rest {
                [] => 0,
                [Part::Lit(l)] if before.starts_with(l) => l.len(),
                [Part::Lit(_)] => return false,
                [.., Part::Var] => last,
                [.., Part::Lit(l)] => match before.rfind(l) {
                    Some(k) => k + l.len(),
                    None => return false,
                },
            };
            let (head, var) = seg.split_at(start);
            if !match_from(rest, head, caps) {
                return false;
            }
            caps.push(var);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/{a}/{b}/...`: one variable per segment `depth` deep.
    fn chain(depth: u32) -> Vec<Node> {
        (0..=depth)
            .map(|i| {
                node(&[], &[], if i < depth { i + 1 } else { Node::NONE }, if i == depth { 0 } else { Node::NONE })
            })
            .collect()
    }

    #[test]
    fn captures_stay_in_order_inline_and_past_it() {
        for depth in [1, 8, 9, 12] {
            let nodes = chain(depth);
            let path: String = (0..depth).map(|i| ["/v", &i.to_string()].concat()).collect();
            let (leaf, caps) = route(&nodes, &path).expect("routes");
            assert_eq!(leaf, 0);
            let want: Vec<String> = (0..depth).map(|i| format!("v{i}")).collect();
            assert_eq!(caps.iter().map(|c| (*c).to_owned()).collect::<Vec<_>>(), want, "depth {depth}");
        }
    }

    #[test]
    fn a_mixed_segment_gives_the_first_variable_the_longest_match() {
        use Part::{Lit, Var};
        let git: &[Part] = &[Var, Lit("-"), Var, Lit(".git")];
        // No captures: no match.
        let cases: &[(&[Part], &str, &[&str])] = &[
            (git, "x-y-z.git", &["x-y", "z"]),
            (git, "a--b.git", &["a-", "b"]),
            (git, "\u{e9}-\u{fc}.git", &["\u{e9}", "\u{fc}"]),
            (git, "--.git", &[]),
            (git, "a-b", &[]),
            (&[Var, Lit("."), Var], "a.b.c", &["a.b", "c"]),
            (&[Lit("v"), Var, Lit("."), Var], "v1.2.3", &["1.2", "3"]),
            (&[Var, Lit("-"), Var, Lit("-"), Var], "a-b-c-d", &["a-b", "c", "d"]),
            (&[Var, Var], "\u{e9}\u{fc}", &["\u{e9}", "\u{fc}"]),
            (&[Lit("a"), Var, Lit("a")], "aaaa", &["aa"]),
            (&[Lit("a"), Var, Lit("a")], "aa", &[]),
        ];
        for (parts, seg, want) in cases {
            let mut caps = Captures::default();
            let got = if match_from(parts, seg, &mut caps) { caps.to_vec() } else { vec![] };
            assert_eq!(got, *want, "{parts:?} on {seg:?}");
        }
    }
}
