//! `velme` binary: the only crate that prints, reads the environment and picks exit codes (R-CMP-03).
#![forbid(unsafe_code)]

use std::io::{IsTerminal, Write};
use std::process::ExitCode;

use serde::Serialize;
use velme_diagnostics::render::{self, JsonDiagnostic, LineIndex};
use velme_diagnostics::{Code, Diagnostic};
use velme_syntax::SourceFile;

/// Exit codes (`tooling/40` §4, R-CLI-10).
const EXIT_OK: u8 = 0;
const EXIT_INVALID_PROGRAM: u8 = 1;
const EXIT_BUILD_FAILED: u8 = 2;
const EXIT_EXECUTION_FAILED: u8 = 3;
const EXIT_ARTIFACT: u8 = 4;
const EXIT_USAGE: u8 = 64;
const EXIT_INTERNAL: u8 = 70;

/// When codes from different rows fail together, the earlier exit code here wins (R-CLI-16).
const EXIT_PRECEDENCE: [u8; 6] = [
    EXIT_INTERNAL,
    EXIT_USAGE,
    EXIT_INVALID_PROGRAM,
    EXIT_ARTIFACT,
    EXIT_BUILD_FAILED,
    EXIT_EXECUTION_FAILED,
];

/// The `--json` envelope version (D-49, R-CLI-15).
const JSON_FORMAT: &str = "velme-cli/1";

/// Stack for the parser thread. The D-71 limits keep parsing within a few MiB even in a debug build; the main thread's
/// default stack differs by platform (1 MiB on Windows), so the size is set here (R-SYN-19).
const PARSE_STACK: usize = 64 * 1024 * 1024;

const USAGE: &str = "usage: velme check FILE [--json]\n       velme --version";

fn version_line() -> String {
    format!("velme {}", env!("CARGO_PKG_VERSION"))
}

enum Command {
    Version,
    Check { file: String, json: bool },
}

/// `--json` is a global flag (`tooling/40` §2.1), so it may come before or after the command.
fn parse_args(args: &[String]) -> Option<Command> {
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
    match rest.as_slice() {
        ["--version" | "-V"] if !json => Some(Command::Version),
        ["check", file] if !file.starts_with('-') => Some(Command::Check {
            file: (*file).to_owned(),
            json,
        }),
        _ => None,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match parse_args(&args) {
        Some(Command::Version) => {
            print_out(&format!("{}\n", version_line()));
            EXIT_OK
        }
        Some(Command::Check { file, json }) => check(&file, json),
        None => {
            print_err(&format!("{USAGE}\n"));
            EXIT_USAGE
        }
    };
    ExitCode::from(code)
}

/// Output errors (a closed pipe) are ignored: there is nowhere left to report them.
fn print_out(s: &str) {
    let _ = std::io::stdout().lock().write_all(s.as_bytes());
}

fn print_err(s: &str) {
    let _ = std::io::stderr().lock().write_all(s.as_bytes());
}

/// `velme check FILE`: for now, lexing and parsing (M1); name, type and call checks follow in M2.
fn check(arg: &str, json: bool) -> u8 {
    let path = display_path(arg);
    let (text, diagnostics) = match std::fs::read(arg) {
        Err(err) => (None, vec![SourceFile::unreadable(&path, &err)]),
        Ok(bytes) => match SourceFile::from_bytes(path.clone(), bytes.clone()) {
            Ok(file) => {
                let Some(diagnostics) = parse_on_big_stack(&file) else {
                    return EXIT_INTERNAL;
                };
                (Some(file.text), diagnostics)
            }
            // The span is a byte offset into the raw bytes, which the lossy text keeps up to the bad byte.
            Err(diag) => (Some(String::from_utf8_lossy(&bytes).into_owned()), vec![diag]),
        },
    };
    let exit = exit_code(&diagnostics);

    if json {
        let lines = LineIndex::new(text.as_deref().unwrap_or(""));
        let envelope = Envelope {
            format: JSON_FORMAT,
            status: if exit == EXIT_OK { "ok" } else { "failed" },
            results: Vec::new(),
            diagnostics: diagnostics
                .iter()
                .map(|d| JsonDiagnostic::new(d, &path, &lines))
                .collect(),
            notices: Vec::new(),
        };
        match serde_json::to_string_pretty(&envelope) {
            Ok(out) => print_out(&format!("{}\n", render::escape_json(&out))),
            Err(_) => return EXIT_INTERNAL,
        }
    } else {
        if !diagnostics.is_empty() {
            print_err(&render::render_human(&diagnostics, &path, text.as_deref(), use_color()));
        }
        if exit == EXIT_OK {
            print_out("✓ Parsed\n");
        }
    }
    exit
}

/// Parses on a thread with [`PARSE_STACK`]; `None` only if the thread couldn't start or the parser panicked, which is a
/// bug (R-SYN-19).
fn parse_on_big_stack(file: &SourceFile) -> Option<Vec<Diagnostic>> {
    std::thread::scope(|scope| {
        let parser = std::thread::Builder::new()
            .stack_size(PARSE_STACK)
            .spawn_scoped(scope, || velme_syntax::parse(file).1)
            .ok()?;
        parser.join().ok()
    })
}

/// The `--json` envelope (`tooling/40` §3.2). `diagnostics` holds the ones that belong to the file rather than to one
/// goal, such as syntax errors (D-72).
#[derive(Serialize)]
struct Envelope {
    format: &'static str,
    status: &'static str,
    results: Vec<()>,
    diagnostics: Vec<JsonDiagnostic>,
    notices: Vec<String>,
}

/// Paths are shown with `/` separators on every platform (R-CLI-19).
fn display_path(arg: &str) -> String {
    if cfg!(windows) {
        arg.replace('\\', "/")
    } else {
        arg.to_owned()
    }
}

/// Colour only on a terminal, and never when `NO_COLOR` is set (R-CMP-15).
fn use_color() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// The process exit code for a set of diagnostics: warnings never fail (D-69), and errors follow R-CLI-16.
fn exit_code(diagnostics: &[Diagnostic]) -> u8 {
    diagnostics
        .iter()
        .filter(|d| d.is_error())
        .map(|d| code_exit(d.code))
        .min_by_key(|exit| EXIT_PRECEDENCE.iter().position(|e| e == exit))
        .unwrap_or(EXIT_OK)
}

/// The `tooling/40` §4 row for each code.
fn code_exit(code: Code) -> u8 {
    use Code::*;
    match code {
        UnexpectedToken
        | InconsistentIndentation
        | TabIndentation
        | ReservedWord
        | UnterminatedText
        | UnsupportedLanguageVersion
        | LintWarning
        | UnknownType
        | UnknownName
        | DuplicateDeclaration
        | TypeMismatch
        | UnknownField
        | InvalidOperandType
        | NullableAccess
        | RecursiveType
        | UnknownGoal
        | CallArityMismatch
        | InvalidCall
        | CallCycle
        | BindingUsedBeforeDefinition
        | DuplicateBinding
        | GoalHasNoBody
        | InvalidBudget => EXIT_INVALID_PROGRAM,
        IRSchemaInvalid
        | IRInvalid
        | SynthesisFailed
        | ProviderUnavailable
        | ProviderNotConfigured
        | BackendFailed
        | PlanUnclear
        | SynthesisPending
        | SynthesisBlocked
        | VerificationFailed => EXIT_BUILD_FAILED,
        CheckFailed | ExampleFailed | BudgetExceeded | ArithmeticError | Timeout | MemoryLimitExceeded
        | CallLimitExceeded | SizeLimitExceeded | CapabilityDenied => EXIT_EXECUTION_FAILED,
        InternalError => EXIT_INTERNAL,
        ArtifactUnavailable | LockStale | ArtifactCorrupt => EXIT_ARTIFACT,
        FileError | InvalidInput | GoalNotFound => EXIT_USAGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_is_snapshotted() {
        insta::assert_snapshot!(version_line(), @"velme 0.1.0");
    }

    #[test]
    fn every_code_has_an_exit_row() {
        for &code in Code::ALL {
            let exit = code_exit(code);
            assert!(EXIT_PRECEDENCE.contains(&exit), "{code:?} maps to {exit}");
        }
    }

    #[test]
    fn exit_precedence_follows_r_cli_16() {
        let diag = |code| Diagnostic::new(code, Default::default(), "");
        let exit = |codes: &[Code]| exit_code(&codes.iter().map(|&c| diag(c)).collect::<Vec<_>>());
        assert_eq!(exit(&[]), EXIT_OK);
        assert_eq!(exit(&[Code::LintWarning]), EXIT_OK);
        assert_eq!(exit(&[Code::CheckFailed, Code::LockStale]), EXIT_ARTIFACT);
        assert_eq!(exit(&[Code::LockStale, Code::InvalidInput]), EXIT_USAGE);
        assert_eq!(exit(&[Code::InvalidInput, Code::InternalError]), EXIT_INTERNAL);
        assert_eq!(
            exit(&[Code::VerificationFailed, Code::UnexpectedToken]),
            EXIT_INVALID_PROGRAM
        );
    }
}
