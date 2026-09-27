//! `velme` binary: the only crate that prints, reads the environment and picks exit codes (R-CMP-03).
#![forbid(unsafe_code)]

use std::process::ExitCode;

/// Bad flags exit with 64 (R-CLI-10).
const EXIT_USAGE: u8 = 64;

fn version_line() -> String {
    format!("velme {}", env!("CARGO_PKG_VERSION"))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag] if flag == "--version" || flag == "-V" => {
            println!("{}", version_line());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: velme --version");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_is_snapshotted() {
        insta::assert_snapshot!(version_line(), @"velme 0.1.0");
    }
}
