//! Parameter styles: the styles and shapes. These are in their own modules: percent-encoding, splitting into pairs,
//! the text of a value and the encoding and decoding of each value in each style.

mod decode;
mod encode;
mod keys;
mod pairs;
mod percent;

pub use decode::*;
pub use encode::*;
pub use pairs::*;
pub use percent::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Parameter serialization style (`style` in the description).
pub enum Style {
    /// `simple`: comma separated.
    Simple,
    /// `label`: dot prefixed.
    Label,
    /// `matrix`: semicolon prefixed `;name=value`.
    Matrix,
    /// `form`: `name=value` pairs.
    Form,
    /// `spaceDelimited`: space separated.
    SpaceDelimited,
    /// Swagger 2.0 `collectionFormat: tsv`: tab separated.
    TabDelimited,
    /// `pipeDelimited`: pipe separated.
    PipeDelimited,
    /// `deepObject`: `name[key]=value`.
    DeepObject,
    /// `cookie` (3.2): the value is sent as is.
    Cookie,
}

impl Style {
    /// Returns the character that this style splits a list on: the one that an item must not hold.
    const fn separator(self) -> Option<char> {
        match self {
            Self::SpaceDelimited => Some(' '),
            Self::PipeDelimited => Some('|'),
            Self::TabDelimited => Some('\t'),
            Self::Simple | Self::Label | Self::Matrix | Self::Form | Self::DeepObject | Self::Cookie => None,
        }
    }

    /// Returns the percent-encoded delimiter of `spaceDelimited`, `pipeDelimited` and `tsv` values. `|` is
    /// percent-encoded like the space (RFC 3986).
    /// Returns what separates the items of an array in a header: a comma, or the character of a delimited style.
    const fn header_separator(self) -> char {
        match self {
            Self::SpaceDelimited => ' ',
            Self::PipeDelimited => '|',
            Self::TabDelimited => '\t',
            Self::Simple | Self::Label | Self::Matrix | Self::Form | Self::DeepObject | Self::Cookie => ',',
        }
    }

    const fn delimiter(self) -> Option<&'static str> {
        match self {
            Self::SpaceDelimited => Some("%20"),
            Self::PipeDelimited => Some("%7C"),
            Self::TabDelimited => Some("%09"),
            Self::Simple | Self::Label | Self::Matrix | Self::Form | Self::DeepObject | Self::Cookie => None,
        }
    }
}

/// How a parameter goes on the wire: `style`, `explode` and `allowReserved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enc {
    /// `style`.
    pub style: Style,
    /// `explode`: one pair per array item or object member.
    pub explode: bool,
    /// `allowReserved`: RFC 3986 reserved characters go unescaped.
    pub allow_reserved: bool,
}

impl Enc {
    /// `form` exploded. The default for query parameters and cookies.
    pub const FORM: Self = Self { style: Style::Form, explode: true, allow_reserved: false };
    /// `simple` not exploded. The default for path parameters and headers.
    pub const SIMPLE: Self = Self { style: Style::Simple, explode: false, allow_reserved: false };
}

/// Wire shape of a parameter value, needed to parse it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A primitive value.
    Primitive,
    /// An array.
    Array,
    /// An object.
    Object,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn query(q: &str, style: Style) -> Option<Value> {
        decode_query(&parse_query(q), "c", "/query/c", style, Shape::Array, false).expect("one value")
    }

    #[test]
    fn empty_items_inside_a_list_are_kept() {
        assert_eq!(query("c=a,,b", Style::Form), Some(serde_json::json!(["a", "", "b"])));
        assert_eq!(query("c=a%7C%7Cb", Style::PipeDelimited), Some(serde_json::json!(["a", "", "b"])));
        assert_eq!(query("c=a,", Style::Form), Some(serde_json::json!(["a", ""])));
    }

    #[test]
    fn an_empty_value_is_the_empty_list() {
        assert_eq!(query("c=", Style::Form), Some(serde_json::json!([])));
        assert_eq!(
            decode_header(Some(""), "/header/h", Style::Simple, Shape::Array, false),
            Ok(Some(serde_json::json!([])))
        );
    }

    /// `allowReserved` keeps reserved characters in a path value but never `/`, `?` or `#`.
    #[test]
    fn a_reserved_path_value_still_encodes_slash_query_and_fragment() {
        let reserved = Enc { style: Style::Simple, explode: false, allow_reserved: true };
        assert_eq!(encode_path("p", "a/b?c#d:e@f", reserved).unwrap(), "a%2Fb%3Fc%23d:e@f");
        assert_eq!(encode_path("p", &["x/y", "z"], reserved).unwrap(), "x%2Fy,z");
        let plain = Enc { allow_reserved: false, ..reserved };
        assert_eq!(encode_path("p", "a:b", plain).unwrap(), "a%3Ab");
    }

    /// 3.2.1 Style Examples, `undefined` column. Label and matrix expand to nothing.
    #[test]
    fn an_undefined_label_or_matrix_value_expands_to_nothing() {
        for style in [Style::Label, Style::Matrix] {
            for explode in [false, true] {
                let enc = Enc { style, explode, allow_reserved: false };
                assert_eq!(encode_path("color", &None::<String>, enc).unwrap(), "", "{style:?} explode={explode}");
            }
        }
        let label = Enc { style: Style::Label, explode: false, allow_reserved: false };
        assert_eq!(encode_path("color", "blue", label).unwrap(), ".blue");
    }
}
