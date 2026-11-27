//! The cargo subcommand.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};

use crate::diag::Diagnostics;
use crate::emit::Options;
use crate::generate::{self, generate_all};
use crate::settings;

#[derive(Parser)]
#[command(name = env!("CARGO_PKG_NAME"), bin_name = "cargo honig", version, arg_required_else_help = true)]
#[command(about = concat!("cargo honig: ", env!("CARGO_PKG_DESCRIPTION")))]
#[command(after_help = concat!("Settings of a module: ", env!("CARGO_PKG_HOMEPAGE"), "/settings.html"))]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Cargo.toml to use
    #[arg(short, long, global = true, value_name = "PATH", default_value = "Cargo.toml")]
    manifest: PathBuf,
    /// Only print errors
    #[arg(short, long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Add the module `api` to Cargo.toml and the `honig` dependency with your adapters
    Init { spec: String },
    /// Generate the modules set in Cargo.toml files
    Generate {
        /// Only this module of [package.metadata.honig]
        module: Option<String>,
        /// Only this workspace member
        #[arg(short, long)]
        package: Option<String>,
        /// Write nothing: fail when a module is stale
        #[arg(long)]
        check: bool,
    },
    /// Validate under its own OpenAPI version and print diagnostics
    Validate {
        /// A description file, or a module name for its spec. Default: all modules
        spec: Option<String>,
    },
}

/// Why the command failed.
#[derive(Debug)]
enum Failure {
    /// A file, the manifest or the generated code.
    Generator(crate::error::Error),
    /// `init` on a manifest that already has `[package.metadata.honig]`.
    AlreadySet(PathBuf),
    /// No package that the workspace command names has the modules that it asks for.
    NoMember(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generator(e) => e.fmt(f),
            Self::AlreadySet(path) => write!(f, "{} already has [package.metadata.honig]", path.display()),
            Self::NoMember(why) => f.write_str(why),
        }
    }
}

impl From<crate::error::Error> for Failure {
    fn from(e: crate::error::Error) -> Self {
        Self::Generator(e)
    }
}

/// The options every command shares.
struct Args {
    manifest: PathBuf,
    quiet: bool,
}

impl Args {
    /// Prints a progress line on stdout. `--quiet` leaves it out and then only errors are printed.
    fn say(&self, line: std::fmt::Arguments<'_>) {
        if !self.quiet {
            println!("{line}");
        }
    }
}

fn print(d: &Diagnostics, quiet: bool) {
    for line in d.lines(quiet) {
        eprintln!("{line}");
    }
}

fn dispatch(command: Command, args: &Args) -> Result<ExitCode, Failure> {
    match command {
        Command::Validate { spec } => validate(spec.as_deref(), args),
        Command::Init { spec } => init(&spec, args),
        Command::Generate { module, package, check } => in_members(check, module.as_deref(), package.as_deref(), args),
    }
}

/// Validates the description file `target`. When `target` is a module, or is absent, it validates the spec files
/// of that module, or of every module.
fn validate(target: Option<&str>, args: &Args) -> Result<ExitCode, Failure> {
    let names_module = |name: &str| {
        settings::Manifest::load(&args.manifest)
            .is_ok_and(|m| settings::modules(&m, &args.manifest).is_ok_and(|ms| ms.iter().any(|x| x.name() == name)))
    };
    let specs = match target {
        Some(spec) if !names_module(spec) => vec![PathBuf::from(spec)],
        only => {
            let manifest = settings::Manifest::load(&args.manifest)?;
            let modules = settings::modules(&manifest, &args.manifest)?;
            let mut specs = vec![];
            for module in modules.into_iter().filter(|m| only.is_none_or(|o| m.name() == o)) {
                specs.extend(settings::settings(module, &args.manifest)?.spec_paths()?);
            }
            specs
        }
    };
    let mut failed = false;
    for spec in specs {
        let diagnostics = crate::load::validate_path(&spec);
        print(&diagnostics, args.quiet);
        if diagnostics.has_errors() {
            failed = true;
        } else {
            args.say(format_args!("ok: {}", spec.display()));
        }
    }
    Ok(if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS })
}

/// Generates or checks the manifest of `args`. At a virtual workspace manifest, that is each member with
/// `[package.metadata.honig.*]`, or the member `package`. With a `module`, only the members that have it.
fn in_members(check: bool, module: Option<&str>, package: Option<&str>, args: &Args) -> Result<ExitCode, Failure> {
    let manifest = settings::Manifest::load(&args.manifest)?;
    let Some(members) = manifest.members() else {
        if let Some(p) = package.filter(|p| manifest.package_name() != Some(*p)) {
            let name = manifest.package_name().unwrap_or("this package");
            return Err(Failure::NoMember(format!("{} is `{name}`, not `{p}`", args.manifest.display())));
        }
        return generate_or_check(check, module, args);
    };
    let mut chosen = vec![];
    for path in members {
        let member = settings::Manifest::load(path)?;
        let has_module =
            |name: &str| settings::modules(&member, path).is_ok_and(|ms| ms.iter().any(|m| m.name() == name));
        if member.table().is_some()
            && package.is_none_or(|p| member.package_name() == Some(p))
            && module.is_none_or(has_module)
        {
            chosen.push(path.clone());
        }
    }
    if chosen.is_empty() {
        let what = match (package, module) {
            (Some(p), Some(m)) => format!("member `{p}` with the module `{m}`"),
            (Some(p), None) => format!("member `{p}` with [package.metadata.honig.*]"),
            (None, Some(m)) => format!("member with the module `{m}`"),
            (None, None) => "member with [package.metadata.honig.*]".to_owned(),
        };
        return Err(Failure::NoMember(format!("the workspace {} has no {what}", args.manifest.display())));
    }
    let mut code = ExitCode::SUCCESS;
    for manifest in chosen {
        let member = Args { manifest, quiet: args.quiet };
        if generate_or_check(check, module, &member)? != ExitCode::SUCCESS {
            code = ExitCode::FAILURE;
        }
    }
    Ok(code)
}

/// One module generated in memory: its settings and its files.
struct Module {
    cfg: settings::Settings,
    generated: generate::Generated,
}

/// The modules generated in memory and honig features that all of them need.
struct Plan {
    runtime: Vec<&'static str>,
    selected: Vec<Module>,
}

/// Generates or checks every module, or only the module `only`.
///
/// the honig dependency serves every module. So it needs the features of every module, also when `only` writes one
/// module. Nothing is written when it lacks one.
fn generate_or_check(check: bool, only: Option<&str>, args: &Args) -> Result<ExitCode, Failure> {
    let manifest = settings::Manifest::load(&args.manifest)?;
    let Some(Plan { runtime, selected }) = generate_modules(&manifest, only, args)? else {
        return Ok(ExitCode::FAILURE);
    };
    print_diagnostics(&selected, args);
    crate::manifest::check_runtime(&manifest, &runtime)?;
    if check {
        return Ok(check_modules(&selected, args));
    }
    write_modules(&selected, args)
}

/// Returns every module generated in memory, and honig features that all of them need. Only the module `only` is
/// kept, when there is one. `None` when a module does not generate: its diagnostics are printed.
fn generate_modules(manifest: &settings::Manifest, only: Option<&str>, args: &Args) -> Result<Option<Plan>, Failure> {
    let modules = settings::modules(manifest, &args.manifest)?;
    settings::ensure_module(&modules, only)?;
    let mut runtime: Vec<&'static str> = vec![];
    let mut selected = vec![];
    let mut outputs: Vec<(PathBuf, String)> = vec![];
    for module in modules {
        let cfg = settings::settings(module, &args.manifest)?;
        if let Some((_, other)) = outputs.iter().find(|(out, _)| *out == cfg.output) {
            return Err(Failure::Generator(crate::error::Error::Module {
                table: cfg.table.clone(),
                problem: format!(
                    "generates into `{}` like [{other}]. Give each module its own `output`",
                    cfg.output.display()
                ),
            }));
        }
        outputs.push((cfg.output.clone(), cfg.table.clone()));
        let opts = Options {
            client: cfg.client,
            server: cfg.server,
            tests: cfg.tests,
            allow_unimplemented: cfg.allow_unimplemented,
            keep_unknown: cfg.keep_unknown,
            strict_client: cfg.strict_client,
            operations: cfg.operations.clone(),
            overlays: cfg.overlay_paths(),
            rust: crate::emit::rust::Options {
                derives: cfg.derives.clone(),
                libs: settings::format_libs(module)?,
                binary: cfg.binary,
                crates: manifest.dependency_names(),
            },
        };
        let generated = match generate_all(&cfg.spec_paths()?, &opts) {
            Ok(g) => g,
            Err(d) => {
                print(&d, false);
                if only.is_some_and(|o| module.name() != o) {
                    eprintln!("[{}] does not generate: Cargo.toml needs honig features of every module", cfg.table);
                }
                return Ok(None);
            }
        };
        // honig features every module needs: the halves and the codecs of its description.
        for feature in &generated.runtime_features {
            if !runtime.contains(feature) {
                runtime.push(feature);
            }
        }
        if only.is_none_or(|o| module.name() == o) {
            selected.push(Module { cfg, generated });
        }
    }
    Ok(Some(Plan { runtime, selected }))
}

/// Prints the diagnostics of each module.
fn print_diagnostics(selected: &[Module], args: &Args) {
    for module in selected {
        print(&module.generated.diagnostics, args.quiet);
    }
}

/// Runs `check`: fails when a module differs from what `generate` would write.
fn check_modules(selected: &[Module], args: &Args) -> ExitCode {
    let stale: Vec<String> = selected
        .iter()
        .flat_map(|m| generate::stale(&m.cfg.output_path(), &m.cfg.root_path(), &m.generated.files))
        .collect();
    let outputs: Vec<String> = selected.iter().map(|m| m.cfg.output_path().display().to_string()).collect();
    if stale.is_empty() {
        args.say(format_args!("up to date: {}", outputs.join(", ")));
        return ExitCode::SUCCESS;
    }
    eprintln!("stale generated code in {}:", outputs.join(", "));
    for s in stale {
        eprintln!("  {s}");
    }
    eprintln!("run `cargo honig generate`");
    ExitCode::FAILURE
}

/// Runs `generate`: writes each module.
fn write_modules(selected: &[Module], args: &Args) -> Result<ExitCode, Failure> {
    // A file `src/<module>.rs` of yours next to the directory would make `mod <module>;` ambiguous. Nothing is written
    // then. One that honig wrote is the root of the module, or a root that an earlier run wrote there.
    let beside = |m: &&Module| m.cfg.output_path().with_extension("rs");
    if let Some(m) =
        selected.iter().find(|m| crate::disk::is_file(&beside(m)) && !crate::disk::is_generated(&beside(m)))
    {
        return Err(Failure::Generator(crate::error::Error::Module {
            table: m.cfg.table.clone(),
            problem: format!(
                "generates into `{}`, but `{}.rs` already exists. Rename the table or set `output`",
                m.cfg.output.display(),
                m.cfg.output.display()
            ),
        }));
    }
    for m in selected {
        crate::disk::write_module(&m.cfg.output_path(), &m.cfg.root_path(), &m.generated.files)?;
    }
    for m in selected {
        args.say(format_args!(
            "generated {} ({} operations, {} types)",
            m.cfg.output_path().display(),
            m.generated.operations,
            m.generated.types
        ));
    }
    if !args.quiet {
        let cfgs: Vec<&settings::Settings> = selected.iter().map(|m| &m.cfg).collect();
        hints(&cfgs, &args.manifest);
    }
    Ok(ExitCode::SUCCESS)
}

/// Tells the user what is still missing in their crate.
fn hints(cfgs: &[&settings::Settings], manifest: &Path) {
    let deps = settings::Manifest::load(manifest).unwrap_or_default();
    for line in serde_lines(&deps) {
        eprintln!("hint: run `{line}`");
    }
    for cfg in cfgs {
        let module = cfg.output.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let declared = ["lib.rs", "main.rs"]
            .iter()
            .filter_map(|r| crate::disk::read(&cfg.root.join("src").join(r)).ok())
            .any(|t| t.contains(&format!("mod {module};")) || t.contains(&format!("mod {module} ")));
        if !declared {
            eprintln!("hint: add `pub mod {module};` to src/lib.rs (or src/main.rs)");
        }
    }
}

/// Returns the `cargo add` lines of the serde crates that generated code needs and the crate lacks.
fn serde_lines(manifest: &settings::Manifest) -> Vec<&'static str> {
    [("serde", "cargo add serde -F derive"), ("serde_json", "cargo add serde_json")]
        .into_iter()
        .filter(|(dep, _)| !manifest.has_dependency(dep))
        .map(|(_, line)| line)
        .collect()
}

fn init(spec: &str, args: &Args) -> Result<ExitCode, Failure> {
    let text = crate::disk::read(&args.manifest)?;
    let manifest = settings::Manifest::load(&args.manifest)?;
    if manifest.table().is_some() {
        return Err(Failure::AlreadySet(args.manifest.clone()));
    }
    let out = format!("{}\n\n{}", text.trim_end(), settings::init_table(&spec.replace('\\', "/")));
    crate::disk::write(&args.manifest, &out)?;
    args.say(format_args!("added [package.metadata.honig.api] to {}", args.manifest.display()));
    if manifest.runtime().is_none() {
        // The features that `generate` will ask for. A description that does not generate yet needs the halves.
        let written = settings::Manifest::load(&args.manifest)?;
        let needed = generate_modules(&written, None, args)?.map_or_else(|| vec!["client", "server"], |p| p.runtime);
        args.say(format_args!("next: {}", crate::manifest::add_line(&needed)));
    }
    for line in serde_lines(&manifest) {
        args.say(format_args!("next: {line}"));
    }
    args.say(format_args!("next: cargo honig generate"));
    Ok(ExitCode::SUCCESS)
}

/// Runs `cargo honig` with `args`, the command line after the name of the program.
///
/// The exit code is 2 when the command line or the files around the description are wrong. It is 1 when the
/// description or the generated code is wrong.
pub fn run(args: impl IntoIterator<Item = String>) -> ExitCode {
    let mut args: Vec<String> = args.into_iter().collect();
    // Cargo runs `cargo-<name> <name> <args>` for `cargo <name> <args>`.
    if args.first().is_some_and(|a| a == "honig") {
        args.remove(0);
    }
    let cli = match Cli::try_parse_from(std::iter::once(String::from("cargo honig")).chain(args)) {
        Ok(cli) => cli,
        // `--help` and `--version` are errors to clap that exit with 0.
        Err(e) => {
            e.print().ok();
            return ExitCode::from(u8::try_from(e.exit_code()).unwrap_or(2));
        }
    };
    let args = Args { manifest: cli.manifest, quiet: cli.quiet };
    match dispatch(cli.command, &args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(2)
        }
    }
}

/// Returns the command's `--help`. Unwrapped: a dependency that turns on the `wrap_help` of clap would wrap it to the
/// terminal, and the book contains it as written.
#[must_use]
pub fn usage() -> String {
    Cli::command().term_width(0).render_help().to_string()
}

#[cfg(test)]
mod tests {
    use super::serde_lines;
    use crate::settings::Manifest;

    #[test]
    fn init_says_the_cargo_add_lines_of_the_missing_serde_crates() {
        let lines = |deps: &str| serde_lines(&Manifest::parse(&format!("[dependencies]\n{deps}")).unwrap());
        assert_eq!(lines(""), ["cargo add serde -F derive", "cargo add serde_json"]);
        assert_eq!(lines("serde = \"1\"\nserde_json = \"1\"\n"), Vec::<&str>::new());
    }
}
