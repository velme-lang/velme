//! The `external` provider (`compiler/22` §3.2, R-SYNTH-26..29, R-SYNTH-41, D-42, D-45, D-98): the user's command, run
//! once per message with one JSON document on stdin and one on stdout. The command comes from the user, never from a
//! project file; it runs directly, with no shell, in the project root and without any `*_API_KEY` variable
//! (`tooling/41` R-SEC-13). Whatever it says is untrusted: the reply goes through the same validation and verification as
//! an LLM's (R-SYNTH-27).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{Map, Value};
use velme_ir::{MAX_JSON_DEPTH, from_json_str_within, to_canonical_string};

use crate::attempt::{clean_line, clean_name, clean_tail};
use crate::provider::{Identity, ProviderError, SynthBackend, SynthLimits, SynthProvider, SynthReply, Usage};
use crate::request::{ExternalMessage, REQUEST_VERSION, SynthRequest};
use crate::transport::ENVELOPE_DEPTH;

/// The most stdout a backend may write for one message (R-SYNTH-28).
const MAX_STDOUT_BYTES: usize = 2 * 1024 * 1024;

/// How often the supervisor looks whether the command has exited.
const POLL: Duration = Duration::from_millis(10);

/// How long the pipes get to close after the command exits, before the group is killed for holding them open.
const EXIT_GRACE: Duration = Duration::from_millis(250);

/// How long the pipes get to drain once the group is dead, before their threads are abandoned.
const DRAIN: Duration = Duration::from_millis(500);

/// How much of a backend's stderr the notes keep (R-SYNTH-28).
const STDERR_TAIL_BYTES: usize = 4096;

/// The longest a backend's name or version may be after cleaning, in Unicode scalar values (R-SYNTH-26).
const MAX_NAME_CHARS: usize = 128;

/// Why a command line can't be used (`compiler/22` R-SYNTH-29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandError {
    /// There is no program to run.
    Empty,
    /// The program is a relative path, which is never run (`VL0902`, D-50).
    Relative(String),
    /// A bare name that no `PATH` directory outside the project holds (`VL0405`).
    NotFound(String),
}

/// A command line resolved to the program to run: an absolute path, or a bare name found on `PATH` with the current
/// directory, `.` entries and the project directory left out of the search (R-SYNTH-29).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCommand {
    program: PathBuf,
    args: Vec<String>,
    display: String,
}

impl ExternalCommand {
    /// Resolves `argv`, whose first word is the program, for a project at `project`. Nothing is run.
    pub fn resolve(argv: &[String], project: &Path) -> Result<Self, CommandError> {
        let (Some(first), args) = (argv.first(), argv.get(1..).unwrap_or_default()) else {
            return Err(CommandError::Empty);
        };
        if first.is_empty() {
            return Err(CommandError::Empty);
        }
        let path = Path::new(first);
        let program = if path.is_absolute() {
            path.to_path_buf()
        } else if is_bare(path) {
            search_path(first, project).ok_or_else(|| CommandError::NotFound(first.clone()))?
        } else {
            return Err(CommandError::Relative(first.clone()));
        };
        Ok(ExternalCommand {
            program,
            args: args.to_vec(),
            display: first.clone(),
        })
    }

    /// The program as the user wrote it, for the R-SEC-12 notice.
    pub fn display(&self) -> &str {
        &self.display
    }
}

/// Whether `path` is a single plain name: no separator, and not `.` or `..`.
fn is_bare(path: &Path) -> bool {
    let mut parts = path.components();
    matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(_)), None)
    )
}

/// The first executable called `name` in an absolute `PATH` directory that is neither the current directory nor the
/// project's.
fn search_path(name: &str, project: &Path) -> Option<PathBuf> {
    let project = std::fs::canonicalize(project).ok();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|dir| std::fs::canonicalize(dir).ok());
    // The project, the current directory and everything under either is out of the search.
    let inside = |dir: &Path, root: &Option<PathBuf>| root.as_ref().is_some_and(|root| dir.starts_with(root));
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .filter(|dir| dir.is_absolute())
        .filter(|dir| std::fs::canonicalize(dir).is_ok_and(|real| !inside(&real, &project) && !inside(&real, &cwd)))
        .flat_map(|dir| {
            let plain = dir.join(name);
            let suffixed = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            [plain, suffixed]
        })
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The settings of the `external` provider (`tooling/40` §5.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalConfig {
    /// The command.
    pub command: ExternalCommand,
    /// The project root, where the command runs.
    pub root: PathBuf,
    /// The most time any one message may take, `describe` included (`external_timeout_secs`, R-SYNTH-28).
    pub timeout: Duration,
}

impl ExternalConfig {
    /// The default `external_timeout_secs`.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

    /// `command`, run in `root`, with the default timeout.
    pub fn new(command: ExternalCommand, root: impl Into<PathBuf>) -> Self {
        ExternalConfig {
            command,
            root: root.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }
}

/// The `external` backend. Its identity step sends `describe` (R-SYNTH-25, R-SYNTH-26).
#[derive(Debug, Clone)]
pub struct External {
    config: ExternalConfig,
}

impl External {
    /// The backend for `config`.
    pub fn new(config: ExternalConfig) -> Self {
        External { config }
    }

    /// Runs the command for one `message` and reads its one JSON document (R-SYNTH-28).
    fn exchange(&self, message: &ExternalMessage) -> Result<Map<String, Value>, ProviderError> {
        let input = to_canonical_string(message)
            .map_err(|_| ProviderError::Internal("the message could not be written".to_owned()))?;
        let stdout = run(&self.config, input.into_bytes())?;
        let text = String::from_utf8(stdout).map_err(|_| failed("its output wasn't text", ""))?;
        match from_json_str_within::<Value>(&text, MAX_JSON_DEPTH + ENVELOPE_DEPTH) {
            Ok(Value::Object(doc)) => Ok(doc),
            _ => Err(failed(
                "its output wasn't one JSON object, or nested too deeply or repeated a key",
                "",
            )),
        }
    }
}

/// A backend failure with its reason and the cleaned tail of its stderr (R-SYNTH-28).
fn failed(reason: &str, stderr: &str) -> ProviderError {
    ProviderError::BackendFailed {
        reason: reason.to_owned(),
        stderr: stderr.to_owned(),
    }
}

#[async_trait]
impl SynthBackend for External {
    /// Sends `describe` (R-SYNTH-26): the backend's name and version, each 1..=128 cleaned characters.
    async fn identify(&self) -> Result<Identity, ProviderError> {
        let doc = self.exchange(&ExternalMessage::describe())?;
        let text = |key: &str| {
            doc.get(key)
                .and_then(Value::as_str)
                .and_then(|text| clean_name(text, MAX_NAME_CHARS))
        };
        let (Some(backend), Some(version)) = (text("backend"), text("backend_version")) else {
            return Err(failed(
                "its `describe` reply wasn't a `backend` and a `backend_version` of 1 to 128 characters each, with no control or invisible characters",
                "",
            ));
        };
        Ok(Identity {
            provider: "external".to_owned(),
            model: version,
            input_version: REQUEST_VERSION.to_owned(),
            backend: Some(backend),
        })
    }

    fn open(&self, identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        Ok(Box::new(ExternalProvider {
            backend: self.clone(),
            identity: identity.clone(),
        }))
    }
}

/// The provider an [`External`] opens.
struct ExternalProvider {
    backend: External,
    identity: Identity,
}

#[async_trait]
impl SynthProvider for ExternalProvider {
    fn id(&self) -> &str {
        "external"
    }

    fn model(&self) -> &str {
        &self.identity.model
    }

    fn input_version(&self) -> &str {
        &self.identity.input_version
    }

    fn backend(&self) -> &str {
        self.identity.backend.as_deref().unwrap_or("external")
    }

    /// One `synthesize` message (R-SYNTH-26): one provider call, with no transport retry (R-SYNTH-28).
    async fn complete(&self, request: &SynthRequest, _limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        let started = Instant::now();
        let doc = self.backend.exchange(&ExternalMessage::synthesize(request.clone()))?;
        let reply = read_reply(doc)?;
        Ok(SynthReply {
            reply_json: reply,
            usage: Usage::default(),
            latency: started.elapsed(),
        })
    }
}

/// The reply document of a `synthesize` message: exactly one of `ir`, `question`, `pending` or `error` (R-SYNTH-28),
/// as the reply text the retry loop reads (R-SYNTH-10).
fn read_reply(doc: Map<String, Value>) -> Result<String, ProviderError> {
    let unknown = || {
        failed(
            "its reply wasn't exactly one of `ir`, `question`, `pending` or `error`",
            "",
        )
    };
    let mut entries = doc.into_iter();
    let (Some((kind, value)), None) = (entries.next(), entries.next()) else {
        return Err(unknown());
    };
    let text = |value: Value| {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| failed("a reply text wasn't a string", ""))
    };
    match (kind.as_str(), value) {
        ("ir", ir @ Value::Object(_)) => to_canonical_string(&ir).map_err(|_| unknown()),
        ("question", question) => {
            let question = text(question)?;
            to_canonical_string(&serde_json::json!({ "question": question })).map_err(|_| unknown())
        }
        ("pending", pending) => Err(ProviderError::Pending(text(pending)?)),
        ("error", reason) => Err(failed(
            &format!("it reported an error: {}", clean_line(&text(reason)?)),
            "",
        )),
        _ => Err(unknown()),
    }
}

/// Whether an environment variable is a key that never reaches the command: any name ending in `_API_KEY`, in any
/// letter case (`tooling/41` R-SEC-13).
pub fn is_api_key_variable(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().to_ascii_uppercase().ends_with("_API_KEY")
}

/// What the command's supervisor hears, from the threads that watch its pipes and its exit.
enum Event {
    /// The whole stdout up to the cap, and whether the cap was passed.
    Stdout(Vec<u8>, bool),
    /// The last [`STDERR_TAIL_BYTES`] of stderr.
    Stderr(Vec<u8>),
    /// Whether the message was written and stdin closed.
    Stdin(bool),
}

/// Runs the command with `input` on its stdin and returns its stdout (R-SYNTH-28): a failure of any kind is a
/// `BackendFailed` with the tail of stderr. However it ends, success included, the whole process group is killed and the
/// command reaped, so nothing it started outlives the message.
fn run(config: &ExternalConfig, input: Vec<u8>) -> Result<Vec<u8>, ProviderError> {
    let command = &config.command;
    let mut process = Command::new(&command.program);
    process
        .args(&command.args)
        .current_dir(&config.root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for name in std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| is_api_key_variable(name))
    {
        process.env_remove(name);
    }
    #[cfg(unix)]
    {
        // Its own process group, so a timeout can stop the command and whatever it started.
        std::os::unix::process::CommandExt::process_group(&mut process, 0);
    }
    let mut child = process.spawn().map_err(|_| failed("it couldn't be started", ""))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    if let Some(mut stdin) = child.stdin.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let written = stdin.write_all(&input).and_then(|()| stdin.flush()).is_ok();
            drop(stdin);
            let _ = tx.send(Event::Stdin(written));
        });
    }
    if let Some(mut stdout) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0_u8; 8192];
            let mut over = false;
            while let Ok(n) = stdout.read(&mut chunk)
                && n > 0
            {
                kept.extend_from_slice(chunk.get(..n).unwrap_or_default());
                if kept.len() > MAX_STDOUT_BYTES {
                    over = true;
                    break;
                }
            }
            let _ = tx.send(Event::Stdout(kept, over));
        });
    }
    if let Some(mut stderr) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut chunk = [0_u8; 8192];
            while let Ok(n) = stderr.read(&mut chunk)
                && n > 0
            {
                kept.extend_from_slice(chunk.get(..n).unwrap_or_default());
                if kept.len() > 2 * STDERR_TAIL_BYTES {
                    kept.drain(..kept.len() - STDERR_TAIL_BYTES);
                }
            }
            let _ = tx.send(Event::Stderr(kept));
        });
    }
    drop(tx);

    // Only this thread reaps the child, and only once the group is dead, so the leader stays a zombie that keeps its
    // process group id reserved: a signal to the group can never reach an unrelated process (R-SYNTH-28).
    let deadline = Instant::now() + config.timeout;
    let mut heard = Heard::default();
    let mut exited: Option<Instant> = None;
    loop {
        if exited.is_none() && has_exited(&mut child, pid) {
            exited = Some(Instant::now());
        }
        let done = heard.stdout.is_some() && heard.stderr.is_some() && heard.stdin.is_some();
        if heard.cut.is_some() || exited.is_some_and(|at| done || at.elapsed() >= EXIT_GRACE) {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        // The grace after the exit is not the command's time: only a command still running can be too slow.
        if exited.is_none() && left.is_zero() {
            heard.cut = Some(format!("it took longer than {:?}", config.timeout));
            break;
        }
        if let Ok(event) = rx.recv_timeout(left.min(POLL)) {
            heard.hear(event);
        }
    }
    // Every path ends the same way: the whole group is killed, a leader that left it is killed on its own (it is still
    // unreaped, so its pid is still its), and only then is it reaped, which can no longer block for long.
    kill_all(&mut child, pid);
    let status = child.wait();
    // The pipes close with the group, so its last words arrive at once; a holder outside the group is not waited for.
    let drain = Instant::now() + DRAIN;
    while !(heard.stdout.is_some() && heard.stderr.is_some() && heard.stdin.is_some()) {
        let left = drain.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(event) => heard.hear(event),
            Err(_) => break,
        }
    }
    let tail = clean_tail(&heard.stderr.unwrap_or_default(), STDERR_TAIL_BYTES);
    if let Some(reason) = heard.cut {
        return Err(failed(&reason, &tail));
    }
    match status {
        Ok(status) if status.success() => {}
        Ok(status) => {
            let reason = match status.code() {
                Some(code) => format!("it exited with status {code}"),
                None => "it was stopped by a signal".to_owned(),
            };
            return Err(failed(&reason, &tail));
        }
        Err(_) => return Err(failed("it couldn't be waited for", &tail)),
    }
    if heard.stdin != Some(true) {
        return Err(failed("it closed its input before reading the message", &tail));
    }
    heard.stdout.ok_or_else(|| failed("its output couldn't be read", &tail))
}

/// What the supervisor has heard from the pipe threads, and why it cut the command short, if it did.
#[derive(Default)]
struct Heard {
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    stdin: Option<bool>,
    cut: Option<String>,
}

impl Heard {
    fn hear(&mut self, event: Event) {
        match event {
            Event::Stdout(bytes, over) => {
                if over && self.cut.is_none() {
                    self.cut = Some(format!("it wrote more than {} MiB", MAX_STDOUT_BYTES / (1024 * 1024)));
                }
                self.stdout = Some(bytes);
            }
            Event::Stderr(bytes) => self.stderr = Some(bytes),
            Event::Stdin(ok) => self.stdin = Some(ok),
        }
    }
}

/// Whether the command has exited, without reaping it: on unix the zombie stays, so its process group id stays taken.
#[cfg(unix)]
fn has_exited(_child: &mut std::process::Child, pid: u32) -> bool {
    use rustix::process::{Pid, WaitId, WaitIdOptions, waitid};
    let Some(pid) = i32::try_from(pid).ok().and_then(Pid::from_raw) else {
        return true;
    };
    // An error can only mean there is nothing left to wait for.
    waitid(
        WaitId::Pid(pid),
        WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT,
    )
    .map_or(true, |status| status.is_some())
}

#[cfg(not(unix))]
fn has_exited(child: &mut std::process::Child, _pid: u32) -> bool {
    // Windows keeps the exit status on the handle, so `wait` still has it after this.
    child.try_wait().map_or(true, |status| status.is_some())
}

/// Stops the command and everything it started: its whole process group, which it leads (`process_group(0)`), so the
/// group is `pid`'s; then the command itself, in case it left the group. Both are safe because it is not yet reaped.
/// Where there are no groups only the process itself is stopped (group kill is unix-only for now). No helper program is
/// looked up on `PATH`.
#[cfg(unix)]
fn kill_all(child: &mut std::process::Child, pid: u32) {
    use rustix::process::{Pid, Signal, kill_process_group};
    if let Some(group) = i32::try_from(pid).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(group, Signal::KILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_all(child: &mut std::process::Child, _pid: u32) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{CommandError, ExternalCommand, is_api_key_variable};

    /// Every name ending in `_API_KEY`, in any case, is a key; nothing else is (R-SEC-13).
    #[test]
    fn a_key_variable_is_any_name_ending_in_api_key() {
        for name in ["VELME_API_KEY", "anthropic_api_key", "My_Api_Key", "OPENAI_API_KEY"] {
            assert!(is_api_key_variable(&OsString::from(name)), "{name}");
        }
        for name in ["API_KEY_FILE", "APIKEY", "PATH", "VELME_API_KEYS"] {
            assert!(!is_api_key_variable(&OsString::from(name)), "{name}");
        }
    }

    /// A relative path is refused before anything runs; an absolute one is taken as it is (R-SYNTH-29, D-50).
    #[test]
    fn a_relative_program_is_refused_and_an_absolute_one_is_kept() {
        let project = std::env::temp_dir();
        let resolve = |word: &str| ExternalCommand::resolve(&[word.to_owned()], &project);
        assert_eq!(
            resolve("./evil.sh"),
            Err(CommandError::Relative("./evil.sh".to_owned()))
        );
        assert_eq!(resolve("bin/impl"), Err(CommandError::Relative("bin/impl".to_owned())));
        assert_eq!(resolve(".."), Err(CommandError::Relative("..".to_owned())));
        assert_eq!(resolve(""), Err(CommandError::Empty));
        assert_eq!(ExternalCommand::resolve(&[], &project), Err(CommandError::Empty));
        let absolute = std::env::current_exe().expect("this test's own path");
        let kept = ExternalCommand::resolve(&[absolute.to_string_lossy().into_owned()], &project).expect("absolute");
        assert_eq!(kept.program, absolute);
    }
}
