//! What reaches the system says so where it does. That is a function at a time and not a whole module.

/// Whether `source` expects `clippy::disallowed_methods` for a whole module other than an inline `mod tests`.
fn module_wide(source: &str) -> bool {
    let mut in_tests = false;
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        if trimmed.starts_with("mod ") && trimmed.ends_with('{') {
            in_tests = trimmed == "mod tests {";
        }
        let inner = trimmed.starts_with("#![expect(") || trimmed.starts_with("#![allow(");
        inner && trimmed.contains("clippy::disallowed_methods") && !(in_tests && line.starts_with(' '))
    })
}

#[test]
fn the_system_is_reached_one_function_at_a_time() {
    let module_wide: Vec<String> = super::all_sources()
        .into_iter()
        .filter(|(file, source)| !super::is_test_file(file) && module_wide(source))
        .map(|(file, _)| file)
        .collect();
    assert!(
        module_wide.is_empty(),
        "a module-wide expect of clippy::disallowed_methods, which belongs on each function that reaches the \
         system:\n{}",
        module_wide.join("\n")
    );
}
