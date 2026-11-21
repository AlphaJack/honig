//! Base64, standard and URL-safe. The `base64` crate does the work. With the `simd` feature `base64-simd` does the
//! standard alphabet.

use ::base64::Engine as _;
use ::base64::alphabet;
use ::base64::engine::GeneralPurpose;
use ::base64::engine::general_purpose::NO_PAD;

#[cfg(feature = "simd")]
mod standard {
    /// Returns standard base64 with padding.
    pub(super) fn encode(input: &[u8]) -> String {
        base64_simd::STANDARD.encode_to_string(input)
    }

    /// Appends the bytes of `text` with or without padding to `out`. `None` on an invalid character.
    pub(super) fn decode_append(text: &[u8], out: &mut Vec<u8>) -> Option<()> {
        let (unpadded, _) = text.split_at(text.iter().rposition(|c| *c != b'=').map_or(0, |last| last + 1));
        base64_simd::STANDARD_NO_PAD.decode_append(unpadded, out).ok()
    }
}

#[cfg(not(feature = "simd"))]
mod standard {
    use ::base64::Engine as _;
    use ::base64::alphabet;
    use ::base64::engine::GeneralPurpose;
    use ::base64::engine::general_purpose::PAD_INDIFFERENT;

    /// Writes padding and reads text with or without it. The bits a last character holds past the data are ignored, as
    /// RFC 4648 section 3.5 allows a decoder.
    const ENGINE: GeneralPurpose =
        GeneralPurpose::new(&alphabet::STANDARD, PAD_INDIFFERENT.with_decode_allow_trailing_bits(true));

    /// Returns standard base64 with padding.
    pub(super) fn encode(input: &[u8]) -> String {
        ENGINE.encode(input)
    }

    /// Appends the bytes of `text` with or without padding to `out`. `None` on an invalid character.
    pub(super) fn decode_append(text: &[u8], out: &mut Vec<u8>) -> Option<()> {
        ENGINE.decode_vec(text, out).ok()
    }
}

/// PKCE sends 32 bytes. SIMD gains nothing there.
const URL_SAFE: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, NO_PAD);

/// Returns standard base64 with padding.
pub fn base64(input: &[u8]) -> String {
    standard::encode(input)
}

/// Decodes standard base64 with or without padding. `None` on an invalid character.
pub fn unbase64(input: &str) -> Option<Vec<u8>> {
    let mut out = vec![];
    standard::decode_append(input.as_bytes(), &mut out).map(|()| out)
}

/// Four characters make one group of three bytes.
#[cfg(feature = "multipart")]
const GROUP_CHARS: usize = 4;
#[cfg(feature = "multipart")]
const GROUP_BYTES: usize = 3;

/// Decodes base64 given in pieces, as if they were one text. Each piece is decoded where it is while the pieces end on
/// whole groups. A MIME line of 76 does. A group that spans two pieces has the rest joined first.
#[cfg(feature = "multipart")]
fn decode_pieces<'a>(pieces: impl Iterator<Item = &'a [u8]>, capacity: usize) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(capacity);
    let mut pieces = pieces.peekable();
    while let Some(piece) = pieces.next() {
        if !piece.len().is_multiple_of(GROUP_CHARS) && pieces.peek().is_some() {
            let mut rest = piece.to_vec();
            for p in pieces {
                rest.extend_from_slice(p);
            }
            standard::decode_append(&rest, &mut out)?;
            break;
        }
        standard::decode_append(piece, &mut out)?;
    }
    Some(out)
}

/// Decodes base64 that MIME broke into lines (RFC 2045 section 6.8). Whitespace anywhere is skipped. `None` on any
/// other character that is not base64.
#[cfg(feature = "multipart")]
pub(crate) fn unbase64_mime(text: &[u8]) -> Option<Vec<u8>> {
    let capacity = text.len() / GROUP_CHARS * GROUP_BYTES;
    // Line breaks are found with `memchr`. A space or a tab inside a line is not base64, so such a text goes again
    // split at every kind of whitespace.
    decode_pieces(lines(text), capacity)
        .or_else(|| decode_pieces(text.split(u8::is_ascii_whitespace).filter(|p| !p.is_empty()), capacity))
}

/// Returns the non-empty pieces of `text` between `\r` and `\n`.
#[cfg(feature = "multipart")]
fn lines(text: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = text;
    std::iter::from_fn(move || {
        loop {
            if rest.is_empty() {
                return None;
            }
            let end = memchr::memchr2(b'\r', b'\n', rest).unwrap_or(rest.len());
            let (line, after) = rest.split_at(end);
            rest = after.get(1..).unwrap_or_default();
            if !line.is_empty() {
                return Some(line);
            }
        }
    })
}

/// Returns URL-safe base64 without padding (RFC 4648 section 5) for PKCE.
pub(crate) fn base64url(bytes: &[u8]) -> String {
    URL_SAFE.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_with_or_without_padding_at_every_length() {
        assert_eq!(unbase64("aGVsbG8="), Some(b"hello".to_vec()));
        assert_eq!(unbase64("aGVsbG8"), Some(b"hello".to_vec()));
        assert_eq!(unbase64("aGV*bG8="), None);
        for len in 0..=64_u8 {
            let data: Vec<u8> = (0..len).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(unbase64(&base64(&data)), Some(data.clone()), "{len} bytes");
            assert_eq!(unbase64(base64(&data).trim_end_matches('=')), Some(data), "{len} bytes unpadded");
        }
    }

    /// MIME breaks base64 into lines. A line need not end on a whole group, and stray spaces or tabs are skipped too.
    #[cfg(feature = "multipart")]
    #[test]
    fn mime_base64_skips_whitespace_anywhere() {
        let data: Vec<u8> = (0..=200_u8).collect();
        let text = base64(&data);
        for width in [4, 5, 7, 76] {
            let wrapped: Vec<u8> = text.as_bytes().chunks(width).flat_map(|l| [l, b"\r\n"].concat()).collect();
            assert_eq!(unbase64_mime(&wrapped), Some(data.clone()), "lines of {width}");
        }
        assert_eq!(unbase64_mime(b"aGVs bG8=\n"), Some(b"hello".to_vec()));
        assert_eq!(unbase64_mime(b"\taGVs\nbG8\n\n"), Some(b"hello".to_vec()));
        assert_eq!(unbase64_mime(b"aGVs\r\nb*8="), None);
        assert_eq!(unbase64_mime(b""), Some(vec![]));
    }

    #[test]
    fn an_equals_sign_inside_the_text_is_refused() {
        assert_eq!(unbase64("aGVs=bG8="), None);
    }

    #[cfg(not(feature = "simd"))]
    #[test]
    fn bits_left_over_in_the_last_character_are_ignored() {
        assert_eq!(unbase64("aGVsbG9="), Some(b"hello".to_vec()));
    }

    #[test]
    fn pkce_uses_the_url_safe_alphabet_without_padding() {
        assert_eq!(base64url(&[0xfb, 0xff]), "-_8");
    }
}
