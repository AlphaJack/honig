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

/// Returns the count `n` as an `i64`, `i64::MAX` past it: a count of digits to add to an exponent.
pub(crate) fn signed(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// A decimal number as its significant digits and the power of ten they scale by: `-1.50e3` is `(true, "15", 2)`.
/// The digits are `head` then `tail`, two slices of the text, so reading them copies nothing. Zero has no digits.
struct Parts<'s> {
    negative: bool,
    head: &'s str,
    tail: &'s str,
    exponent: i64,
}

impl<'s> Parts<'s> {
    /// Reads decimal text: an optional `-`, digits with an optional fraction and an optional exponent. `None` for any
    /// other text or an exponent past `i64`.
    fn of(text: &'s str) -> Option<Self> {
        let (negative, unsigned) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
        let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
        let exponent: i64 = exponent.strip_prefix('+').unwrap_or(exponent).parse().ok()?;
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty() || !all_digits(whole) || !all_digits(fraction) {
            return None;
        }
        let whole_significant = whole.trim_start_matches('0');
        let (head, tail) = if whole_significant.is_empty() {
            ("", fraction.trim_start_matches('0'))
        } else {
            (whole_significant, fraction)
        };
        let tail_trimmed = tail.trim_end_matches('0');
        let (head, tail, trailing_zeros) = if tail_trimmed.is_empty() {
            let head_trimmed = head.trim_end_matches('0');
            (head_trimmed, "", tail.len() + head.len() - head_trimmed.len())
        } else {
            (head, tail_trimmed, tail.len() - tail_trimmed.len())
        };
        let exponent = exponent.checked_sub(signed(fraction.len()))?.checked_add(signed(trailing_zeros))?;
        Some(Self { negative, head, tail, exponent })
    }

    /// Returns the significant digits as numbers, the first first.
    fn digits(&self) -> impl Iterator<Item = u8> + use<'s> {
        self.head.bytes().chain(self.tail.bytes()).map(|d| d - b'0')
    }

    /// Returns how many significant digits there are.
    fn len(&self) -> usize {
        self.head.len() + self.tail.len()
    }

    /// Returns the significant digits as one number. `None` past `u64`.
    fn coefficient(&self) -> Option<u64> {
        self.digits().try_fold(0_u64, |c, d| c.checked_mul(u64::from(DIGIT_BASE))?.checked_add(u64::from(d)))
    }

    /// Returns the power of ten of the first significant digit: the order of magnitude.
    fn magnitude(&self) -> i64 {
        self.exponent.saturating_add(signed(self.len()) - 1)
    }
}

/// Compares two decimal numbers exactly. `None` when either is not decimal text.
#[must_use]
pub(crate) fn compare(a: &str, b: &str) -> Option<Ordering> {
    let (a, b) = (Parts::of(a)?, Parts::of(b)?);
    let sign = |p: &Parts<'_>| {
        if p.len() == 0 {
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
        let width = a.len().max(b.len());
        let (da, db) = (a.digits().chain(std::iter::repeat(0)), b.digits().chain(std::iter::repeat(0)));
        da.take(width).cmp(db.take(width))
    });
    Some(if sa < 0 { magnitude.reverse() } else { magnitude })
}

/// Is `value` a multiple of `of`? Both are decimal text. `None` when either is not or when `of` is not positive.
///
/// `value` is `a*10^e` and `of` is `b*10^f`, with `a` and `b` not ending in zeros. The quotient is `(a / b)*10^(e-f)`:
/// a whole number when `a*10^(e-f)` is a multiple of `b`. With `e < f` it would need `a` to end in a zero, so it is not.
#[must_use]
pub fn is_multiple(value: &str, of: &str) -> Option<bool> {
    let (v, m) = (Parts::of(value)?, Parts::of(of)?);
    if m.negative || m.len() == 0 {
        return None;
    }
    if v.len() == 0 {
        return Some(true);
    }
    let Some(shift) = v.exponent.checked_sub(m.exponent).filter(|s| *s >= 0) else { return Some(false) };
    let Some(b) = m.coefficient().map(u128::from) else {
        return Some(divides_long(v.digits(), shift.unsigned_abs(), &m.digits().collect::<Vec<_>>()));
    };
    // `a mod b`, a digit at a time: `r < b < 2^64` keeps `10*r + 9` inside a `u128`.
    let a = v.digits().fold(0, |r, d| (r * DECIMAL_BASE + u128::from(d)) % b);
    Some((a * power_of_ten_mod(shift.unsigned_abs(), b)).is_multiple_of(b))
}

/// How many powers of ten an `f64` holds exactly: `10^0` to `10^22`.
const EXACT_POWERS: usize = 23;
/// `10^0` to `10^22`, each exact.
const POWERS: [f64; EXACT_POWERS] = {
    let mut powers = [1.0; EXACT_POWERS];
    let mut i = 1;
    while i < EXACT_POWERS {
        powers[i] = powers[i - 1] * 10.0;
        i += 1;
    }
    powers
};
/// `2^52`: below it, the gap between two `f64`s near `n*10^-k` is less than `10^-k`. So at most one multiple of
/// `10^-k` reads as a given `f64`, and the shortest text of that `f64` is it.
const UNIQUE: f64 = 4_503_599_627_370_496.0;

/// Returns `of` as `c*10^e`: its significant digits as a `u64` and its exponent. `None` when `of` is not a positive
/// decimal or has more than 19 significant digits.
fn coefficient(of: &str) -> Option<(u64, i64)> {
    let m = Parts::of(of)?;
    if m.negative || m.len() == 0 {
        return None;
    }
    Some((m.coefficient()?, m.exponent))
}

/// Is the shortest text of `value` a multiple of `of`, as [`is_multiple`] reads them? Decided without that text when
/// `value` is `n*10^-k` for a whole `n` below [`UNIQUE`], where `10^-k` is the unit of the last digit of `of`. `None`
/// otherwise: the text decides those.
///
/// `n / 10^k` divides two exact `f64`s, so it rounds once, as reading the text `n*10^-k` does. When it gives `value`
/// back, that text reads as `value`, and below [`UNIQUE`] no other multiple of `10^-k` does.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "`n` is a whole number below 2^52 by the check before the cast"
)]
#[expect(clippy::float_cmp, reason = "the same `f64` exactly, as reading the text gives it")]
pub(crate) fn f64_is_multiple(value: f64, of: &str) -> Option<bool> {
    let (c, e) = coefficient(of)?;
    let k = usize::try_from(e.min(0).unsigned_abs()).ok()?;
    let scale = *POWERS.get(k)?;
    let n = (value * scale).round();
    if n.abs() >= UNIQUE || n / scale != value {
        return None;
    }
    let n = n.abs() as u64;
    if e > 0 {
        let unit = u64::from(DIGIT_BASE).checked_pow(u32::try_from(e).ok()?)?;
        return Some(n.is_multiple_of(c.checked_mul(unit)?));
    }
    Some(n.is_multiple_of(c))
}

/// Is the whole number `value` a multiple of `of`? `None` when the numbers do not fit a `u128`: the text decides those.
#[must_use]
pub(crate) fn int_is_multiple(value: i128, of: &str) -> Option<bool> {
    let (c, e) = coefficient(of)?;
    let power = u128::from(DIGIT_BASE).checked_pow(u32::try_from(e.unsigned_abs()).ok()?)?;
    let v = value.unsigned_abs();
    if e >= 0 {
        Some(v.is_multiple_of(u128::from(c).checked_mul(power)?))
    } else {
        Some(v.checked_mul(power)?.is_multiple_of(u128::from(c)))
    }
}

/// Does `b` divide `a*10^shift`? `a` and `b` are digits without leading zeros, `b` of any length. It is long
/// division: one digit of the dividend at a time, the remainder kept below `b` by at most nine subtractions.
///
/// Only the first `4*len(b)` factors of ten can matter: `b` holds fewer factors of 2, and of 5, than that. Past them a
/// factor of ten is coprime to what is left of `b`, so the check ends however large `shift` is.
fn divides_long(a: impl Iterator<Item = u8>, shift: u64, b: &[u8]) -> bool {
    let zeros = shift.min(FACTORS_PER_DIGIT.saturating_mul(u64::try_from(b.len()).unwrap_or(u64::MAX)));
    let mut r: Vec<u8> = vec![];
    for d in a.chain((0..zeros).map(|_| 0)) {
        if !(r.is_empty() && d == 0) {
            r.push(d);
        }
        while r.len() > b.len() || (r.len() == b.len() && r.as_slice() >= b) {
            subtract(&mut r, b);
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

    /// A `multipleOf` of any length divides exactly.
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

    /// The values the equivalence tests read: random bit patterns, decimals `m/10^k` with `m` up to `2^53`, the
    /// neighbours of `2^52/10^k` where the fast path stops, cents, ties and the edges of `f64`.
    #[expect(clippy::cast_precision_loss, reason = "53 bits fit an `f64` exactly")]
    fn values() -> Vec<f64> {
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut values = vec![0.0, -0.0, 5e-324, f64::MIN_POSITIVE, f64::MAX, 0.1 + 0.2, 19.995, 1e22, 1e23];
        for _ in 0..40_000 {
            values.push(f64::from_bits(next()));
            let k = usize::try_from(next() % 24).unwrap_or(0);
            let m = (next() >> 11) as f64;
            values.push(m / 10_f64.powi(i32::try_from(k).unwrap_or(0)));
        }
        for power in POWERS {
            let edge = UNIQUE / power;
            let mut v = edge;
            for _ in 0..50 {
                values.extend([v, -v]);
                v = v.next_down();
            }
            v = edge;
            for _ in 0..50 {
                values.push(v);
                v = v.next_up();
            }
        }
        for cents in -20_000..20_000 {
            values.extend([f64::from(cents) / 100.0, f64::from(cents) / 1000.0, f64::from(cents) + 0.5]);
        }
        values.retain(|v| v.is_finite());
        values
    }

    const OFS: [&str; 14] =
        ["0.01", "0.1", "0.5", "0.25", "1", "5", "100", "0.001", "3", "0.03", "1e-22", "1e-23", "7e5", "0.0000001"];

    /// Where the fast path answers, it answers as the text does.
    #[test]
    fn a_float_multiple_is_what_its_text_says() {
        let mut decided = 0;
        for v in values() {
            for of in OFS {
                if let Some(fast) = f64_is_multiple(v, of) {
                    decided += 1;
                    assert_eq!(Some(fast), is_multiple(&format!("{v:e}"), of), "{v:e} of {of}");
                }
            }
        }
        assert!(decided > 200_000, "the fast path decided {decided}");
    }

    #[test]
    fn a_whole_multiple_is_what_its_text_says() {
        let edges = [i128::from(i64::MIN), i128::from(i64::MAX), i128::from(u64::MAX), 0, -1, 1];
        for v in (-5000..5000).chain(edges) {
            for of in OFS {
                if let Some(fast) = int_is_multiple(v, of) {
                    assert_eq!(Some(fast), is_multiple(&v.to_string(), of), "{v} of {of}");
                }
            }
        }
    }

    /// The values that an API sends most, prices and counts, take the fast path.
    #[test]
    fn a_price_is_decided_without_its_text() {
        assert_eq!(f64_is_multiple(19.99, "0.01"), Some(true));
        assert_eq!(f64_is_multiple(2.5, "0.5"), Some(true));
        assert_eq!(f64_is_multiple(0.3, "0.1"), Some(true));
        assert_eq!(f64_is_multiple(15.0, "5"), Some(true));
        assert_eq!(f64_is_multiple(16.0, "5"), Some(false));
        assert_eq!(f64_is_multiple(19.995, "0.01"), None, "the text decides a value with more digits than `of`");
        assert_eq!(int_is_multiple(30, "0.5"), Some(true));
    }
}
