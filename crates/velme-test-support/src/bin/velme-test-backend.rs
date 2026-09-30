//! The test `external` backend as a program (`compiler/22` §3.2, D-42, D-99, D-101): the HTTP service of
//! [`velme_test_support::backend`], for running by hand against `velme build --provider external --external-url
//! http://127.0.0.1:<port>`. Not a product: nothing here is reachable from `velme`.
//!
//! ```text
//! velme-test-backend --dir DIR [--port N] [--log FILE] [--capture FILE] [--describe NAME VERSION] [--describe-extra]
//!                    [--token TOKEN] [--mode MODE [--on describe|synthesize|both]]
//! ```
//!
//! It prints `listening on http://127.0.0.1:<port>` and serves until it is stopped. `--mode` is `status:N`, `garbage`,
//! `huge`, `hang` or `redirect`.
#![forbid(unsafe_code)]
// A tool for tests: it fails loudly on a bad invocation.
#![allow(clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]

use std::io::Write;

use velme_test_support::backend::{Config, Mode, On, Server};

fn parse() -> Config {
    let mut config = Config::default();
    let (mut mode, mut on) = (None, On::Synthesize);
    let mut rest = std::env::args().skip(1);
    while let Some(flag) = rest.next() {
        let mut value = || rest.next().expect("a value after the flag");
        match flag.as_str() {
            "--dir" => config.dir = Some(value().into()),
            "--port" => config.port = value().parse().expect("a port number"),
            "--log" => config.log = Some(value().into()),
            "--capture" => config.capture = Some(value().into()),
            "--token" => config.token = Some(value()),
            "--describe" => {
                let name = value();
                config.describe = (name, value());
            }
            "--describe-extra" => config.describe_extra = true,
            "--mode" => {
                mode = Some(match value().as_str() {
                    "garbage" => Mode::Garbage,
                    "huge" => Mode::Huge,
                    "hang" => Mode::Hang,
                    "redirect" => Mode::Redirect,
                    other => Mode::Status(
                        other
                            .strip_prefix("status:")
                            .and_then(|n| n.parse().ok())
                            .unwrap_or_else(|| panic!("unknown mode {other}")),
                    ),
                });
            }
            "--on" => {
                on = match value().as_str() {
                    "describe" => On::Describe,
                    "synthesize" => On::Synthesize,
                    "both" => On::Both,
                    other => panic!("unknown --on {other}"),
                };
            }
            other => panic!("unknown flag {other}"),
        }
    }
    config.mode = mode.map(|mode| (mode, on));
    config
}

fn main() {
    let server = Server::start(parse());
    println!("listening on {}", server.url());
    std::io::stdout().flush().expect("stdout");
    loop {
        std::thread::park();
    }
}
