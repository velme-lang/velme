//! The `anthropic`, `ollama` and `external` providers as `velme build` chooses them (`tooling/40` §2.1, R-CLI-12, R-CLI-13,
//! `compiler/22` R-SYNTH-24..29): what they need, where each setting comes from, and the notice that says what is sent
//! (`tooling/41` R-SEC-12).

use velme_diagnostics::{Code, Diagnostic, Span};
#[cfg(feature = "test-provider")]
use velme_syntax::SourceFile;
use velme_synth::{
    Anthropic, AnthropicConfig, ApiKey, CommandError, External, ExternalCommand, ExternalConfig, KeyError, OLLAMA_URL,
    Ollama, OllamaConfig, Replay, SynthBackend, SynthOptions,
};

use crate::project::Project;

/// A provider and the notice that says what it sends.
pub type Chosen = (Box<dyn SynthBackend>, String);

/// The provider `name` names, or why there is none (`tooling/40` §2.1, R-CLI-12), recorded as replay fixtures when
/// `VELME_SYNTH_RECORD=1` says so (`compiler/22` R-SYNTH-43); the `replay` provider is never recorded onto itself.
pub fn backend(
    name: &str,
    project: &Project,
    flags: &BuildFlags,
    options: &SynthOptions,
) -> Result<Chosen, Diagnostic> {
    let (backend, notice) = provider(name, project, flags, options)?;
    if name != "replay" && std::env::var("VELME_SYNTH_RECORD").is_ok_and(|v| v == "1") {
        let recorder = velme_synth::Recorder::new(backend, project.root.join(REPLAY_DIR)).with_options(options);
        return Ok((Box::new(recorder), notice));
    }
    Ok((backend, notice))
}

/// The provider `name` names, and the notice that says what it sends (`tooling/41` R-SEC-12): `replay` reads the
/// fixtures of the project's `tests/fixtures/synth`; `scripted` is there only in a build with the `test-provider`
/// feature (D-94).
fn provider(name: &str, project: &Project, flags: &BuildFlags, options: &SynthOptions) -> Result<Chosen, Diagnostic> {
    match name {
        "replay" => Ok((
            Box::new(Replay::new(project.root.join(REPLAY_DIR))),
            "Nothing is sent: the replay provider answers from recorded fixtures.".to_owned(),
        )),
        #[cfg(feature = "test-provider")]
        "scripted" => {
            let path = std::env::var("VELME_SYNTH_SCRIPT").map_err(|_| {
                Diagnostic::new(
                    Code::InvalidInput,
                    Span::default(),
                    "The scripted provider needs `VELME_SYNTH_SCRIPT`, the path of its script.",
                )
            })?;
            let text =
                std::fs::read_to_string(&path).map_err(|e| SourceFile::unreadable(&crate::display_path(&path), &e))?;
            let scripted = velme_synth::Scripted::from_script(&text)
                .map_err(|e| Diagnostic::new(Code::InvalidInput, Span::default(), e.to_string()))?;
            Ok((
                Box::new(scripted),
                "Nothing is sent: the scripted provider answers from its script.".to_owned(),
            ))
        }
        "anthropic" => anthropic(flags.model, options),
        "ollama" => ollama(flags.model, options),
        "external" => external(flags.external_command, project),
        _ => Err(Diagnostic::new(
            Code::InvalidInput,
            Span::default(),
            format!("I don't know a provider called `{name}`."),
        )
        .with_help("the providers are anthropic, ollama, external and replay")),
    }
}

/// Whether `d` is the library's own `VL0405` for a provider that isn't set up, not a rejected key (R-SYNTH-07).
fn is_generic_not_configured(d: &Diagnostic, goal: &str) -> bool {
    d.code == Code::ProviderNotConfigured
        && d.message
            == velme_synth::provider_diagnostic(&velme_synth::ProviderError::NotConfigured, "", goal, d.span).message
}

/// The provider that can't be used: every goal that needs it is `VL0405` (`tooling/40` R-CLI-12).
pub struct NotConfigured;

#[async_trait::async_trait]
impl SynthBackend for NotConfigured {
    async fn identify(&self) -> Result<velme_synth::Identity, velme_synth::ProviderError> {
        Err(velme_synth::ProviderError::NotConfigured)
    }

    fn open(
        &self,
        _identity: &velme_synth::Identity,
    ) -> Result<Box<dyn velme_synth::SynthProvider>, velme_synth::ProviderError> {
        Err(velme_synth::ProviderError::NotConfigured)
    }
}

/// Where a project's replay fixtures are unless `[synthesis] replay_dir` says otherwise (`tooling/40` §5.1).
pub const REPLAY_DIR: &str = "tests/fixtures/synth";

/// The `[synthesis]` settings of a build with provider `name`: `external` defaults to no retries, since a deterministic
/// backend gives the same answer again (`compiler/22` R-SYNTH-30). A replay follows the build it replays: the provider
/// it recorded, so a replayed external build asks for what the recorded one did, and the settings that shaped its
/// requests and replies (R-SYNTH-43).
pub fn synth_options(name: &str, project: &Project) -> SynthOptions {
    let recorded = if name == "replay" {
        velme_synth::read_replay_identity(&project.root.join(REPLAY_DIR))
            .ok()
            .flatten()
    } else {
        None
    };
    let external = name == "external" || recorded.as_ref().is_some_and(|r| r.provider == "external");
    let mut options = SynthOptions {
        max_retries: if external {
            0
        } else {
            SynthOptions::default().max_retries
        },
        ..SynthOptions::default()
    };
    if let Some(recorded) = &recorded {
        recorded.apply(&mut options);
    }
    options
}

/// The provider of a build that doesn't name one (`tooling/40` §2.1).
pub const DEFAULT_PROVIDER: &str = "anthropic";

/// The flags of `velme build` that choose and set up the provider (`tooling/40` §2.1).
pub struct BuildFlags<'a> {
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub external_command: Option<&'a str>,
    pub verbose: bool,
}

/// Rewords the `VL0405` of `goal` in `diagnostics` for the setup that produced it: `unusable` when the provider couldn't
/// be used, else the missing Ollama model or unknown Anthropic model (`compiler/22` R-SYNTH-24, R-SYNTH-07).
pub fn reword_not_configured(
    diagnostics: &mut [Diagnostic],
    provider: &str,
    model_flag: Option<&str>,
    unusable: Option<&Diagnostic>,
    goal: &str,
) {
    let (message, help): (String, Option<String>) = if let Some(why) = unusable {
        (why.message.clone(), why.help.clone())
    } else if provider == "ollama" {
        let model = model_name(model_flag);
        (
            format!("The Ollama server doesn't have the model `{model}`."),
            Some(format!(
                "run `ollama pull {model}`, or choose another model with `--model`"
            )),
        )
    } else if provider == "anthropic" {
        let model = model_name(model_flag);
        (
            format!("The Anthropic API doesn't know the model `{model}`."),
            Some("choose another model with `--model`, or set `VELME_MODEL`".to_owned()),
        )
    } else {
        return;
    };
    // A rejected key has its own words and stays as it is (R-SYNTH-07).
    let only_generic = unusable.is_none() && provider == "anthropic";
    for d in diagnostics
        .iter_mut()
        .filter(|d| d.code == Code::ProviderNotConfigured && (!only_generic || is_generic_not_configured(d, goal)))
    {
        d.message.clone_from(&message);
        d.help.clone_from(&help);
    }
}

pub fn not_configured(message: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic::new(Code::ProviderNotConfigured, Span::default(), message).with_help(help)
}

/// The model of `--model`, else `VELME_MODEL`, trimmed; empty when neither is set (`tooling/40` §5.2).
pub fn model_name(flag: Option<&str>) -> String {
    flag.map(str::to_owned)
        .or_else(|| std::env::var("VELME_MODEL").ok())
        .map(|m| m.trim().to_owned())
        .unwrap_or_default()
}

/// Why the environment holds no API key to send (`tooling/40` §5.2, R-SEC-05): none is set, or the one that is set can't
/// be a key. It never says what the key was.
pub fn key_problem() -> Option<Diagnostic> {
    match ApiKey::lookup() {
        Ok(_) => None,
        Err(KeyError::Malformed(variable)) => Some(not_configured(
            format!("The API key in `{variable}` can't be used."),
            &format!("a key is visible ASCII with no spaces: fix `{variable}`, or unset it"),
        )),
        Err(KeyError::Missing) => Some(not_configured(
            "The Anthropic provider needs an API key, and there isn't one.",
            "set `VELME_API_KEY` (or `ANTHROPIC_API_KEY`) in the environment; Velme reads a key from nowhere else",
        )),
    }
}

/// The `anthropic` provider, for the model from `--model`, else `VELME_MODEL` (`tooling/40` §5.2, R-CLI-12). The key
/// comes only from the environment (R-SEC-05) and is read again at each request, never held here. It is built without
/// one: the identity step contacts nothing, so a build the store can answer needs no key, and a request that does need
/// it ends with `VL0405` (see [`key_problem`], which the caller words that with). A missing model can't be built
/// without, and is reported after a missing key.
pub fn anthropic(flag: Option<&str>, options: &SynthOptions) -> Result<Chosen, Diagnostic> {
    let model = model_name(flag);
    if model.is_empty() {
        return Err(key_problem().unwrap_or_else(|| {
            not_configured(
                "The Anthropic provider needs a model, and there isn't one.",
                "pass `--model ID`, or set `VELME_MODEL`, to the model id to use",
            )
        }));
    }
    let config = AnthropicConfig {
        options: options.clone(),
        ..AnthropicConfig::new(model)
    };
    Ok((
        Box::new(Anthropic::new(config)),
        "Sending your plans, types, checks and examples to Anthropic to write the code.".to_owned(),
    ))
}

/// The `ollama` provider for the model from `--model`, else `VELME_MODEL`. No key is needed; the server is the default
/// local one, since the config file that sets `ollama_url` comes with M6.
pub fn ollama(flag: Option<&str>, options: &SynthOptions) -> Result<Chosen, Diagnostic> {
    let model = model_name(flag);
    if model.is_empty() {
        return Err(not_configured(
            "The Ollama provider needs a model, and there isn't one.",
            "pass `--model NAME`, or set `VELME_MODEL`, to a model the server has",
        ));
    }
    let notice = format!(
        "Sending your plans, types, checks and examples to the Ollama server at {OLLAMA_URL}, model {model}, to write the code."
    );
    let config = OllamaConfig {
        options: options.clone(),
        ..OllamaConfig::new(model)
    };
    Ok((Box::new(Ollama::new(config)), notice))
}

/// The `external` provider for the command from `--external-command`, else `VELME_EXTERNAL_COMMAND` (R-CLI-13; the
/// user-level config comes with M6, and the project's `velme.toml` is never a source). A command that can't be used is
/// `VL0902` when it is empty or a relative path (`compiler/22` R-SYNTH-29), and `VL0405` when nothing names it or it isn't
/// on `PATH`.
pub fn external(flag: Option<&str>, project: &Project) -> Result<Chosen, Diagnostic> {
    let text = flag
        .map(str::to_owned)
        .or_else(|| std::env::var("VELME_EXTERNAL_COMMAND").ok())
        .filter(|text| !text.trim().is_empty());
    let Some(text) = text else {
        return Err(not_configured(
            "The external provider needs a command, and there isn't one.",
            "pass `--external-command CMD`, or set `VELME_EXTERNAL_COMMAND`; the project's velme.toml can't name it",
        ));
    };
    let invalid = |message: String| {
        Diagnostic::new(Code::InvalidInput, Span::default(), message)
            .with_help("give an absolute path, or the bare name of a program on PATH")
    };
    let words = split_words(&text).map_err(|why| invalid(format!("The external command isn't well formed: {why}.")))?;
    let command = ExternalCommand::resolve(&words, &project.root).map_err(|error| match error {
        CommandError::Empty => invalid("The external command names no program.".to_owned()),
        CommandError::Relative(path) => invalid(format!(
            "The external command `{path}` is a relative path, which Velme never runs."
        )),
        CommandError::NotFound(name) => not_configured(
            format!("The external command `{name}` isn't on your PATH."),
            "give its absolute path, or put it in a directory on PATH that isn't the project's",
        ),
    })?;
    let notice = format!(
        "Sending your plans, types, checks and examples to the command `{}` to write the code.",
        command.display()
    );
    let config = ExternalConfig::new(command, &project.root);
    Ok((Box::new(External::new(config)), notice))
}

/// `text` split into words the way a shell would with quoting only: no globbing and no variable or `~` expansion
/// (R-CLI-13). Single quotes are literal; a backslash escapes the next character, and inside double quotes only `"` and
/// `\`.
fn split_words(text: &str) -> Result<Vec<String>, &'static str> {
    let (mut words, mut word, mut open) = (Vec::new(), String::new(), false);
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if open {
                    words.push(std::mem::take(&mut word));
                    open = false;
                }
            }
            '\'' => {
                open = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => word.push(c),
                        None => return Err("a single quote isn't closed"),
                    }
                }
            }
            '"' => {
                open = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => word.push(c),
                            Some(c) => {
                                word.push('\\');
                                word.push(c);
                            }
                            None => return Err("a double quote isn't closed"),
                        },
                        Some(c) => word.push(c),
                        None => return Err("a double quote isn't closed"),
                    }
                }
            }
            '\\' => {
                open = true;
                word.push(chars.next().ok_or("the text ends with a backslash")?);
            }
            c => {
                open = true;
                word.push(c);
            }
        }
    }
    if open {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::{is_generic_not_configured, split_words};

    /// Quoting only: quotes group, a backslash escapes, and nothing else is expanded (R-CLI-13).
    #[test]
    fn words_are_split_with_quoting_only() {
        let words = |text| split_words(text).expect("well formed");
        assert_eq!(words("impl --queue velme"), ["impl", "--queue", "velme"]);
        assert_eq!(words(r#"impl "a b" 'c $HOME' d\ e"#), ["impl", "a b", "c $HOME", "d e"]);
        assert_eq!(words("impl $HOME ~ *.txt"), ["impl", "$HOME", "~", "*.txt"]);
        assert_eq!(words(r#"a "" b"#), ["a", "", "b"]);
        assert_eq!(words("  "), Vec::<String>::new());
        assert!(split_words("impl 'open").is_err());
        assert!(split_words("impl \"open").is_err());
        assert!(split_words("impl \\").is_err());
    }

    /// Only the library's own "no provider is set up" `VL0405` (an unknown model) is reworded for the Anthropic provider;
    /// a rejected key keeps its words (R-SYNTH-07).
    #[test]
    fn only_the_generic_not_configured_diagnostic_is_reworded() {
        use velme_synth::{ProviderError, provider_diagnostic};
        let generic = provider_diagnostic(&ProviderError::NotConfigured, "", "Double", Default::default());
        let rejected = provider_diagnostic(&ProviderError::KeyRejected, "", "Double", Default::default());
        assert!(is_generic_not_configured(&generic, "Double"));
        assert!(!is_generic_not_configured(&rejected, "Double"));
        assert!(!is_generic_not_configured(&generic, "Other"));
    }
}
