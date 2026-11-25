//! Hexadecimal digits and percent escapes.

/// The radix of a hexadecimal digit as in a percent escape or a character reference.
#[cfg(any(feature = "server", feature = "multipart", all(feature = "tracing", feature = "tracing-opentelemetry")))]
pub(crate) const HEX: u32 = 16;

/// Returns the value of a hexadecimal digit byte as in a percent escape.
#[cfg(any(feature = "server", feature = "multipart"))]
pub(crate) fn hex_digit(b: u8) -> Option<u32> {
    char::from(b).to_digit(HEX)
}

/// A percent escape's length: `%` and two hexadecimal digits.
#[cfg(any(feature = "server", feature = "multipart"))]
pub(crate) const PCT_ESCAPE: usize = "%XX".len();
