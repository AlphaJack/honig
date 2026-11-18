//! What generated builders share. A setter takes anything that converts to the type of its field. A value that does
//! not convert is kept as the first error, and `build` returns it. So a chain of setters needs one `?` and not one per
//! field.

use std::fmt;

/// A field of a builder whose value did not convert to its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    field: &'static str,
    message: String,
}

impl BuildError {
    /// Returns the error of a built value that breaks a rule of its whole schema (`if`/`then`, `not`, ...). It names no
    /// field.
    #[must_use]
    pub const fn value(message: String) -> Self {
        Self { field: "", message }
    }

    /// Returns the name of the field as the description spells it. It is empty for a rule of the whole value.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        self.field
    }

    /// Returns why the value did not convert.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            return f.write_str(&self.message);
        }
        write!(f, "`{}`: {}", self.field, self.message)
    }
}

impl std::error::Error for BuildError {}

/// Returns `value` converted to the type of `field`.
///
/// # Errors
///
/// The value does not convert: the error names the field.
pub fn field<F, V>(field: &'static str, value: V) -> Result<F, BuildError>
where
    V: TryInto<F>,
    V::Error: fmt::Display,
{
    value.try_into().map_err(|e| BuildError { field, message: e.to_string() })
}

/// Builds a `T` one field at a time and keeps the first field that does not convert.
#[derive(Debug)]
#[must_use = "a builder does nothing until `build`"]
pub struct Builder<T>(Result<T, BuildError>);

impl<T> Builder<T> {
    /// Starts from what `start` returns: the value made from its required fields, or the first that did not convert.
    pub fn start(start: impl FnOnce() -> Result<T, BuildError>) -> Self {
        Self(start())
    }

    /// Sets `field` through `put` once `value` converts. After an error it does nothing: the first error stays.
    pub fn set<F, V>(self, name: &'static str, value: V, put: impl FnOnce(&mut T, F)) -> Self
    where
        V: TryInto<F>,
        V::Error: fmt::Display,
    {
        let Ok(mut built) = self.0 else { return self };
        match field(name, value) {
            Ok(v) => {
                put(&mut built, v);
                Self(Ok(built))
            }
            Err(e) => Self(Err(e)),
        }
    }

    /// Returns the value.
    ///
    /// # Errors
    ///
    /// The first field whose value did not convert.
    pub fn build(self) -> Result<T, BuildError> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq)]
    struct Pair {
        a: u8,
        b: u8,
    }

    #[test]
    fn the_first_field_that_does_not_convert_is_the_error() {
        let ok = Builder::start(|| Ok(Pair::default())).set("a", 1_i64, |p, v| p.a = v).set("b", 2_i64, |p, v| p.b = v);
        assert_eq!(ok.build(), Ok(Pair { a: 1, b: 2 }));
        let bad =
            Builder::start(|| Ok(Pair::default())).set("a", 300_i64, |p, v| p.a = v).set("b", -1_i64, |p, v| p.b = v);
        let err = bad.build().expect_err("300 is past u8");
        assert_eq!(err.field(), "a");
        assert_eq!(err.to_string(), format!("`a`: {}", u8::try_from(300_i64).expect_err("past u8")));
    }

    #[test]
    fn a_rule_of_the_whole_value_names_no_field() {
        let err = BuildError::value("`customs` is required".to_owned());
        assert_eq!((err.field(), err.to_string()), ("", "`customs` is required".to_owned()));
    }

    #[test]
    fn a_required_field_that_does_not_convert_skips_the_setters() {
        let start = || Ok(Pair { a: field("a", 256_i64)?, b: 0 });
        let err = Builder::start(start).set("b", 1_i64, |p, v| p.b = v).build().expect_err("256 is past u8");
        assert_eq!(err.field(), "a");
    }
}
