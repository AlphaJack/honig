//! The name that `serde_json` gives the struct of a raw value (`RawValue`). The JSON engines and the decoders that
//! pass a raw value through untouched all compare against it.

/// The struct name of `serde_json::value::RawValue`. `Decimal` writes itself as one.
pub(crate) const RAW_VALUE: &str = "$serde_json::private::RawValue";
