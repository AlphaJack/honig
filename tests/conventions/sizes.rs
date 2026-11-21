//! No source file of the generator or the runtime outside the tests runs past 1500 lines: a longer one splits by kind.

/// The longest file allowed.
const MAX_LINES: usize = 1500;

#[test]
fn no_source_file_is_over_1500_lines() {
    let long: Vec<String> = super::all_sources()
        .into_iter()
        .filter(|(file, text)| !super::is_test_file(file) && text.lines().count() > MAX_LINES)
        .map(|(file, text)| format!("{file}: {} lines", text.lines().count()))
        .collect();
    assert!(long.is_empty(), "split these by kind: {long:#?}");
}
