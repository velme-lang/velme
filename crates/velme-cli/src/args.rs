//! The command line (`tooling/40` §2, §2.1): which command, which flags, and which of them may go together. A bad flag is
//! not a usage screen but a `VL0902` (R-CLI-14, R-CLI-21, D-108), so it can travel in the `--json` envelope like any input
//! error.

use velme_diagnostics::{Code, Diagnostic, Span};

/// `--color` (`tooling/40` §2.1, R-CLI-28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Each stream by its own state.
    Auto,
    /// Both streams.
    Always,
    /// Neither.
    Never,
}

/// The command (`tooling/40` §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    Check,
    Build,
    Run,
    Test,
    Explain,
    Trace,
    Artifact,
    Gc,
    CacheClean,
}

impl Cmd {
    /// How the command is typed.
    fn name(self) -> &'static str {
        match self {
            Cmd::Check => "check",
            Cmd::Build => "build",
            Cmd::Run => "run",
            Cmd::Test => "test",
            Cmd::Explain => "explain",
            Cmd::Trace => "trace",
            Cmd::Artifact => "artifact",
            Cmd::Gc => "gc",
            Cmd::CacheClean => "cache clean",
        }
    }

    /// Whether the command takes a source file.
    pub fn takes_file(self) -> bool {
        !matches!(self, Cmd::Gc | Cmd::CacheClean)
    }

    /// Whether the command runs goals (`run`, `test`, `trace`), and so may be told to `--build` first.
    fn executes(self) -> bool {
        matches!(self, Cmd::Run | Cmd::Test | Cmd::Trace)
    }
}

/// A command line that parsed.
#[derive(Debug, Clone)]
pub struct Cli {
    pub cmd: Cmd,
    /// The source file; empty for the commands that take none.
    pub file: String,
    pub json: bool,
    pub color: Color,
    pub quiet: bool,
    pub verbose: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub external_url: Option<String>,
    pub ollama_url: Option<String>,
    pub locked: bool,
    pub offline: bool,
    pub build: bool,
    pub jobs: Option<usize>,
    pub config: Option<String>,
    pub goal: Option<String>,
    pub input: Option<String>,
    pub args: Vec<(String, String)>,
}

/// What the command line asked for.
#[derive(Debug, Clone)]
pub enum Parsed {
    Version,
    Run(Box<Cli>),
}

/// A command line that is a usage error: what to report, and where.
#[derive(Debug, Clone)]
pub struct Bad {
    pub json: bool,
    /// The file the command was given, if it got as far as one.
    pub file: Option<String>,
    pub diagnostic: Diagnostic,
}

fn invalid(message: String, help: Option<&str>) -> Diagnostic {
    let d = Diagnostic::new(Code::InvalidInput, Span::default(), message);
    match help {
        Some(help) => d.with_help(help),
        None => d,
    }
}

/// "`{flag}` can't be used here: {reason}." (`reference/90` VL0902).
fn misuse(flag: &str, reason: &str) -> Diagnostic {
    invalid(format!("`{flag}` can't be used here: {reason}."), None)
}

/// "Input `{name}` should be {expected}, but got {found}." (`reference/90` VL0902).
fn expected(name: &str, expected: &str, found: &str) -> Diagnostic {
    invalid(format!("Input `{name}` should be {expected}, but got {found}."), None)
}

/// The command line `args` (after the program name). `--json` is a global flag, so it may come before or after the command;
/// the other flags follow it, in any order.
pub fn parse(args: &[String]) -> Result<Parsed, Box<Bad>> {
    let json = args.iter().any(|a| a == "--json");
    let mut rest = args.iter().map(String::as_str).filter(|a| *a != "--json");
    let bad = |file: Option<&str>, diagnostic: Diagnostic| {
        Box::new(Bad {
            json,
            file: file.map(str::to_owned),
            diagnostic,
        })
    };
    let Some(word) = rest.next() else {
        return Err(bad(None, expected("COMMAND", "a command", "nothing")));
    };
    if matches!(word, "--version" | "-V") {
        return match rest.next() {
            None if !json => Ok(Parsed::Version),
            _ => Err(bad(None, misuse(word, "it takes nothing with it"))),
        };
    }
    let cmd = match word {
        "check" => Cmd::Check,
        "build" => Cmd::Build,
        "run" => Cmd::Run,
        "test" => Cmd::Test,
        "explain" => Cmd::Explain,
        "trace" => Cmd::Trace,
        "artifact" => Cmd::Artifact,
        "gc" => Cmd::Gc,
        "cache" => match rest.next() {
            Some("clean") => Cmd::CacheClean,
            _ => {
                return Err(bad(
                    None,
                    misuse("cache", "the only cache command is `velme cache clean`"),
                ));
            }
        },
        other => return Err(bad(None, misuse(other, "Velme has no such command"))),
    };
    let mut cli = Cli {
        cmd,
        file: String::new(),
        json,
        color: Color::Auto,
        quiet: false,
        verbose: false,
        provider: None,
        model: None,
        external_url: None,
        ollama_url: None,
        locked: false,
        offline: false,
        build: false,
        jobs: None,
        config: None,
        goal: None,
        input: None,
        args: Vec::new(),
    };
    let mut file: Option<String> = None;
    let mut provider_flag: Option<&str> = None;
    while let Some(arg) = rest.next() {
        let at = |file: &Option<String>| file.clone();
        let fail = |file: &Option<String>, d: Diagnostic| bad(at(file).as_deref(), d);
        // The value of a flag that takes one.
        let mut value = |flag: &str, file: &Option<String>| -> Result<String, Box<Bad>> {
            rest.next()
                .map(str::to_owned)
                .ok_or_else(|| fail(file, misuse(flag, "it needs a value")))
        };
        let once = |flag: &str, set: bool, file: &Option<String>| -> Result<(), Box<Bad>> {
            if set {
                Err(fail(file, misuse(flag, "it was given twice")))
            } else {
                Ok(())
            }
        };
        let only = |flag: &str, ok: bool, why: &str, file: &Option<String>| -> Result<(), Box<Bad>> {
            if ok { Ok(()) } else { Err(fail(file, misuse(flag, why))) }
        };
        let run_only = format!("`velme {}` doesn't take it", cmd.name());
        match arg {
            "--color" => {
                let v = value(arg, &file)?;
                cli.color = match v.as_str() {
                    "auto" => Color::Auto,
                    "always" => Color::Always,
                    "never" => Color::Never,
                    _ => return Err(fail(&file, expected(arg, "auto, always or never", &format!("`{v}`")))),
                };
            }
            "-q" | "--quiet" => cli.quiet = true,
            "-v" | "--verbose" => cli.verbose = true,
            "--config" => {
                only(arg, cmd.takes_file(), &run_only, &file)?;
                once(arg, cli.config.is_some(), &file)?;
                cli.config = Some(value(arg, &file)?);
            }
            "--provider" | "--model" | "--external-url" | "--ollama-url" => {
                only(arg, cmd.takes_file(), &run_only, &file)?;
                let slot = match arg {
                    "--provider" => &mut cli.provider,
                    "--model" => &mut cli.model,
                    "--external-url" => &mut cli.external_url,
                    _ => &mut cli.ollama_url,
                };
                once(arg, slot.is_some(), &file)?;
                *slot = Some(value(arg, &file)?);
                provider_flag.get_or_insert(arg);
            }
            "--locked" => {
                only(arg, cmd.takes_file(), &run_only, &file)?;
                cli.locked = true;
            }
            "--offline" => {
                only(arg, cmd.takes_file(), &run_only, &file)?;
                cli.offline = true;
            }
            "--build" => {
                only(
                    arg,
                    cmd.executes(),
                    if cmd == Cmd::Build {
                        "`velme build` always builds"
                    } else {
                        &run_only
                    },
                    &file,
                )?;
                cli.build = true;
            }
            "--jobs" => {
                only(arg, cmd.executes(), &run_only, &file)?;
                once(arg, cli.jobs.is_some(), &file)?;
                let v = value(arg, &file)?;
                cli.jobs = Some(match v.parse::<usize>() {
                    Ok(n) if n >= 1 => n,
                    _ => {
                        return Err(fail(
                            &file,
                            expected(arg, "a whole number of at least 1", &format!("`{v}`")),
                        ));
                    }
                });
            }
            "--backend" => {
                only(arg, cmd.executes(), &run_only, &file)?;
                match value(arg, &file)?.as_str() {
                    "interp" => {}
                    "wasm" => return Err(fail(&file, misuse("--backend wasm", "it isn't available yet"))),
                    v => return Err(fail(&file, expected(arg, "interp or wasm", &format!("`{v}`")))),
                }
            }
            "--goal" => {
                only(
                    arg,
                    cmd.takes_file() && cmd != Cmd::Check && cmd != Cmd::Build,
                    &run_only,
                    &file,
                )?;
                once(arg, cli.goal.is_some(), &file)?;
                cli.goal = Some(value(arg, &file)?);
            }
            "--input" => {
                only(arg, matches!(cmd, Cmd::Run | Cmd::Trace), &run_only, &file)?;
                once(arg, cli.input.is_some(), &file)?;
                cli.input = Some(value(arg, &file)?);
            }
            "--arg" => {
                only(arg, matches!(cmd, Cmd::Run | Cmd::Trace), &run_only, &file)?;
                let v = value(arg, &file)?;
                match v.split_once('=') {
                    Some((name, json)) => cli.args.push((name.to_owned(), json.to_owned())),
                    None => return Err(fail(&file, expected(arg, "name=JSON", &format!("`{v}`")))),
                }
            }
            flag if flag.starts_with('-') => {
                return Err(fail(&file, misuse(flag, "Velme has no such flag")));
            }
            positional => {
                if !cmd.takes_file() {
                    return Err(fail(
                        &file,
                        misuse(positional, &format!("`velme {}` takes no file", cmd.name())),
                    ));
                }
                if file.is_some() {
                    return Err(fail(&file, misuse(positional, "only one file at a time")));
                }
                file = Some(positional.to_owned());
            }
        }
    }
    if cmd.takes_file() {
        let Some(file) = file else {
            return Err(bad(None, expected("FILE", "a source file", "nothing")));
        };
        if cmd != Cmd::Check && cmd != Cmd::Build && cmd != Cmd::Test && cli.goal.is_none() {
            return Err(bad(Some(&file), expected("--goal", "the name of a goal", "nothing")));
        }
        cli.file = file;
    }
    let file = cmd.takes_file().then(|| cli.file.clone());
    if cli.build && cli.locked {
        return Err(bad(
            file.as_deref(),
            invalid(
                "`--build` and `--locked` can't be used together.".to_owned(),
                Some("`--locked` forbids synthesis; drop one of them"),
            ),
        ));
    }
    if let Some(flag) = provider_flag
        && cmd != Cmd::Build
        && !cli.build
    {
        return Err(bad(
            file.as_deref(),
            misuse(
                flag,
                "it chooses how goals are built, so it needs `velme build` or `--build`",
            ),
        ));
    }
    Ok(Parsed::Run(Box::new(cli)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(line: &str) -> Result<Parsed, Box<Bad>> {
        let args: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
        parse(&args)
    }

    fn message(line: &str) -> String {
        parsed(line).expect_err("a usage error").diagnostic.message
    }

    #[test]
    fn a_bad_flag_is_vl0902_naming_the_flag_and_why() {
        assert_eq!(
            message("check a.velme --provider replay"),
            "`--provider` can't be used here: it chooses how goals are built, so it needs `velme build` or `--build`."
        );
        assert_eq!(
            message("check a.velme --nope"),
            "`--nope` can't be used here: Velme has no such flag."
        );
        assert_eq!(
            message("run a.velme --goal G --backend wasm"),
            "`--backend wasm` can't be used here: it isn't available yet."
        );
        assert_eq!(
            message("run a.velme --goal G --jobs 0"),
            "Input `--jobs` should be a whole number of at least 1, but got `0`."
        );
        assert_eq!(
            message("check a.velme --color pink"),
            "Input `--color` should be auto, always or never, but got `pink`."
        );
        assert_eq!(
            message("check a.velme --build"),
            "`--build` can't be used here: `velme check` doesn't take it."
        );
    }

    #[test]
    fn provider_flags_are_valid_on_build_and_with_build() {
        assert!(parsed("build a.velme --provider replay --model m").is_ok());
        assert!(parsed("run a.velme --goal G --build --provider replay").is_ok());
        assert!(parsed("test a.velme --build --ollama-url http://localhost:1").is_ok());
        assert!(parsed("run a.velme --goal G --provider replay").is_err());
        assert!(parsed("run a.velme --goal G --build --locked").is_err());
    }

    #[test]
    fn json_may_come_first_and_reaches_the_error() {
        let bad = parsed("--json check a.velme --nope").expect_err("a usage error");
        assert!(bad.json);
        assert_eq!(bad.file.as_deref(), Some("a.velme"));
    }

    #[test]
    fn gc_and_cache_clean_take_no_file() {
        assert!(parsed("gc").is_ok());
        assert!(parsed("cache clean -q").is_ok());
        assert!(parsed("gc a.velme").is_err());
        assert!(parsed("cache").is_err());
        assert!(parsed("gc --provider replay").is_err());
    }
}
