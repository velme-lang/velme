//! Velme test support (`delivery/51`): helpers shared by the test suites of several crates, and the test `external` backend.
//! A dev-dependency only.
#![forbid(unsafe_code)]
// A helper here fails the test that called it, so it panics on bad input like a test body does (CC-ERR-01 covers
// non-test code only).
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

pub mod backend;
pub mod schema;

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use velme_ir::{
    CallNode, Fingerprint, Goal, Origin, Request, Synthesis, ValidIr, calls, contract_key, from_json_str, signature,
    synthesis_key, validate,
};
use velme_runtime::{ArtifactFormat, Child, Entry, Lock, Manifest, Store, Verification};
use velme_sema::hir::{GoalId, Program};
use velme_sema::{SourceFile, analyze};
use velme_synth::{
    ChildRunner, Identity, ProviderError, Sleeper, SynthBackend, SynthLimits, SynthProvider, SynthReply, SynthRequest,
};

/// `path`, relative to the repository root.
pub fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

/// The text of the file at `path`.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The checked program of `text`, which must have no errors.
pub fn program(text: &str) -> Program {
    let (program, diags) = analyze(&SourceFile::new("test.velme", text));
    assert!(diags.iter().all(|d| !d.is_error()), "{diags:#?}");
    program.expect("a program without errors")
}

/// The goal called `name`.
pub fn goal_id(program: &Program, name: &str) -> GoalId {
    GoalId(
        program
            .goals
            .iter()
            .position(|g| g.name == name)
            .unwrap_or_else(|| panic!("no goal {name}")),
    )
}

/// The examples of the goal `goal` of `program`, whose text is `source`: each one's inputs and the output it expects
/// (`language/12` R-GOAL-21).
pub fn example_cases(
    program: &Program,
    source: &str,
    goal: GoalId,
) -> Vec<(Vec<velme_builtins::Value>, velme_builtins::Value)> {
    let checks =
        velme_check::GoalChecks::new(program, goal, source).unwrap_or_else(|d| panic!("examples don't lower: {d:#?}"));
    let cases = checks.examples().iter();
    cases.map(|case| (case.args.clone(), case.expected.clone())).collect()
}

/// What the interpreter gives for the leaf goal `ir` on `inputs` within `limits`: the value and the fuel and memory
/// it spent, or the failure. For a backend held to it (INV-3, `runtime/31` R-SBX-15).
pub fn interpret(
    ir: &ValidIr,
    inputs: Vec<velme_builtins::Value>,
    limits: velme_builtins::execution::Limits,
) -> Result<(velme_builtins::Value, velme_builtins::execution::Spent), velme_builtins::execution::Failure> {
    let output = velme_interp::run(ir, inputs, Vec::new(), limits)?;
    let spent = velme_builtins::execution::Spent {
        fuel: output.fuel,
        memory: output.memory,
    };
    Ok((output.value, spent))
}

/// Hand-written IR for a goal of `program`, validated as a whole goal: its `calls` must equal the compiler's
/// (`compiler/21` R-IR-16).
pub fn valid_ir(program: &Program, ir: &str) -> ValidIr {
    let goal: Goal = from_json_str(ir).unwrap_or_else(|e| panic!("IR doesn't parse: {e}"));
    let id = goal_id(program, &goal.goal);
    let calls = calls(program, id).expect("a checked goal has a call section");
    let request = Request {
        program,
        goal: id,
        calls: &calls,
        origin: Origin::Complete,
    };
    validate(ir, &request).unwrap_or_else(|d| panic!("IR doesn't validate: {d:#?}"))
}

/// What a synthesis provider replies with for hand-written IR: its `body` alone, as `{"body": …}` (`compiler/22`
/// R-SYNTH-10, D-103).
pub fn body_reply(ir: &str) -> String {
    let goal: serde_json::Value = from_json_str(ir).unwrap_or_else(|e| panic!("IR doesn't parse: {e}"));
    let body = goal.get("body").unwrap_or_else(|| panic!("IR has no body"));
    serde_json::json!({ "body": body }).to_string()
}

/// The backend a fixture's manifest names: hand-written IR comes from the `external` backend (`runtime/32` R-ART-21).
pub const FIXTURE_BACKEND: &str = "velme-test-support";

/// The fixture backend's `backend_version` and `request_version`.
pub const FIXTURE_VERSION: &str = "hand-written";

/// The fixture backend's model, `<backend>@<backend_version>` (`compiler/22` R-SYNTH-26).
pub fn fixture_model() -> String {
    format!("{FIXTURE_BACKEND}@{FIXTURE_VERSION}")
}

/// The `external` provider id (`runtime/32` R-ART-21).
const EXTERNAL: &str = "external";

/// Installs hand-written IR for a goal of `program`, declared in the project file `file`, into the store and lock of
/// the project `project` (D-16), and returns the artifact's address. The IR is validated but not verified, so a test
/// can install IR that fails its goal's examples or checks on purpose.
pub fn install(project: &Path, file: &str, program: &Program, ir: &str) -> Fingerprint {
    let ir = valid_ir(program, ir);
    let manifest = fixture_manifest(program, &ir);
    install_artifact(project, file, program, &manifest, &ir)
}

/// The manifest [`install`] writes for `ir`: an `external` artifact of [`FIXTURE_BACKEND`] that ran no verification.
pub fn fixture_manifest(program: &Program, ir: &ValidIr) -> Manifest {
    let goal = ir.goal();
    let id = goal_id(program, &goal.goal);
    let contract = contract_key(program, id).expect("contract key");
    let compiler_version = env!("CARGO_PKG_VERSION");
    let model = fixture_model();
    let synthesis = Synthesis {
        input_version: FIXTURE_VERSION,
        compiler_version,
        provider: EXTERNAL,
        model: &model,
    };
    Manifest {
        format: ArtifactFormat,
        goal: goal.goal.clone(),
        kind: program.goals.get(id.0).expect("a goal of the program").kind.into(),
        signature: signature(program, id).expect("signature"),
        contract_key: contract,
        synthesis_key: synthesis_key(contract, &synthesis).expect("synthesis key"),
        language_version: program.language_version.clone(),
        compiler_version: compiler_version.to_owned(),
        ir_version: goal.ir_version.clone(),
        builtins_version: goal.builtins_version.clone(),
        prompt_version: Some(FIXTURE_VERSION.to_owned()),
        provider: EXTERNAL.to_owned(),
        backend: Some(FIXTURE_BACKEND.to_owned()),
        model_version: Some(model),
        children: goal
            .calls
            .iter()
            .map(|CallNode::Call(call)| Child {
                binding: call.binding.clone(),
                goal: call.goal.clone(),
                signature: call.goal_signature.parse().expect("a validated signature"),
            })
            .collect(),
        verification: Verification {
            examples: 0,
            generated_inputs: 0,
            input_set: Fingerprint::of(&[(); 0]).expect("an empty input set"),
            max_fuel_observed: 0,
        },
    }
}

/// Stores `manifest` with `ir` and pins it in the lock of `project` under the keys computed from `program`, whatever
/// the manifest claims, so a test can install a manifest that disagrees with its lock entry (AC-ART-12).
pub fn install_artifact(
    project: &Path,
    file: &str,
    program: &Program,
    manifest: &Manifest,
    ir: &ValidIr,
) -> Fingerprint {
    let id = goal_id(program, &ir.goal().goal);
    let artifact = Store::new(project).put(manifest, ir).expect("artifact stored");
    let mut lock = Lock::read(project)
        .expect("lock readable")
        .unwrap_or_else(|| Lock::new(program.language_version.clone()));
    lock.language = program.language_version.clone();
    lock.insert(Entry {
        file: file.to_owned(),
        name: ir.goal().goal.clone(),
        signature: signature(program, id).expect("signature"),
        contract_key: contract_key(program, id).expect("contract key"),
        artifact,
    });
    lock.write(project).expect("lock written");
    artifact
}

/// A backend and provider that panic on any use: passed where no provider may be contacted, so the test fails if one
/// is (`compiler/20` AC-CMP-02, `compiler/22` AC-SYNTH-01).
#[derive(Debug, Clone, Copy, Default)]
pub struct PanicProvider;

#[async_trait]
impl SynthBackend for PanicProvider {
    async fn identify(&self) -> Result<Identity, ProviderError> {
        panic!("the identity step contacted a provider")
    }

    fn open(&self, _identity: &Identity) -> Result<Box<dyn SynthProvider>, ProviderError> {
        panic!("a provider was built")
    }
}

#[async_trait]
impl SynthProvider for PanicProvider {
    fn id(&self) -> &str {
        panic!("a provider was asked its id")
    }

    fn model(&self) -> &str {
        panic!("a provider was asked its model")
    }

    fn input_version(&self) -> &str {
        panic!("a provider was asked its input version")
    }

    async fn complete(&self, _request: &SynthRequest, _limits: &SynthLimits) -> Result<SynthReply, ProviderError> {
        panic!("a provider was called")
    }
}

/// A sleeper that returns at once and remembers what it was asked to wait: the injected clock of `compiler/22` R-SYNTH-12,
/// so tests of transport retries take no wall time.
#[derive(Debug, Clone, Default)]
pub struct RecordingSleeper {
    waits: std::sync::Arc<std::sync::Mutex<Vec<std::time::Duration>>>,
}

impl RecordingSleeper {
    /// The waits asked for so far, in order.
    pub fn waits(&self) -> Vec<std::time::Duration> {
        self.waits.lock().expect("waits").clone()
    }
}

#[async_trait]
impl Sleeper for RecordingSleeper {
    async fn sleep(&self, duration: std::time::Duration) {
        self.waits.lock().expect("waits").push(duration);
    }
}

/// A runner for candidate goals without calls: it runs the body on the reference interpreter, as the scheduler does for
/// a leaf. `stop_after` makes the `n`th run fail with the watchdog's `VL0603`, for tests of `compiler/22` R-SYNTH-14.
#[derive(Debug, Clone, Default)]
pub struct LeafRunner {
    /// The 1-based run that is stopped by the watchdog, if any.
    pub stop_at: Option<usize>,
    runs: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl LeafRunner {
    /// A runner that never times out.
    pub fn new() -> Self {
        Self::default()
    }

    /// A runner whose `n`th run (from 1) is stopped by the watchdog.
    pub fn timing_out_at(n: usize) -> Self {
        LeafRunner {
            stop_at: Some(n),
            ..Self::default()
        }
    }
}

impl ChildRunner for LeafRunner {
    fn run(
        &self,
        candidate: &ValidIr,
        inputs: Vec<velme_builtins::Value>,
    ) -> Result<velme_check::Invocation, Vec<velme_diagnostics::Diagnostic>> {
        let n = self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if self.stop_at == Some(n) {
            return Err(vec![velme_diagnostics::Diagnostic::new(
                velme_diagnostics::Code::Timeout,
                Default::default(),
                "It took too long.",
            )]);
        }
        let budget = velme_interp::Budget::new(velme_interp::Limits {
            fuel: 1_000_000,
            memory: 1 << 20,
        });
        let (value, spent) = velme_interp::run_measured(candidate, inputs.clone(), Vec::new(), budget);
        match value {
            Ok(result) => Ok(velme_check::Invocation {
                inputs,
                bindings: Vec::new(),
                result,
                fuel: spent.fuel,
                memory: spent.memory,
            }),
            Err(failure) => Err(vec![failure.diagnostic(&candidate.goal().goal, Default::default())]),
        }
    }
}

/// The `anthropic` provider pointed at `server` with the sentinel-able key `key` and a sleeper that never waits (D-98,
/// R-SYNTH-44): the only way a test reaches a mock, since no config key or environment variable sets the base URL.
pub fn mock_anthropic(
    config: velme_synth::AnthropicConfig,
    server: &mock::MockServer,
    key: &str,
    sleeper: &RecordingSleeper,
) -> velme_synth::Anthropic {
    velme_synth::Anthropic::new(config)
        .with_sleeper(std::sync::Arc::new(sleeper.clone()))
        .with_test_endpoint(server.url(), velme_synth::ApiKey::new(key))
}

/// A local HTTP server that plays back canned responses, for the tests of the `anthropic` provider: no network.
pub mod mock {
    use std::collections::{BTreeMap, VecDeque};
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    use serde_json::{Value, json};

    /// One canned response.
    #[derive(Debug, Clone)]
    pub struct MockResponse {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
        hang: bool,
    }

    impl MockResponse {
        /// A `200` with `body`.
        pub fn ok(body: impl Into<String>) -> Self {
            Self::status(200, body)
        }

        /// A response with `status` and `body`.
        pub fn status(status: u16, body: impl Into<String>) -> Self {
            MockResponse {
                status,
                headers: Vec::new(),
                body: body.into(),
                hang: false,
            }
        }

        /// A connection that is accepted and then never answered, until the client gives up.
        pub fn hang() -> Self {
            MockResponse {
                hang: true,
                ..Self::status(200, "")
            }
        }

        /// The response with one more header.
        #[must_use]
        pub fn header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }

        /// An Ollama `/api/tags` response listing `models`, each a name and a digest.
        pub fn ollama_tags(models: &[(&str, &str)]) -> Self {
            let models: Vec<Value> = models
                .iter()
                .map(|(name, digest)| json!({"name": name, "model": name, "digest": digest, "size": 1}))
                .collect();
            Self::ok(json!({ "models": models }).to_string())
        }

        /// An Ollama `/api/chat` response whose message content is the text of `reply`.
        pub fn ollama_chat(reply: &Value) -> Self {
            Self::ok(
                json!({
                    "model": "test",
                    "message": {"role": "assistant", "content": reply.to_string()},
                    "done": true,
                    "done_reason": "stop",
                    "prompt_eval_count": 13,
                    "eval_count": 9,
                })
                .to_string(),
            )
        }

        /// A Messages API response in which the model called `tool` with `input`.
        pub fn tool_call(tool: &str, input: &Value) -> Self {
            Self::ok(
                json!({
                    "id": "msg_test",
                    "type": "message",
                    "role": "assistant",
                    "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "toolu_test", "name": tool, "input": input}],
                    "usage": {
                        "input_tokens": 11,
                        "output_tokens": 7,
                        "cache_read_input_tokens": 5,
                        "cache_creation_input_tokens": 3,
                    },
                })
                .to_string(),
            )
        }
    }

    /// One request the server received.
    #[derive(Debug, Clone)]
    pub struct MockRequest {
        /// The request method.
        pub method: String,
        /// The request path.
        pub path: String,
        /// The headers, with lower-case names.
        pub headers: BTreeMap<String, String>,
        /// The body.
        pub body: String,
    }

    impl MockRequest {
        /// The body as JSON.
        pub fn json(&self) -> Value {
            serde_json::from_str(&self.body).expect("a JSON body")
        }
    }

    /// A server on a free local port. It answers each request with the next canned response, and a `500` when there is
    /// none left. Dropping it stops the thread.
    pub struct MockServer {
        url: String,
        requests: Arc<Mutex<Vec<MockRequest>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl MockServer {
        /// Starts a server that plays `responses` back in order.
        pub fn start(responses: impl IntoIterator<Item = MockResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
            let url = format!("http://{}", listener.local_addr().expect("an address"));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let mut queue: VecDeque<MockResponse> = responses.into_iter().collect();
            let thread = {
                let (requests, stop) = (requests.clone(), stop.clone());
                std::thread::spawn(move || {
                    for stream in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(mut stream) = stream else { continue };
                        let Some(request) = read_request(&mut stream) else {
                            continue;
                        };
                        requests.lock().expect("requests").push(request);
                        let response = queue
                            .pop_front()
                            .unwrap_or_else(|| MockResponse::status(500, "no response queued"));
                        if response.hang {
                            // Held open until the client hangs up.
                            let mut sink = [0_u8; 64];
                            while stream.read(&mut sink).is_ok_and(|n| n > 0) {}
                            continue;
                        }
                        let mut text = format!(
                            "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
                            response.status,
                            response.body.len()
                        );
                        for (name, value) in &response.headers {
                            text.push_str(&format!("{name}: {value}\r\n"));
                        }
                        text.push_str("\r\n");
                        text.push_str(&response.body);
                        let _ = stream.write_all(text.as_bytes());
                    }
                })
            };
            MockServer {
                url,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        /// The base URL of the server.
        pub fn url(&self) -> &str {
            &self.url
        }

        /// The requests received so far, in order.
        pub fn requests(&self) -> Vec<MockRequest> {
            self.requests.lock().expect("requests").clone()
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            // A connection unblocks the `accept` that waits for the next request.
            let _ = TcpStream::connect(self.url.trim_start_matches("http://"));
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    pub(crate) fn read_request(stream: &mut TcpStream) -> Option<MockRequest> {
        let mut data = Vec::new();
        let mut chunk = [0_u8; 4096];
        let end = loop {
            let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
            data.extend_from_slice(&chunk[..n]);
            if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
        };
        let head = String::from_utf8_lossy(&data[..end]).into_owned();
        let mut lines = head.lines();
        let mut first = lines.next()?.split_whitespace();
        let method = first.next()?.to_owned();
        let path = first.next()?.to_owned();
        let headers: BTreeMap<String, String> = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        // A GET has no body, and so no length.
        let length: usize = headers.get("content-length").map_or(Some(0), |v| v.parse().ok())?;
        while data.len() < end + length {
            let n = stream.read(&mut chunk).ok().filter(|n| *n > 0)?;
            data.extend_from_slice(&chunk[..n]);
        }
        Some(MockRequest {
            method,
            path,
            headers,
            body: String::from_utf8_lossy(&data[end..end + length]).into_owned(),
        })
    }
}
