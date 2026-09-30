//! `velme build --provider external` through the binary (`tooling/40` §2.1, R-CLI-13, `compiler/22` R-SYNTH-28,
//! R-SYNTH-29, `tooling/41` R-SEC-05, R-SEC-12, R-SEC-13, T-10): where the URL and the token come from, what the service
//! sees, and what the learner is told. The service is the in-process test backend of `velme-test-support`.
// `clippy.toml` allows these in `#[test]` bodies only; the helpers below are test code too.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use velme_test_support::backend::{Config, Mode, On, Server};

const SOURCE: &str = "language: velme/0.1

goal Double(n: Number) -> Number:
    plan: \"Double it.\"
    check:
        - result == n * 2
    examples:
        - Double(2) == 4
";

struct Out {
    stdout: String,
    stderr: String,
    code: i32,
}

/// A project with `SOURCE`, and a directory of backend replies holding a good `Double` body.
fn project(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cli-external").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("replies")).expect("project directory");
    fs::write(dir.join("game.velme"), SOURCE).expect("source");
    let double = json!({"kind": "binary", "op": "mul", "left": {"kind": "input", "name": "n"},
                        "right": {"kind": "literal", "type": {"t": "Number"}, "value": 2}});
    fs::write(dir.join("replies/Double.json"), double.to_string()).expect("reply");
    let replies = dir.join("replies");
    (dir, replies)
}

fn velme(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> Out {
    let mut command = Command::new(env!("CARGO_BIN_EXE_velme"));
    command
        .args(args)
        .current_dir(dir)
        .env_remove("NO_COLOR")
        .env_remove("VELME_SYNTH_RECORD")
        .env_remove("VELME_API_KEY")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("VELME_MODEL")
        .env_remove("VELME_EXTERNAL_URL")
        .env_remove("VELME_EXTERNAL_TOKEN")
        .envs(envs.iter().copied());
    let out = command.output().expect("velme runs");
    Out {
        stdout: String::from_utf8(out.stdout).expect("utf-8"),
        stderr: String::from_utf8(out.stderr).expect("utf-8"),
        code: out.status.code().expect("an exit code"),
    }
}

/// `build game.velme --provider external --external-url URL` in `dir`.
fn build(dir: &Path, url: &str, envs: &[(&str, &str)]) -> Out {
    velme(
        dir,
        &["build", "game.velme", "--provider", "external", "--external-url", url],
        envs,
    )
}

fn shown(out: &Out) -> String {
    format!("{}{}", out.stdout, out.stderr)
}

/// Every file under `dir`, as text where it is, for grepping.
fn all_text(dir: &Path, found: &mut String) {
    for entry in fs::read_dir(dir).expect("directory").filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            all_text(&path, found);
        } else if let Ok(text) = fs::read_to_string(&path) {
            found.push_str(&text);
        }
    }
}

/// The API-key variables never reach the service: with sentinel values in them and a token of its own, the only thing the
/// service is sent is that token; none of the sentinels lands in any output or file, the replay fixtures a recording
/// build writes included (AC-SYNTH-18, R-SEC-13, AC-SEC-05). A project `velme.toml` that names the URL is not a source
/// of it (R-CLI-13, T-10).
#[test]
fn ac_synth_18_the_service_gets_only_its_own_token_and_the_project_cannot_name_it() {
    let (dir, replies) = project("token");
    let outside = dir.parent().expect("a parent").join("token-dump");
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&outside).expect("dump directory");
    let dump = outside.join("auth.txt");
    let log = outside.join("log.txt");
    let mut config = Config::replying(&replies)
        .wanting_token("tok-SENTINEL-TOKEN-0123")
        .logging(&log);
    config.auth_dump = Some(dump.clone());
    let server = Server::start(config);
    let envs = [
        ("VELME_API_KEY", "sk-SENTINEL-ONE"),
        ("ANTHROPIC_API_KEY", "sk-SENTINEL-TWO"),
        ("openai_api_key", "sk-SENTINEL-THREE"),
        ("VELME_EXTERNAL_TOKEN", "tok-SENTINEL-TOKEN-0123"),
        ("VELME_SYNTH_RECORD", "1"),
    ];
    let out = build(&dir, server.url(), &envs);
    assert_eq!(out.code, 0, "{}", shown(&out));
    assert_eq!(
        fs::read_to_string(&dump).expect("the header"),
        "Bearer tok-SENTINEL-TOKEN-0123"
    );
    let mut everything = shown(&out);
    all_text(&dir, &mut everything);
    assert!(
        dir.join("tests/fixtures/synth/replay.json").is_file(),
        "the build was recorded"
    );
    assert!(
        !everything.contains("SENTINEL"),
        "a key or the token reached an output or a file"
    );

    // The project's own `velme.toml` can't name the URL: with one that does, and no flag or variable, the build asks for a
    // URL and the service is never contacted.
    let (dir, _) = project("toml");
    let log2 = outside.join("log2.txt");
    let server = Server::start(Config::replying(dir.join("replies")).logging(&log2));
    fs::write(
        dir.join("velme.toml"),
        format!(
            "external_url = \"{0}\"\n[synthesis]\nexternal_url = \"{0}\"\n[providers.external]\nurl = \"{0}\"\n",
            server.url()
        ),
    )
    .expect("velme.toml");
    let out = velme(&dir, &["build", "game.velme", "--provider", "external"], &[]);
    assert_eq!(out.code, 2, "{}", shown(&out));
    assert!(
        shown(&out).contains("VL0405") && shown(&out).contains("VELME_EXTERNAL_URL"),
        "{}",
        shown(&out)
    );
    assert!(!log2.exists(), "the service named by the project was contacted");
    let _ = fs::remove_dir_all(&outside);
}

/// A URL that is plain `http` to a host that isn't this machine, has another scheme or user information, or doesn't parse is
/// `VL0902` before any contact, whether it comes from the flag or the environment; an absent one is `VL0405` (AC-SYNTH-36,
/// R-SYNTH-29, R-CLI-13).
#[test]
fn ac_synth_36_a_bad_url_is_vl0902_before_any_contact() {
    let (dir, _) = project("bad-url");
    for url in [
        "http://example.com",
        "http://10.0.0.5:8080",
        "http://127.0.0.1.evil.example",
        "http://localhost.evil.example",
        "ftp://localhost",
        "file:///etc/passwd",
        "https://user:pw@example.com",
        "http://user@127.0.0.1:9",
        "127.0.0.1:9",
        "https://",
        "https://exa mple.com",
        "https://example.com/?a=b",
    ] {
        let out = build(&dir, url, &[]);
        let text = shown(&out);
        assert_eq!(out.code, 64, "{url}: {text}");
        assert!(text.contains("VL0902"), "{url}: {text}");
        assert!(!text.contains("VL0404"), "{url} was contacted: {text}");
        assert!(
            !text.contains("pw@") && !text.contains("user:pw"),
            "{url}: the URL was echoed: {text}"
        );
        assert!(!text.contains("Sending"), "{url}: {text}");
    }
    let env = velme(
        &dir,
        &["build", "game.velme", "--provider", "external"],
        &[("VELME_EXTERNAL_URL", "http://example.com")],
    );
    assert!(shown(&env).contains("VL0902"), "{}", shown(&env));
    let none = velme(&dir, &["build", "game.velme", "--provider", "external"], &[]);
    let text = shown(&none);
    assert!(text.contains("VL0405") && text.contains("VELME_EXTERNAL_URL"), "{text}");
}

/// The URL comes from the flag or the environment; the notice names the host, once, before the first contact, and the
/// cached build prints none; a build that needs no provider doesn't mind (R-CLI-13, R-CLI-12, R-SEC-12).
#[test]
fn the_url_comes_from_the_flag_or_the_environment_and_the_notice_names_the_host() {
    let (dir, replies) = project("sources");
    let log = dir.parent().expect("a parent").join("sources.log");
    let _ = fs::remove_file(&log);
    let server = Server::start(Config::replying(&replies).logging(&log));
    let out = velme(
        &dir,
        &["build", "game.velme", "--provider", "external"],
        &[("VELME_EXTERNAL_URL", server.url())],
    );
    assert_eq!(out.code, 0, "{}", shown(&out));
    let notice =
        "Sending your plans, types, checks and examples to the external backend at 127.0.0.1 to write the code.";
    assert_eq!(out.stderr.matches(notice).count(), 1, "{}", out.stderr);
    assert!(
        !out.stderr.contains(server.url()),
        "the URL, with its port, is not shown"
    );
    let again = velme(
        &dir,
        &["build", "game.velme", "--provider", "external"],
        &[("VELME_EXTERNAL_URL", server.url())],
    );
    assert_eq!(again.code, 0);
    assert!(!again.stderr.contains("Sending"), "{}", again.stderr);
    assert_eq!(
        fs::read_to_string(&log).expect("log").lines().count(),
        2,
        "describe, then one request"
    );
    // The flag wins over the environment.
    fs::remove_file(dir.join("velme.lock")).expect("lock removed");
    let flag = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-url",
            server.url(),
        ],
        &[("VELME_EXTERNAL_URL", "http://example.com")],
    );
    assert_eq!(flag.code, 0, "{}", shown(&flag));
}

/// A malformed token is an error, never dropped; a token the service rejects is `VL0405` naming `VELME_EXTERNAL_TOKEN`,
/// and neither is echoed (R-SEC-13, R-SEC-06, D-101).
#[test]
fn a_malformed_or_rejected_token_is_vl0405_and_never_echoed() {
    let (dir, replies) = project("bad-token");
    let log = dir.parent().expect("a parent").join("bad-token.log");
    let _ = fs::remove_file(&log);
    let server = Server::start(
        Config::replying(&replies)
            .logging(&log)
            .wanting_token("right-token-0123456789"),
    );
    let out = build(
        &dir,
        server.url(),
        &[("VELME_EXTERNAL_TOKEN", "has a space-0123456789")],
    );
    let text = shown(&out);
    assert_eq!(out.code, 2, "{text}");
    assert!(
        text.contains("VL0405") && text.contains("VELME_EXTERNAL_TOKEN"),
        "{text}"
    );
    assert!(!text.contains("has a space"), "{text}");
    assert!(!log.exists(), "a request was sent without the token");
    for (envs, wording) in [
        (&[][..], "wants a token"),
        (
            &[("VELME_EXTERNAL_TOKEN", "wrong-token-0123456789")][..],
            "rejected the token",
        ),
    ] {
        let out = build(&dir, server.url(), envs);
        let text = shown(&out);
        assert_eq!(out.code, 2, "{text}");
        assert!(
            text.contains("VL0405") && text.contains("VELME_EXTERNAL_TOKEN"),
            "{text}"
        );
        assert!(text.contains(wording), "{envs:?}: {text}");
        assert!(
            !text.contains("wrong-token-0123456789") && !text.contains("right-token-0123456789"),
            "{text}"
        );
    }
    let good = build(
        &dir,
        server.url(),
        &[("VELME_EXTERNAL_TOKEN", "right-token-0123456789")],
    );
    assert_eq!(good.code, 0, "{}", shown(&good));
}

/// A backend that answers `{"pending"}` leaves the goal waiting and the build failed with exit code 2; the next build,
/// once the backend has the answer, succeeds (AC-SYNTH-31, D-45).
#[test]
fn a_pending_answer_exits_2_and_the_next_build_asks_again() {
    let (dir, replies) = project("pending");
    let good = fs::read_to_string(replies.join("Double.json")).expect("reply");
    fs::write(replies.join("Double.json"), r#"{"pending": "ticket 42"}"#).expect("pending");
    let server = Server::start(Config::replying(&replies));
    let first = build(&dir, server.url(), &[]);
    assert_eq!(first.code, 2, "{}", shown(&first));
    let text = shown(&first);
    assert!(text.contains("VL0408") && text.contains("ticket 42"), "{text}");
    assert!(!dir.join("velme.lock").exists());
    fs::write(replies.join("Double.json"), good).expect("answer");
    let second = build(&dir, server.url(), &[]);
    assert_eq!(second.code, 0, "{}", shown(&second));
    assert!(dir.join("velme.lock").is_file());
}

/// A backend failure is `VL0406` with exit code 2 and the tail of its reply as a note, escaped; a redirect is not
/// followed; `external` defaults to no retries, so the failing backend is asked once (R-SYNTH-28, R-SYNTH-30).
#[test]
fn a_failing_backend_is_vl0406_and_is_asked_once() {
    for (name, mode) in [("failing", Mode::Garbage), ("redirect", Mode::Redirect)] {
        let (dir, replies) = project(name);
        let log = dir.parent().expect("a parent").join(format!("{name}.log"));
        let _ = fs::remove_file(&log);
        let server = Server::start(
            Config::replying(&replies)
                .logging(&log)
                .misbehaving(mode, On::Synthesize),
        );
        let out = build(&dir, server.url(), &[]);
        assert_eq!(out.code, 2, "{}", shown(&out));
        let text = shown(&out);
        assert!(text.contains("VL0406") && text.contains("velme-test-support"), "{text}");
        // Human mode shows the raw reply escaped, never an ESC byte (AC-CLI-14 is M6; here nothing raw gets through).
        assert!(!text.contains('\u{1b}'), "a raw ESC reached the terminal");
        assert_eq!(
            fs::read_to_string(&log).expect("log").lines().collect::<Vec<_>>(),
            ["describe", "synthesize Double"],
            "{name}: describe, then one request, and no redirect followed"
        );
    }
}

/// A service that isn't there is `VL0404` after the transport retries; and once a goal ends that way the service is asked
/// nothing more, however many goals are left: a two-goal project counts the requests it received (R-SYNTH-12,
/// R-SYNTH-45, D-101).
#[test]
fn a_service_that_is_not_there_is_vl0404_and_no_goal_is_contacted_again() {
    let (dir, _) = project("gone");
    let gone = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
        format!("http://{}", listener.local_addr().expect("an address"))
    };
    let out = build(&dir, &gone, &[]);
    let text = shown(&out);
    assert_eq!(out.code, 2, "{text}");
    assert!(text.contains("VL0404"), "{text}");
    assert!(!dir.join("velme.lock").exists());

    // Two goals; the service answers `describe`, then rate limits every request.
    let (dir, replies) = project("gone-two");
    fs::write(
        dir.join("game.velme"),
        format!("{SOURCE}\ngoal Triple(n: Number) -> Number:\n    plan: \"Triple it.\"\n    examples:\n        - Triple(2) == 6\n"),
    )
    .expect("two goals");
    let log = dir.parent().expect("a parent").join("gone-two.log");
    let _ = fs::remove_file(&log);
    let server = Server::start(
        Config::replying(&replies)
            .logging(&log)
            .misbehaving(Mode::Status(429), On::Synthesize),
    );
    let out = build(&dir, server.url(), &[]);
    let text = shown(&out);
    assert_eq!(out.code, 2, "{text}");
    assert_eq!(text.matches("VL0404").count(), 2, "both goals end with VL0404: {text}");
    let logged = fs::read_to_string(&log).expect("log");
    let asked: Vec<&str> = logged.lines().collect();
    assert_eq!(asked.len(), 4, "describe, then one goal's three tries: {asked:?}");
    assert_eq!(asked[0], "describe");
    assert!(asked[1..].iter().all(|line| *line == asked[1]), "{asked:?}");
}

/// The token is taken out of everything the service says: a `200` `{"error"}` that echoes it, and a `backend_version`
/// equal to it, reach no output, `--json`, `replay.json`, lock or artifact (R-SEC-13, D-101).
#[test]
fn the_token_never_comes_back_from_the_service() {
    let token = "tok-ECHOED-1234-0123456";
    let envs = [("VELME_EXTERNAL_TOKEN", token), ("VELME_SYNTH_RECORD", "1")];
    let (dir, replies) = project("echo-error");
    fs::write(
        replies.join("Double.json"),
        json!({"error": format!("bad token {token}")}).to_string(),
    )
    .expect("reply");
    let server = Server::start(Config::replying(&replies).wanting_token(token));
    let human = build(&dir, server.url(), &envs);
    assert_eq!(human.code, 2, "{}", shown(&human));
    assert!(
        shown(&human).contains("VL0406") && shown(&human).contains("bad token ***"),
        "{}",
        shown(&human)
    );
    let as_json = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-url",
            server.url(),
            "--json",
        ],
        &envs,
    );
    let mut everything = format!("{}{}{}", shown(&human), as_json.stdout, as_json.stderr);
    // The backend's own reply file holds the text it echoes; it isn't Velme's.
    fs::remove_dir_all(&replies).expect("replies removed");
    all_text(&dir, &mut everything);
    assert!(!everything.contains(token), "the token came back");

    // A version that is the token itself is recorded as `***`, in the manifest and in `replay.json`.
    let (dir, replies) = project("echo-version");
    let server = Server::start(Config::replying(&replies).wanting_token(token).describing("svc", token));
    let out = build(&dir, server.url(), &envs);
    assert_eq!(out.code, 0, "{}", shown(&out));
    let as_json = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "external",
            "--external-url",
            server.url(),
            "--json",
        ],
        &envs,
    );
    let mut everything = format!("{}{}{}", shown(&out), as_json.stdout, as_json.stderr);
    all_text(&dir, &mut everything);
    assert!(!everything.contains(token), "the token came back");
    assert!(
        fs::read_to_string(dir.join("tests/fixtures/synth/replay.json"))
            .expect("replay.json")
            .contains("***")
    );
}

/// `velme run --input` never reaches a provider: the service isn't contacted, and nothing of the input is sent anywhere
/// (AC-SEC-06, R-SEC-08).
#[test]
fn ac_sec_06_a_run_with_input_makes_no_provider_request() {
    let (dir, replies) = project("run-input");
    let outside = dir.parent().expect("a parent");
    let log = outside.join("run-input.log");
    let capture = outside.join("run-input.json");
    let _ = fs::remove_file(&log);
    let server = Server::start(Config::replying(&replies).logging(&log).capturing(&capture));
    let envs = [("VELME_EXTERNAL_URL", server.url())];
    let built = velme(&dir, &["build", "game.velme", "--provider", "external"], &envs);
    assert_eq!(built.code, 0, "{}", shown(&built));
    let (logged, captured) = (fs::read(&log).expect("log"), fs::read(&capture).expect("capture"));
    fs::write(dir.join("input.json"), r#"{"n": 987654321}"#).expect("input");
    let out = velme(
        &dir,
        &["run", "game.velme", "--goal", "Double", "--input", "input.json"],
        &envs,
    );
    assert_eq!(out.code, 0, "{}{}", out.stdout, out.stderr);
    assert!(out.stdout.contains("1975308642"), "{}", out.stdout);
    assert_eq!(fs::read(&log).expect("log"), logged, "the service was contacted again");
    assert_eq!(fs::read(&capture).expect("capture"), captured);
    assert!(!String::from_utf8_lossy(&captured).contains("987654321"));
}

/// The `ollama` provider needs a model and no key: without one the goal is `VL0405` and no server is contacted
/// (R-CLI-12, R-SYNTH-24).
#[test]
fn the_ollama_provider_needs_a_model_and_no_key() {
    let (dir, _) = project("ollama");
    let out = velme(&dir, &["build", "game.velme", "--provider", "ollama"], &[]);
    let shown = format!("{}{}", out.stdout, out.stderr);
    assert_eq!(out.code, 2, "{shown}");
    assert!(shown.contains("VL0405") && shown.contains("--model"), "{shown}");
    assert!(!shown.contains("API"), "no key is asked for: {shown}");
    assert!(!out.stderr.contains("Sending"), "nothing is sent: {}", out.stderr);
}

/// A replayed `external` build asks as often as the recorded one did: with no retries, so a rejected reply that was
/// recorded once replays once and the goal fails the same way, rather than asking for an attempt no fixture holds
/// (R-SYNTH-30, R-SYNTH-43).
#[test]
fn a_replayed_external_build_reproduces_the_recorded_one() {
    let (dir, replies) = project("replay-retries");
    // A reply that fails the goal's example.
    let wrong = fs::read_to_string(replies.join("Double.json"))
        .expect("reply")
        .replace("\"value\":2", "\"value\":3");
    fs::write(replies.join("Double.json"), wrong).expect("wrong reply");
    let server = Server::start(Config::replying(&replies));
    let record = build(&dir, server.url(), &[("VELME_SYNTH_RECORD", "1")]);
    let recorded = shown(&record);
    assert_eq!(record.code, 2, "{recorded}");
    assert!(recorded.contains("VL0403"), "{recorded}");
    let replayed = velme(&dir, &["build", "game.velme", "--provider", "replay"], &[]);
    assert_eq!(replayed.code, 2, "{}", shown(&replayed));
    let text = shown(&replayed);
    assert!(text.contains("VL0403") && !text.contains("VL0404"), "{text}");
}

/// The notice of what is sent names the user's model, which reaches the terminal escaped: no raw ESC byte or bidi control
/// gets through (R-SEC-12, D-47). The external notice holds only a host of checked characters.
#[test]
fn the_notice_shows_the_model_escaped() {
    let (dir, _) = project("notice-escape");
    let model = velme(
        &dir,
        &[
            "build",
            "game.velme",
            "--provider",
            "ollama",
            "--model",
            "m\u{1b}[31mx\u{202e}y",
        ],
        &[],
    );
    assert!(model.stderr.contains("Sending your plans"), "{}", model.stderr);
    assert!(model.stderr.contains("\\u{1b}"), "{}", model.stderr);
    assert!(
        !model.stderr.contains('\u{1b}') && !model.stderr.contains('\u{202e}'),
        "{:?}",
        model.stderr
    );
    assert!(model.stderr.contains("\\u{202e}"), "{}", model.stderr);
}
