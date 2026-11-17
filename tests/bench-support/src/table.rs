//! A table printed a row at a time, as each row is measured.
//!
//! Every column is padded to its width, text to the left and numbers to the right, so a terminal and a CI log line it
//! up. Tabs line up only when every value fits one tab stop.

/// How a column lines up its values.
#[derive(Debug, Clone, Copy)]
pub enum Align {
    /// Text: names.
    Left,
    /// Numbers and sizes.
    Right,
}

/// One column: its header, how wide its widest value is and how it lines up.
#[derive(Debug, Clone, Copy)]
pub struct Column {
    header: &'static str,
    width: usize,
    align: Align,
}

impl Column {
    /// A column of text as wide as `widest`.
    #[must_use]
    pub fn left(header: &'static str, widest: usize) -> Self {
        Self { header, width: widest, align: Align::Left }
    }

    /// A column of numbers as wide as `sample`, such as `"99999.9"`.
    #[must_use]
    pub fn right(header: &'static str, sample: &str) -> Self {
        Self { header, width: sample.chars().count(), align: Align::Right }
    }
}

/// A table whose header is printed. [`Table::row`] prints each row.
#[derive(Debug)]
pub struct Table(Vec<Column>);

impl Table {
    /// Prints the header. A column is as wide as its header or its widest value, whichever is wider.
    #[must_use]
    pub fn new(columns: &[Column]) -> Self {
        let widened = columns.iter().map(|c| Column { width: c.width.max(c.header.chars().count()), ..*c }).collect();
        let table = Self(widened);
        table.row(&table.0.iter().map(|c| c.header.to_owned()).collect::<Vec<_>>());
        table
    }

    /// Prints one row: a cell per column, in order.
    pub fn row(&self, cells: &[String]) {
        let padded: Vec<String> = self
            .0
            .iter()
            .zip(cells)
            .map(|(c, cell)| match c.align {
                Align::Left => format!("{cell:<width$}", width = c.width),
                Align::Right => format!("{cell:>width$}", width = c.width),
            })
            .collect();
        println!("{}", padded.join("  ").trim_end());
    }
}

/// The units of [`size`], each 1024 times the one before.
const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
/// The step between two [`UNITS`].
const STEP: usize = 1024;

/// Returns `bytes` in the largest unit that holds it whole: `256 B`, `16 KiB`, `1 MiB`.
#[must_use]
pub fn size(bytes: usize) -> String {
    let mut n = bytes;
    let mut unit = UNITS.iter();
    let mut name = unit.next().copied().unwrap_or_default();
    while n >= STEP && n.is_multiple_of(STEP) {
        let Some(next) = unit.next() else { break };
        n /= STEP;
        name = next;
    }
    format!("{n} {name}")
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn a_size_takes_the_largest_unit_that_holds_it_whole() {
        assert_eq!(size(256), "256 B");
        assert_eq!(size(16 << 10), "16 KiB");
        assert_eq!(size(1 << 20), "1 MiB");
        assert_eq!(size(1536), "1536 B");
    }
}
