//! Diagnostics collected across every pass. A pass never aborts on the first
//! problem: it records it with a JSON pointer and keeps going.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
/// Severity of a diagnostic.
pub enum Level {
    /// Output is still produced, maybe without the affected part.
    Warning,
    /// The document cannot be processed.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
/// One problem found in a document with where it is.
pub struct Diagnostic {
    /// How serious it is.
    pub level: Level,
    /// RFC 6901 JSON pointer into the source document such as `/paths/~1orders/get`.
    pub pointer: String,
    /// What is wrong for humans.
    pub message: String,
    /// Whether `generate` stops for this warning. It stops for every error.
    pub(crate) refuses: bool,
}

impl Diagnostic {
    /// Returns an error at `pointer`.
    pub fn error(pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self { level: Level::Error, pointer: pointer.into(), message: message.into(), refuses: false }
    }

    /// Returns a warning at `pointer`.
    pub fn warning(pointer: impl Into<String>, message: impl Into<String>) -> Self {
        Self { level: Level::Warning, pointer: pointer.into(), message: message.into(), refuses: false }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.level {
            Level::Warning => "warning",
            Level::Error => "error",
        };
        let pointer = if self.pointer.is_empty() { "#" } else { &self.pointer };
        write!(f, "{level}: {pointer}: {}", self.message)
    }
}

/// Collector threaded through passes.
#[derive(Debug, Default, Clone)]
pub struct Diagnostics(pub Vec<Diagnostic>);

impl Diagnostics {
    /// Records a diagnostic.
    pub fn push(&mut self, d: Diagnostic) {
        self.0.push(d);
    }

    /// Records an error.
    pub fn error(&mut self, pointer: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::error(pointer, message));
    }

    /// Records a warning.
    pub fn warning(&mut self, pointer: impl Into<String>, message: impl Into<String>) {
        self.push(Diagnostic::warning(pointer, message));
    }

    /// Makes an error of every warning that stops `generate` and says whether any diagnostic is an error.
    pub(crate) fn refused(&mut self) -> bool {
        for d in self.0.iter_mut().filter(|d| d.refuses) {
            d.level = Level::Error;
        }
        self.has_errors()
    }

    /// Makes an error of every warning, for `--deny-warnings`, and says whether there was one.
    pub(crate) fn deny_warnings(&mut self) -> bool {
        let mut any = false;
        for d in self.0.iter_mut().filter(|d| d.level == Level::Warning) {
            d.level = Level::Error;
            any = true;
        }
        any
    }

    /// Is any diagnostic an error?
    pub fn has_errors(&self) -> bool {
        self.0.iter().any(|d| d.level == Level::Error)
    }

    /// Returns whether there are no diagnostics at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns every diagnostic in the order recorded.
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.0.iter()
    }

    /// Appends every diagnostic of `other`.
    pub fn extend(&mut self, other: Self) {
        self.0.extend(other.0);
    }

    /// Returns the lines to print: one per distinct (level, message) in the order first recorded. A message recorded at
    /// several pointers is printed once and lists every pointer under it. `quiet` keeps only errors.
    #[must_use]
    pub fn lines(&self, quiet: bool) -> Vec<String> {
        let mut groups: Vec<(&Diagnostic, Vec<&str>)> = vec![];
        for d in self.0.iter().filter(|d| !quiet || d.level == Level::Error) {
            match groups.iter_mut().find(|(g, _)| g.level == d.level && g.message == d.message) {
                Some((_, pointers)) => pointers.push(&d.pointer),
                None => groups.push((d, vec![&d.pointer])),
            }
        }
        groups
            .into_iter()
            .map(|(d, pointers)| {
                if pointers.len() == 1 {
                    return d.to_string();
                }
                let level = if d.level == Level::Error { "error" } else { "warning" };
                let at: String =
                    pointers.iter().map(|p| format!("\n  at {}", if p.is_empty() { "#" } else { p })).collect();
                format!("{level}: {} ({} places){at}", d.message, pointers.len())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_or_a_refusing_warning_stops_generation() {
        let mut d = Diagnostics::default();
        d.warning("/a", "reported only");
        assert!(!d.refused());
        d.push(Diagnostic { refuses: true, ..Diagnostic::warning("/b", "a warning that stops it") });
        assert!(d.refused());
        assert!(d.has_errors(), "it is an error once it stops generation");
        let mut d = Diagnostics::default();
        d.error("/c", "an error");
        assert!(d.refused());
    }

    #[test]
    fn a_message_at_several_pointers_prints_once() {
        let d = Diagnostics(vec![
            Diagnostic::warning("/paths/~1orders/get", "same"),
            Diagnostic::error("/info", "other"),
            Diagnostic::warning("/paths/~1products/get", "same"),
        ]);
        assert_eq!(
            d.lines(false),
            vec![
                "warning: same (2 places)\n  at /paths/~1orders/get\n  at /paths/~1products/get".to_owned(),
                "error: /info: other".to_owned(),
            ]
        );
        assert_eq!(d.lines(true), vec!["error: /info: other".to_owned()]);
    }
}
