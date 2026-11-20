//! Builders that take every value by name: one type parameter per required value, so `build` exists once each is set.
//! Each setter takes `impl TryInto` and keeps the first value that does not convert; `build` returns it, or the value.

use std::fmt;

use crate::BuildError;

/// A required value that a builder has not got yet.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unset;

/// A required value that a builder got. `None` when it did not convert: the builder holds the error.
#[derive(Debug, Clone)]
pub struct Set<T>(pub Option<T>);

/// Returns `value` converted to the type of `wire`, or the first error. After an error nothing converts.
#[doc(hidden)]
pub fn put<T, V>(wire: &'static str, value: V, error: Option<BuildError>) -> (Option<T>, Option<BuildError>)
where
    V: TryInto<T>,
    V::Error: fmt::Display,
{
    if error.is_some() {
        return (None, error);
    }
    match crate::field(wire, value) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    }
}

/// Writes a builder `$builder` of `$target`: one type parameter per required value, `Unset` until its setter runs.
/// Each setter takes `impl TryInto` and keeps the first value that does not convert; `build` exists once every
/// required value is set and returns that error, or the value (through `TryFrom` into `$outer` when given, for the
/// rules across fields).
///
/// - `required { field: Generic: Type = "wire" }`
/// - `optional { field (setter): Type = "wire" => wrap, start }`: `wrap` is `some`, `value` or `nullable`
/// - `fixed { field: start }`: values that no setter sets
#[doc(hidden)]
#[macro_export]
macro_rules! typestate_builder {
    (
        $builder:ident => $($target:ident)::+ $(, try_from $($outer:ident)::+)?;
        required { $($field:ident : $gen:ident : $ty:ty = $wire:literal),* $(,)? }
        optional { $($ofield:ident ($osetter:ident) : $oty:ty = $owire:literal => $wrap:ident, $ostart:expr),* $(,)? }
        fixed { $($xfield:ident : $xstart:expr),* $(,)? }
    ) => {
        /// Takes each value by name. `build` exists once every required one is set.
        #[allow(clippy::allow_attributes, clippy::struct_field_names, reason = "its fields are those of the type that it builds")]
        #[derive(Debug)]
        #[must_use = "a builder does nothing until `build`"]
        pub struct $builder<$($gen = $crate::Unset),*> {
            $($field: $gen,)*
            __error: Option<$crate::BuildError>,
            $($ofield: Option<$oty>,)*
        }

        impl $builder {
            pub const fn new() -> Self {
                Self { $($field: $crate::Unset,)* __error: None, $($ofield: None,)* }
            }
        }

        impl $crate::typestate_builder!(@out [$($target)::+] $([$($outer)::+])?) {
            /// Starts a builder that takes each value by name.
            pub fn builder() -> $builder {
                $builder::new()
            }
        }

        impl ::core::default::Default for $builder {
            fn default() -> Self {
                Self::new()
            }
        }

        $crate::typestate_builder!(@required $builder [] [$($field : $gen : $ty = $wire),*] [$($ofield),*]);

        #[allow(clippy::allow_attributes, unused_mut, reason = "a builder without optional values")]
        impl<$($gen),*> $builder<$($gen),*> {
            $(
                pub fn $osetter(mut self, value: impl TryInto<$oty, Error: ::core::fmt::Display>) -> Self {
                    let (value, error) = $crate::put($owire, value, self.__error.take());
                    self.__error = error;
                    self.$ofield = value.or(self.$ofield);
                    self
                }
            )*
        }

        impl $builder<$($crate::Set<$ty>),*> {
            /// Returns the value: the first one that did not convert, or a rule across fields that it breaks.
            pub fn build(self) -> Result<$crate::typestate_builder!(@out [$($target)::+] $([$($outer)::+])?), $crate::BuildError> {
                if let Some(e) = self.__error {
                    return Err(e);
                }
                let value = $($target)::+ {
                    $($field: match self.$field.0 { Some(v) => v, None => return Err($crate::BuildError::value(String::new())) },)*
                    $($ofield: match self.$ofield { Some(v) => $crate::typestate_builder!(@wrap $wrap v), None => $ostart },)*
                    $($xfield: $xstart,)*
                };
                $crate::typestate_builder!(@finish value $([$($outer)::+])?)
            }
        }
    };
    (@out [$($target:tt)*]) => { $($target)* };
    (@out [$($target:tt)*] [$($outer:tt)*]) => { $($outer)* };
    (@finish $value:ident) => { Ok($value) };
    (@finish $value:ident [$($outer:tt)*]) => { <$($outer)* as ::core::convert::TryFrom<_>>::try_from($value) };
    (@wrap some $v:ident) => { Some($v) };
    (@wrap value $v:ident) => { $v };
    (@wrap nullable $v:ident) => { $crate::Nullable::Value($v) };
    (@required $builder:ident [$($bf:ident : $bg:ident),*] [] [$($of:ident),*]) => {};
    (
        @required $builder:ident [$($bf:ident : $bg:ident),*]
        [$f:ident : $g:ident : $t:ty = $w:literal $(, $af:ident : $ag:ident : $at:ty = $aw:literal)*]
        [$($of:ident),*]
    ) => {
        impl<$($bg,)* $($ag,)*> $builder<$($bg,)* $crate::Unset, $($ag,)*> {
            pub fn $f(self, value: impl TryInto<$t, Error: ::core::fmt::Display>) -> $builder<$($bg,)* $crate::Set<$t>, $($ag,)*> {
                let (value, error) = $crate::put($w, value, self.__error);
                $builder { $($bf: self.$bf,)* $f: $crate::Set(value), $($af: self.$af,)* __error: error, $($of: self.$of,)* }
            }
        }
        $crate::typestate_builder!(@required $builder [$($bf : $bg,)* $f : $g] [$($af : $ag : $at = $aw),*] [$($of),*]);
    };
}

#[cfg(test)]
mod tests {
    #[derive(Debug, PartialEq, Eq)]
    struct Pair {
        a: u8,
        b: u8,
        c: Option<u8>,
        d: u8,
    }

    crate::typestate_builder! {
        PairBuilder => Pair;
        required { a: A: u8 = "a", b: B: u8 = "b" }
        optional { c (c): u8 = "c" => some, None }
        fixed { d: 7 }
    }

    #[test]
    fn required_values_go_by_name_in_any_order() {
        let p = PairBuilder::new().b(2_i64).c(3_i64).a(1_i64).build();
        assert_eq!(p, Ok(Pair { a: 1, b: 2, c: Some(3), d: 7 }));
    }

    #[test]
    fn the_first_value_that_does_not_convert_is_the_error() {
        let err = PairBuilder::new().a(300_i64).b(-1_i64).build().unwrap_err();
        assert!(err.to_string().starts_with("/a: "), "{err}");
    }
}
