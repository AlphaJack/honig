//! Exact arithmetic on decimal text: comparison and `multipleOf`. A JSON number is decimal, and `f64` rounds it:
//! `0.07 / 0.01` is not a whole `f64`, and past about `1e15` every `f64` quotient looks whole. Read as text, a value
//! keeps every digit, so `minimum` and `multipleOf` hold exactly.

use std::cmp::Ordering;

/// The base of decimal text.
const DECIMAL_BASE: u128 = 10;
/// [`DECIMAL_BASE`] as a digit is held.
const DIGIT_BASE: u8 = 10;

/// Is `s` made of ASCII digits only? An empty `s` is.
pub(crate) fn all_digits(s: &str) -> bool {
    s.bytes().all(|b| b.is_ascii_digit())
}

/// A decimal number as its significant digits and the power of ten they scale by: `-1.50e3` is `(true, "15", 2)`.
/// Zero has no digits.
struct Parts<'s> {
    negative: bool,
    digits: &'s str,
    exponent: i64,
}

impl<'s> Parts<'s> {
    /// Reads decimal text: an optional `-`, digits with an optional fraction and an optional exponent. `None` for any
    /// other text or an exponent past `i64`.
    fn of(text: &'s str, buf: &'s mut String) -> Option<Self> {
        let (negative, unsigned) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
        let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
        let exponent: i64 = exponent.strip_prefix('+').unwrap_or(exponent).parse().ok()?;
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
            return None;
        }
        buf.clear();
        buf.push_str(whole);
        buf.push_str(fraction);
        let significant = buf.trim_start_matches('0');
        let trimmed = significant.trim_end_matches('0');
        let len = |s: &str| i64::try_from(s.len()).unwrap_or(i64::MAX);
        let exponent = exponent.checked_sub(len(fraction))?.checked_add(len(significant) - len(trimmed))?;
        Some(Self { negative, digits: trimmed, exponent })
    }

    /// Returns the power of ten of the first significant digit: the order of magnitude.
    fn magnitude(&self) -> i64 {
        self.exponent.saturating_add(i64::try_from(self.digits.len()).unwrap_or(i64::MAX) - 1)
    }
}

/// Compares two decimal numbers exactly. `None` when either is not decimal text.
#[must_use]
pub(crate) fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (mut buf_a, mut buf_b) = (String::new(), String::new());
    let (a, b) = (Parts::of(a, &mut buf_a)?, Parts::of(b, &mut buf_b)?);
    let sign = |p: &Parts<'_>| {
        if p.digits.is_empty() {
            0
        } else if p.negative {
            -1
        } else {
            1
        }
    };
    let (sa, sb) = (sign(&a), sign(&b));
    if sa != sb || sa == 0 {
        return Some(sa.cmp(&sb));
    }
    let magnitude = a.magnitude().cmp(&b.magnitude()).then_with(|| {
        // Same order of magnitude: the digits compare from the first, the shorter one padded with zeros.
        let width = a.digits.len().max(b.digits.len());
        format!("{:0<width$}", a.digits).cmp(&format!("{:0<width$}", b.digits))
    });
    Some(if sa < 0 { magnitude.reverse() } else { magnitude })
}

/// Is `value` a multiple of `of`? Both are decimal text. `None` when either is not or when `of` is not positive.
///
/// `value` is `a*10^e` and `of` is `b*10^f`, with `a` and `b` not ending in zeros. The quotient is `(a / b)*10^(e-f)`:
/// a whole number when `a*10^(e-f)` is a multiple of `b`. With `e < f` it would need `a` to end in a zero, so it is not.
#[must_use]
pub(crate) fn is_multiple(value: &str, of: &str) -> Option<bool> {
    let (mut buf_v, mut buf_m) = (String::new(), String::new());
    let (v, m) = (Parts::of(value, &mut buf_v)?, Parts::of(of, &mut buf_m)?);
    if m.negative || m.digits.is_empty() {
        return None;
    }
    if v.digits.is_empty() {
        return Some(true);
    }
    let Some(shift) = v.exponent.checked_sub(m.exponent).filter(|s| *s >= 0) else { return Some(false) };
    let Ok(b) = m.digits.parse::<u64>().map(u128::from) else {
        return Some(divides_long(v.digits, shift.unsigned_abs(), m.digits));
    };
    // `a mod b`, a digit at a time: `r < b < 2^64` keeps `10*r + 9` inside a `u128`.
    let a = v.digits.bytes().fold(0, |r, d| (r * DECIMAL_BASE + u128::from(d - b'0')) % b);
    Some((a * power_of_ten_mod(shift.unsigned_abs(), b)).is_multiple_of(b))
}

/// Does `b` divide `a*10^shift`? `a` and `b` are digit text without leading zeros, `b` of any length. It is long
/// division: one digit of the dividend at a time, the remainder kept below `b` by at most nine subtractions.
///
/// Only the first `4*len(b)` factors of ten can matter: `b` holds fewer factors of 2, and of 5, than that. Past them a
/// factor of ten is coprime to what is left of `b`, so the check ends however large `shift` is.
fn divides_long(a: &str, shift: u64, b: &str) -> bool {
    let b: Vec<u8> = b.bytes().map(|d| d - b'0').collect();
    let zeros = shift.min(FACTORS_PER_DIGIT.saturating_mul(u64::try_from(b.len()).unwrap_or(u64::MAX)));
    let mut r: Vec<u8> = vec![];
    for d in a.bytes().map(|d| d - b'0').chain((0..zeros).map(|_| 0)) {
        if !(r.is_empty() && d == 0) {
            r.push(d);
        }
        while r.len() > b.len() || (r.len() == b.len() && r >= b) {
            subtract(&mut r, &b);
        }
    }
    r.is_empty()
}

/// Subtracts `b` from `r`, both digits with the most significant first and `r >= b`, and drops the leading zeros.
fn subtract(r: &mut Vec<u8>, b: &[u8]) {
    let mut borrow = 0;
    for i in 0..r.len() {
        let at = r.len() - 1 - i;
        let take = b.len().checked_sub(1 + i).map_or(0, |j| b[j]) + borrow;
        (r[at], borrow) = if r[at] >= take { (r[at] - take, 0) } else { (r[at] + DIGIT_BASE - take, 1) };
    }
    let lead = r.iter().take_while(|d| **d == 0).count();
    r.drain(..lead);
}

/// An upper bound of the factors of 2, and of 5, per decimal digit of a number: `10^n < 2^(4n)`.
const FACTORS_PER_DIGIT: u64 = 4;

/// Returns `10^k mod b` by squaring. `b < 2^64`, so every product fits a `u128`.
fn power_of_ten_mod(mut k: u64, b: u128) -> u128 {
    let (mut result, mut base) = (1 % b, DECIMAL_BASE % b);
    while k > 0 {
        if k & 1 == 1 {
            result = result * base % b;
        }
        base = base * base % b;
        k >>= 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimals_compare_exactly() {
        assert_eq!(compare("0.09999999999999999999", "0.1"), Some(Ordering::Less), "past what an f64 tells apart");
        assert_eq!(compare("1.50", "1.5"), Some(Ordering::Equal));
        assert_eq!(compare("-2", "-10"), Some(Ordering::Greater));
        assert_eq!(compare("0", "-0.0"), Some(Ordering::Equal));
        assert_eq!(compare("1e40", "9999999999"), Some(Ordering::Greater));
        assert_eq!(compare("12345678901234567.89", "12345678901234567.88"), Some(Ordering::Greater));
        assert_eq!(compare("x", "1"), None);
    }

    #[test]
    fn multiples_are_exact() {
        for (value, of) in [
            ("0.07", "0.01"),
            ("19.99", "0.01"),
            ("0.3", "0.1"),
            ("1e20", "0.1"),
            ("0", "7"),
            ("5", "2.5"),
            ("1e6144", "3"),
            ("0.5", "0.25"),
        ] {
            assert_eq!(is_multiple(value, of), Some(value != "1e6144"), "{value} of {of}");
        }
        assert_eq!(is_multiple("1e6144", "2"), Some(true));
        for (value, of) in
            [("0.015", "0.01"), ("1", "20"), ("100000000000000001", "10"), ("0.30000000000000004", "0.1")]
        {
            assert_eq!(is_multiple(value, of), Some(false), "{value} of {of}");
        }
        assert_eq!(is_multiple("1", "0"), None);
        assert_eq!(is_multiple("1", "-2"), None);
    }

    /// A `multipleOf` of any length divides exactly: past 19 digits an `f64` fallback took values off by less than a
    /// millionth.
    #[test]
    fn a_long_multiple_of_divides_exactly() {
        let of = "123456789012345678901"; // 21 significant digits
        assert_eq!(is_multiple("246913578024691357802", of), Some(true));
        assert_eq!(is_multiple("246913578024691357803", of), Some(false), "one past twice");
        assert_eq!(is_multiple("1.23456789012345678901e30", of), Some(true), "a power of ten times it");
        assert_eq!(is_multiple("0.000000000000000000002", "0.000000000000000000001"), Some(true));
        assert_eq!(is_multiple("0.0000000000000000000025", "0.000000000000000000001"), Some(false));
        // Factors of ten past what `of` holds of 2 and 5 change nothing: the check ends however large the exponent.
        assert_eq!(is_multiple("7e9000000000000000000", "1024000000000000000000001"), Some(false));
        assert_eq!(is_multiple("1e9000000000000000000", "2048000000000000000000000"), Some(true));
    }
}
