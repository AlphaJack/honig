//! The reference pages of the book under `docs/book/src/generated` come from the code. So they cannot drift from it.
//! A stale page is rewritten and the test fails. Commit the new page.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use toml_edit::DocumentMut;

#[test]
fn generated_pages_are_fresh() {
    let out = out_dir();
    let stale: Vec<&str> = pages()
        .unwrap()
        .into_iter()
        .map(|(name, from, text)| (name, with_source(name, &from, text)))
        .filter(|(name, text)| read(&out.join(name)).ok().as_ref() != Some(text))
        .map(|(name, text)| {
            fs::write(out.join(name), text).unwrap();
            name
        })
        .collect();
    assert!(stale.is_empty(), "rewrote stale pages in docs/book/src/generated: {stale:?}");
}

/// A Markdown page starts with where it comes from, so an edit goes to the source. A text page goes into a code
/// block as it is.
fn with_source(name: &str, from: &Path, text: String) -> String {
    if !Path::new(name).extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
        return text;
    }
    let from = from.strip_prefix(super::root()).unwrap_or(from);
    let from: Vec<String> = from.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    format!(
        "<!-- Generated from {} by src/tests/docs_pages.rs. Edit the source, not this file. -->\n\n{text}",
        from.join("/")
    )
}

fn out_dir() -> PathBuf {
    super::root().join("docs").join("book").join("src").join("generated")
}

/// Each page's file name and text:
///
/// - `cli-help.txt`: the command's `--help`
/// - `server-options.md`: the rustdoc of the `Server` builder of the runtime with one section per method
/// - `features.md`: the cargo features of the runtime from the `##` comments of its `Cargo.toml`
/// - `server-steps.md`, `ranges.md`: the module docs of the server and of its range requests
/// - `reconnect.md`: the rustdoc of `ItemStream::reconnecting`
/// - `errors.md`, `error-fields.md`: the variants of `ClientError` and of `ProblemMember` with their rustdoc
/// - `error-rendering.md`: the rustdoc of `Rejection::response`
/// - `settings.md`: every setting of a module from `SETTINGS`
/// - `formats.md`: the Rust type of each string format as the generator picks it
/// - `checked-text.md`: the runtime's text types that only a valid value of their format can be
/// - `module-files.md`: the files of a generated module from `emit::rust::FILES`
/// - `principals.md`, `operation-layer.md`: the rustdoc of `Verified` and of `Server::operation_layer`
/// - `upgrade-v20.md`, `upgrade-v30.md`, `upgrade-v31.md`, `upgrade-any.md`: the module docs of the upgrade passes
fn pages() -> Result<Vec<(&'static str, PathBuf, String)>> {
    let root = super::root();
    let src = root.join("runtime").join("src");
    let (service_rs, range_rs, reconnect_rs) = (
        src.join("server").join("service.rs"),
        src.join("server").join("range.rs"),
        src.join("client").join("reconnect.rs"),
    );
    let (error_rs, reject_rs) = (src.join("error.rs"), src.join("reject.rs"));
    let auth_rs = src.join("server").join("auth.rs");
    let (formats_rs, formatted_rs) = (src.join("formats.rs"), src.join("formatted.rs"));
    let runtime_toml = root.join("runtime").join("Cargo.toml");
    let upgrade = root.join("src").join("upgrade");
    let (v20, v30, v31, any) = (
        upgrade.join("v20_to_v30.rs"),
        upgrade.join("v30_to_v31.rs"),
        upgrade.join("v31_to_v32.rs"),
        root.join("src").join("upgrade.rs"),
    );
    let kitchen_sink = root.join("tests").join("e2e").join("src").join("v32_kitchen_sink").join("api");
    let service = read(&service_rs)?;
    let reject = read(&reject_rs)?;
    // Layers and Adapters show the methods that have a page of their own.
    let server = methods_page(
        &service,
        "pub struct Server",
        "impl<O: Operations, L> Server<O, L> {",
        &["operation_layer", "into_service", "into_axum"],
    )?;
    Ok(vec![
        ("cli-help.txt", root.join("src").join("cli.rs"), crate::cli::usage()),
        ("server-options.md", service_rs.clone(), plain_links(&server)),
        ("features.md", runtime_toml.clone(), features_page(&read(&runtime_toml)?, turned_on)?),
        ("server-steps.md", service_rs.clone(), plain_links(&module_docs(&service))),
        ("ranges.md", range_rs.clone(), plain_links(&module_docs(&read(&range_rs)?))),
        (
            "reconnect.md",
            reconnect_rs.clone(),
            plain_links(&item_docs(&read(&reconnect_rs)?, "    pub fn reconnecting")?),
        ),
        ("errors.md", error_rs.clone(), plain_links(&variants_page(&read(&error_rs)?, "pub enum Error")?)),
        ("error-fields.md", reject_rs.clone(), plain_links(&variants_page(&reject, "pub enum ProblemMember")?)),
        ("error-rendering.md", reject_rs, plain_links(&item_docs(&reject, "    pub fn response")?)),
        ("formats.md", root.join("src").join("emit").join("rust").join("syntax.rs"), formats_page()),
        ("settings.md", root.join("src").join("settings.rs"), settings_page()),
        (
            "checked-text.md",
            formats_rs.clone(),
            plain_links(&checked_text_page(&read(&formats_rs)?, &read(&formatted_rs)?)),
        ),
        ("principals.md", auth_rs.clone(), plain_links(&item_docs(&read(&auth_rs)?, "pub struct Verified")?)),
        ("operation-layer.md", service_rs, plain_links(&item_docs(&service, "    pub fn operation_layer")?)),
        ("upgrade-v20.md", v20.clone(), module_docs(&read(&v20)?)),
        ("upgrade-v30.md", v30.clone(), module_docs(&read(&v30)?)),
        ("upgrade-v31.md", v31.clone(), module_docs(&read(&v31)?)),
        ("upgrade-any.md", any.clone(), module_docs(&read(&any)?)),
        ("module-files.md", root.join("src").join("emit").join("rust.rs"), module_files_page(&kitchen_sink)?),
    ])
}

/// The `//!` docs at the top of `source`. The `// @generated` line of a generated file comes before them.
fn module_docs(source: &str) -> String {
    let lines: Vec<&str> =
        source.lines().skip_while(|l| l.starts_with("// ")).map_while(|l| doc_text(l, "//!")).collect();
    lines.join("\n") + "\n"
}

/// The `///` docs of the item whose line starts with `item`. Attributes between the docs and the item are skipped.
fn item_docs(source: &str, item: &str) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let at = lines.iter().position(|l| l.starts_with(item)).with_context(|| format!("no `{item}`"))?;
    let mut docs: Vec<&str> = lines[..at]
        .iter()
        .rev()
        // Attributes, also those over several lines (`#[diagnostic::on_unimplemented(` .. `)]`).
        .skip_while(|l| {
            doc_text(l, "///").is_none()
                && (l.trim_start().starts_with("#[") || l.starts_with("    ") || l.trim() == ")]")
        })
        .map_while(|l| doc_text(l, "///"))
        .collect();
    docs.reverse();
    Ok(docs.join("\n") + "\n")
}

/// One row per variant of the enum that starts with `item`: its name and its rustdoc. Its fields are left out.
fn variants_page(source: &str, item: &str) -> Result<String> {
    let lines = source.lines().skip_while(|l| !l.starts_with(item)).skip(1);
    let mut page = "| Variant | Meaning |\n|---|---|\n".to_owned();
    let mut docs: Vec<&str> = vec![];
    for line in lines.take_while(|l| *l != "}") {
        let Some(own) = line.strip_prefix("    ").filter(|l| !l.starts_with(' ')) else { continue };
        if let Some(text) = doc_text(own, "///") {
            docs.push(text);
        } else if !own.starts_with("#[") && !own.starts_with('}') {
            let name: String = own.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
            anyhow::ensure!(!docs.is_empty(), "variant `{name}` has no rustdoc");
            page.push_str(&format!("| `{name}` | {} |\n", docs.join(" ")));
            docs.clear();
        }
    }
    Ok(page)
}

/// A table of [`crate::settings::SETTINGS`]: every setting of a module with its default.
fn settings_page() -> String {
    let mut page = "| Setting | Required | Default | What it does |\n|---|---|---|---|\n".to_owned();
    for (key, default, what) in crate::settings::SETTINGS {
        let (required, default) = default.map_or(("yes", String::new()), |d| ("no", format!("`{d}`")));
        page.push_str(&format!("| `{key}` | {required} | {default} | {} |\n", what.replace('|', "\\|")));
    }
    page
}

/// The Rust type of each string format: with none of its libraries and with all of them. The generator's own choice
/// makes each cell.
fn formats_page() -> String {
    use crate::ir::types::Format;
    use crate::settings::FormatLibs;
    let none = FormatLibs::default();
    let all = FormatLibs { uuid: true, url: true, ipnet: true, jiff: true, ..Default::default() };
    let ty = |libs: &FormatLibs, f| {
        let path = crate::emit::rust::syntax::format_ty(libs, f).to_string();
        format!("`{}`", path.replace(' ', "").replace("runtime::", "api::"))
    };
    let mut page =
        "| Format | Without its library | With `uuid`, `url`, `ipnet` and `jiff` |\n|---|---|---|\n".to_owned();
    for f in Format::ALL {
        page.push_str(&format!("| `{}` | {} | {} |\n", f.name(), ty(&none, f), ty(&all, f)));
    }
    page
}

/// One row per checked text type of the runtime: its format, its `Formatted` alias in `formatted` and the rustdoc of
/// its marker in `formats`.
fn checked_text_page(formats: &str, formatted: &str) -> String {
    let mut page = "| Format | Type | Accepts |\n|---|---|---|\n".to_owned();
    let mut lines = formats.lines();
    while lines.by_ref().any(|l| l.trim_start().starts_with("marker!(")) {
        let block: Vec<&str> = lines.by_ref().take_while(|l| l.trim() != ");").collect();
        let docs: Vec<&str> = block.iter().filter_map(|l| doc_text(l, "///")).collect();
        let mut rest = block.iter().map(|l| l.trim()).filter(|l| !l.starts_with("///"));
        let marker = rest.next().unwrap_or_default().trim_end_matches(',');
        let format = rest.next().unwrap_or_default().trim_end_matches(',').trim_matches('"');
        let alias = format!(" = Formatted<format::{marker}>;");
        let Some(name) = formatted.lines().find_map(|l| l.strip_prefix("pub type")?.strip_suffix(alias.as_str()))
        else {
            continue;
        };
        page.push_str(&format!("| `{format}` | `api::{}` | {} |\n", name.trim(), docs.join(" ")));
    }
    page
}

/// One row per file of the generated module in `dir`: its name and the first line of its module docs.
fn module_files_page(dir: &Path) -> Result<String> {
    let mut written: Vec<String> = fs::read_dir(dir)?
        .map(|entry| entry.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    written.sort();
    let mut listed: Vec<String> = crate::emit::rust::FILES.iter().map(|(file, _)| (*file).to_owned()).collect();
    listed.sort();
    anyhow::ensure!(written == listed, "`emit::rust::FILES` lists {listed:?} but a module contains {written:?}");
    let mut page = "| File | Contents |\n|---|---|\n".to_owned();
    for (file, what) in crate::emit::rust::FILES {
        page.push_str(&format!("| `{file}` | {what} |\n"));
    }
    Ok(page)
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| path.display().to_string())
}

/// The text of a doc comment line without `///` or `//!` and the space after it.
fn doc_text<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(marker)?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
}

/// The rustdoc of `item` and then one section per method of `block`, except the methods in `elsewhere`: another page
/// of the book shows those.
fn methods_page(source: &str, item: &str, block: &str, elsewhere: &[&str]) -> Result<String> {
    let lines: Vec<&str> = source.lines().collect();
    let find =
        |prefix: &str| lines.iter().position(|l| l.starts_with(prefix)).with_context(|| format!("no `{prefix}`"));
    let (item_at, start) = (find(item)?, find(block)?);
    // Attributes (`#[must_use]`, `#[cfg(..)]`) sit between an item and its docs.
    let docs_before = |at: usize| -> Vec<&str> {
        let mut docs: Vec<&str> = lines[..at]
            .iter()
            .rev()
            // Attributes, also those over several lines (`#[diagnostic::on_unimplemented(` .. `)]`).
            .skip_while(|l| {
                doc_text(l, "///").is_none()
                    && (l.trim_start().starts_with("#[") || l.starts_with("    ") || l.trim() == ")]")
            })
            .map_while(|l| doc_text(l, "///"))
            .collect();
        docs.reverse();
        docs
    };
    let mut page = docs_before(item_at).join("\n") + "\n";
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if *line == "}" {
            break;
        }
        if let Some(sig) = line.trim_start().strip_prefix("pub fn ") {
            let method = sig.split('(').next().unwrap_or(sig);
            let method = method.split('<').next().unwrap_or(method);
            if elsewhere.contains(&method) {
                continue;
            }
            page.push_str(&format!("\n### `{method}`\n\n{}\n", docs_before(i).join("\n")));
        }
    }
    Ok(page)
}

/// Who turns the runtime feature `name` on as [`crate::settings::FEATURES`] says.
fn turned_on(name: &str) -> String {
    use crate::settings::{FEATURES, Side, Trigger};
    match FEATURES.iter().find(|(n, _)| *n == name).map(|(_, t)| t) {
        Some(Trigger::Library(Side::Client)) => format!("`client = {{ adapters = [\"{name}\"] }}`"),
        Some(Trigger::Library(Side::Server)) => format!("`server = {{ adapters = [\"{name}\"] }}`"),
        Some(Trigger::Library(Side::Both)) => format!("`adapters = [\"{name}\"]` in `client` or `server`"),
        Some(Trigger::Half(Side::Client)) => "You generate the client".to_owned(),
        Some(Trigger::Half(Side::Server)) => "You generate the server".to_owned(),
        Some(Trigger::Half(Side::Both)) => "You generate either half".to_owned(),
        Some(Trigger::Description) => "The description calls for it".to_owned(),
        Some(Trigger::Chosen) => "You or `cargo honig init` add it".to_owned(),
        None => "Never: `generate` removes it".to_owned(),
    }
}

/// A table of the `[features]` of `manifest`. It has the `##` comment of each feature, the first dependency that it
/// turns on and `when` it is on. Not its version: a version bump would make the page stale. A feature without a `##`
/// comment is an error.
fn features_page(manifest: &str, when: impl Fn(&str) -> String) -> Result<String> {
    let doc: DocumentMut = manifest.parse()?;
    let mut page = "| Feature | Crate | Adds | On when |\n|---|---|---|---|\n".to_owned();
    let mut docs = vec![];
    let section = manifest.lines().skip_while(|l| *l != "[features]").skip(1);
    for line in section.take_while(|l| !l.starts_with('[')) {
        if let Some(text) = doc_text(line, "##") {
            docs.push(text);
        } else if let Some((name, _)) = line.split_once(" = ") {
            let deps = doc["features"][name].as_array().context(name.to_owned())?;
            let dep = deps.iter().find_map(|d| d.as_str()?.strip_prefix("dep:")).unwrap_or_default();
            anyhow::ensure!(!docs.is_empty(), "feature `{name}` has no `##` comment");
            page.push_str(&format!("| `{name}` | {dep} | {} | {} |\n", docs.join(" "), when(name)));
            docs.clear();
        }
    }
    Ok(page)
}

/// The intra-doc links of rustdoc (``[`Server`]``, ``[`serve`](__private::serve)``) as plain code. The book has no
/// item pages to link to.
fn plain_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("[`") {
        let Some((code, tail)) = after.split_once("`]") else { break };
        out.push_str(before);
        out.push_str(&format!("`{code}`"));
        rest = tail.strip_prefix('(').and_then(|t| t.split_once(')')).map_or(tail, |(_, t)| t);
    }
    out.push_str(rest);
    out
}

#[test]
fn the_generator_lists_every_runtime_feature() {
    let manifest: DocumentMut = read(&super::root().join("runtime").join("Cargo.toml")).unwrap().parse().unwrap();
    let mut runtime: Vec<&str> = manifest["features"].as_table().unwrap().iter().map(|(name, _)| name).collect();
    let mut listed: Vec<&str> = crate::settings::FEATURES.iter().map(|(name, _)| *name).collect();
    runtime.sort_unstable();
    listed.sort_unstable();
    assert_eq!(runtime, listed, "runtime/Cargo.toml and crate::settings::FEATURES");
}

#[test]
fn methods_page_has_one_section_per_method() {
    let source = "/// The type.\npub struct T;\n\nimpl T {\n    /// Does a.\n    #[must_use]\n    pub fn a(&self) {}\n\n    pub fn b(&self) {}\n}\n";
    let page = methods_page(source, "pub struct T", "impl T {", &[]).unwrap();
    assert_eq!(page, "The type.\n\n### `a`\n\nDoes a.\n\n### `b`\n\n\n");
}

#[test]
fn features_page_has_one_row_per_feature() {
    let manifest = "[dependencies]\nx = { version = \"1\", optional = true }\n\n[features]\n## Adds x.\n# not shown\nx = [\"dep:x\"]\n## Adds\n## nothing.\nnone = []\n\n[dev-dependencies]\n";
    let page = features_page(manifest, |name| format!("{name} on")).unwrap();
    assert_eq!(
        page,
        "| Feature | Crate | Adds | On when |\n|---|---|---|---|\n| `x` | x | Adds x. | x on |\n| `none` |  | Adds nothing. | none on |\n"
    );
}

#[test]
fn a_feature_without_docs_is_an_error() {
    assert!(features_page("[features]\nx = []\n", str::to_owned).is_err());
}

#[test]
fn intra_doc_links_become_code() {
    assert_eq!(plain_links("see [`Server`] and [`serve`](x::serve), [not]"), "see `Server` and `serve`, [not]");
}
