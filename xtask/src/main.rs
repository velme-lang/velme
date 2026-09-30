//! `cargo xtask <verify [--quick] | layering | features | ac-audit [--strict] [--list]>`.

use std::process::ExitCode;

use anyhow::{Result, bail};
use xtask::{ac_audit, features, layering, verify, workspace_root};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("xtask: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root()?;
    let flag = |name: &str| args.iter().skip(1).any(|a| a == name);
    match args.first().map(String::as_str) {
        Some("verify") => {
            let quick = flag("--quick");
            let failed = verify::run_steps(&root, &verify::steps(quick)?);
            if failed.is_empty() {
                println!("verify: all steps passed");
                Ok(ExitCode::SUCCESS)
            } else {
                println!("verify: FAILED: {}", failed.join(", "));
                Ok(ExitCode::FAILURE)
            }
        }
        Some("layering") => {
            let violations = layering::check_manifest(&root.join("Cargo.toml"))?;
            for violation in &violations {
                println!("layering: {violation}");
            }
            Ok(if violations.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some("features") => {
            let offending = features::check(&root)?;
            for line in &offending {
                println!(
                    "features: velme-cli's normal dependencies enable `{}`: {line}",
                    features::TEST_ENDPOINT
                );
            }
            Ok(if offending.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Some("ac-audit") => {
            let report = ac_audit::audit(&root)?;
            let uncovered = report.uncovered();
            println!(
                "ac-audit: {} of {} criteria have a test",
                report.covered.len(),
                report.defined.len()
            );
            for id in &report.unknown {
                println!("ac-audit: a test cites unknown criterion {id}");
            }
            if flag("--list") || flag("--strict") {
                for id in &uncovered {
                    println!("ac-audit: no test for {id}");
                }
            }
            Ok(if report.passes(flag("--strict")) {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        _ => bail!("usage: cargo xtask <verify [--quick] | layering | features | ac-audit [--strict] [--list]>"),
    }
}
