//! The object types of a generated module, written by one macro each: the struct with its serde attributes and its
//! `Serialize` and `Deserialize` through `NoSeq`.

/// Writes an object type: the struct with the serde attributes of each field, and its `Serialize` and `Deserialize`.
/// `Deserialize` decodes through `NoSeq`: `type: object` accepts no array. The serde attributes refer to `runtime::`
/// paths, so the calling module imports honig as `runtime`. The body is a struct, so it looks and prints as one.
///
/// A marker first on a field picks its kind:
///
/// - none: the field as it is, with only the attributes given
/// - `#[optional] pub f: T`: `Option<T>`, absent when `None`; `null` is refused
/// - `#[maybe] pub f: T`: `Option<T>`, absent when `None`; `null` decodes as absent
/// - `#[nullable] pub f: T`: `runtime::Nullable<T>`, absent when `Absent`
///
/// `#[unique_keys]` first on the struct writes through `UniqueKeys`: an extra map may not repeat a declared name.
#[doc(hidden)]
#[macro_export]
macro_rules! object {
    (#[unique_keys] $(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $crate::object!(@fields [$(#[$m])*] $name [UniqueKeys] [] $($body)*);
    };
    ($(#[$m:meta])* pub struct $name:ident { $($body:tt)* }) => {
        $crate::object!(@fields [$(#[$m])*] $name [] [] $($body)*);
    };
    (@fields $attrs:tt $name:ident $enc:tt [$($done:tt)*] #[optional] $(#[$fm:meta])* pub $f:ident : $t:ty, $($rest:tt)*) => {
        $crate::object!(@fields $attrs $name $enc [$($done)*
            #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "runtime::not_null")]
            $(#[$fm])* pub $f: Option<$t>,] $($rest)*);
    };
    (@fields $attrs:tt $name:ident $enc:tt [$($done:tt)*] #[maybe] $(#[$fm:meta])* pub $f:ident : $t:ty, $($rest:tt)*) => {
        $crate::object!(@fields $attrs $name $enc [$($done)*
            #[serde(default, skip_serializing_if = "Option::is_none")]
            $(#[$fm])* pub $f: Option<$t>,] $($rest)*);
    };
    (@fields $attrs:tt $name:ident $enc:tt [$($done:tt)*] #[nullable] $(#[$fm:meta])* pub $f:ident : $t:ty, $($rest:tt)*) => {
        $crate::object!(@fields $attrs $name $enc [$($done)*
            #[serde(default, skip_serializing_if = "runtime::Nullable::is_absent")]
            $(#[$fm])* pub $f: $crate::Nullable<$t>,] $($rest)*);
    };
    (@fields $attrs:tt $name:ident $enc:tt [$($done:tt)*] $(#[$fm:meta])* pub $f:ident : $t:ty, $($rest:tt)*) => {
        $crate::object!(@fields $attrs $name $enc [$($done)* $(#[$fm])* pub $f: $t,] $($rest)*);
    };
    (@fields [$(#[$m:meta])*] $name:ident [$($enc:ident)?] [$($done:tt)*]) => {
        $(#[$m])*
        #[serde(remote = "Self")]
        pub struct $name { $($done)* }
        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::deserialize($crate::__private::NoSeq(d))
            }
        }
        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                Self::serialize(self, $crate::object!(@encode s $($enc)?))
            }
        }
    };
    (@encode $s:ident) => { $s };
    (@encode $s:ident UniqueKeys) => { $crate::__private::UniqueKeys($s) };
}

#[cfg(test)]
mod tests {
    #![expect(unreachable_pub, reason = "`object!` writes public items, here in a private module")]

    mod runtime {
        pub use crate::{__private::*, *};
    }

    crate::object! {
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub struct Item {
            pub name: String,
            #[optional]
            /// The count.
            pub count: u8,
            #[maybe]
            pub note: String,
            #[nullable]
            pub owner: String,
            #[serde(rename = "type")]
            pub kind: String,
        }
    }

    #[test]
    fn an_object_writes_each_field_as_its_kind() {
        let item: Item = serde_json::from_str(r#"{"name": "a", "note": null, "owner": null, "type": "x"}"#).unwrap();
        assert_eq!((item.count, item.note.as_deref(), item.owner.clone()), (None, None, crate::Nullable::Null));
        assert_eq!(serde_json::to_string(&item).unwrap(), r#"{"name":"a","owner":null,"type":"x"}"#);
        assert!(serde_json::from_str::<Item>(r#"{"name": "a", "count": null, "type": "x"}"#).is_err(), "null count");
        assert!(serde_json::from_str::<Item>(r#"["a", 1, null, null, "x"]"#).is_err(), "an array is no object");
    }
}
