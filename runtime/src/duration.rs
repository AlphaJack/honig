//! `format: duration`: the ISO 8601 durations of RFC 3339 Appendix A.

/// The units of the date part, longest first: `P1Y2M3D`.
const DATE_UNITS: [u8; 3] = *b"YMD";
/// The units of the time part, longest first: `T4H5M6S`.
const TIME_UNITS: [u8; 3] = *b"HMS";

/// Is `text` a duration as RFC 3339 Appendix A writes it? `P`, then weeks alone (`P2W`) or a date part and a time
/// part after `T`. Each part has at least one element. Its units follow each other without a gap (`P1Y2M`, not
/// `P1Y2D`). Numbers are whole and the letters may be either case, as ABNF strings are.
pub(crate) fn is_duration(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    let Some(rest) = upper.strip_prefix('P') else { return false };
    if let Some(weeks) = rest.strip_suffix('W') {
        return whole(weeks);
    }
    let (date, time) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };
    let date_ok = run(date, &DATE_UNITS).is_some();
    match time {
        // `T` needs an element after it.
        Some(time) => date_ok && run(time, &TIME_UNITS).is_some_and(|n| n > 0),
        None => date_ok && !date.is_empty(),
    }
}

/// Returns the number of elements in `text` when each is digits and a unit of `units`, and the units are consecutive.
fn run(text: &str, units: &[u8]) -> Option<usize> {
    let mut last: Option<usize> = None;
    let mut count = 0;
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !c.is_ascii_digit())?;
        let (number, tail) = rest.split_at(end);
        let unit = tail.bytes().next()?;
        let at = units.iter().position(|u| *u == unit)?;
        if number.is_empty() || last.is_some_and(|l| at != l + 1) {
            return None;
        }
        last = Some(at);
        count += 1;
        rest = tail.get(1..)?;
    }
    Some(count)
}

/// Returns one or more ASCII digits.
fn whole(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_duration_checks_as_rfc_3339_appendix_a_writes_it() {
        for good in [
            "P4Y",
            "PT0S",
            "P0D",
            "P1M",
            "PT1M",
            "PT36H",
            "P1DT12H",
            "P2W",
            "P1Y2M3DT4H5M6S",
            "P1YT1H",
            "PT1H30M",
            "p1d",
            "P1M2D",
        ] {
            assert!(is_duration(good), "{good}");
        }
        for bad in [
            "",
            "P",
            "PT",
            "P1",
            "P1D2H",
            "P2S",
            "PT1D",
            "P1Y2W",
            "P1Y1D",
            "PT1H1S",
            "P1.5D",
            "PT1.5S",
            "-P1D",
            "P1DT",
            "P2W1D",
            "1D",
            "P1\u{09ea}D",
            "P1M1Y",
            "P1D ",
        ] {
            assert!(!is_duration(bad), "{bad}");
        }
    }
}
