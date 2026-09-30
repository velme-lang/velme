//! The `anthropic`, `ollama` and `external` providers as `velme build` chooses them (`tooling/40` §2.1, R-CLI-12, R-CLI-13,
//! `compiler/22` R-SYNTH-24..29): what they need, where each setting comes from, and the notice that says what is sent
//! (`tooling/41` R-SEC-12).

use velme_diagnostics::{Code, Diagnostic, Span};
use velme_syntax::SourceFile;
use velme_synth::{
    Anthropic, AnthropicConfig, ApiKey, DEFAULT_MAX_OUTPUT_TOKENS, External, ExternalConfig, ExternalToken,
    ExternalUrl, KeyError, OLLAMA_DEFAULT_MAX_OUTPUT_TOKENS, OLLAMA_URL, Ollama, OllamaConfig, Replay, SynthBackend,
    SynthOptions, TOKEN_VARIABLE, has_certificate, normalize_model,
};

use crate::config::Settings;
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
        let recorder =
            velme_synth::Recorder::new(backend, project.root.join(replay_dir(flags.settings))).with_options(options);
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
            Box::new(Replay::new(project.root.join(replay_dir(flags.settings)))),
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
        "anthropic" => anthropic(flags, options),
        "ollama" => ollama(flags, options),
        "external" => external(flags),
        _ => Err(unknown_provider(name)),
    }
}

fn unknown_provider(name: &str) -> Diagnostic {
    Diagnostic::new(
        Code::InvalidInput,
        Span::default(),
        format!("I don't know a provider called `{name}`."),
    )
    .with_help("the providers are anthropic, ollama, external and replay")
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

/// The fixture directory of a replay or a recording: the project's `replay_dir`, else [`REPLAY_DIR`] (R-CLI-18).
pub fn replay_dir(settings: &Settings) -> &str {
    settings.project.replay_dir.as_deref().unwrap_or(REPLAY_DIR)
}

/// The `[synthesis]` settings of a build with provider `name`, and what the user's ceilings cut (`tooling/40` §5.1, R-CLI-11,
/// D-50). The order is the built-in default, the project's value, and for a replay what the recording says it ran with, then the
/// ceilings, which only tighten. `external` defaults to no retries, since a deterministic backend gives the same answer again
/// (`compiler/22` R-SYNTH-30). A replay follows the build it replays: the provider it recorded, so a replayed external build
/// asks for what the recorded one did, and the settings that shaped its requests and replies (R-SYNTH-43). The second value
/// is a sentence per ceiling that applied, for the notice (`tooling/41` R-SEC-12).
pub fn synth_options(name: &str, project: &Project, settings: &Settings) -> (SynthOptions, Vec<String>) {
    let recorded = if name == "replay" {
        velme_synth::read_replay_identity(&project.root.join(replay_dir(settings)))
            .ok()
            .flatten()
    } else {
        None
    };
    let external = name == "external" || recorded.as_ref().is_some_and(|r| r.provider == "external");
    let defaults = SynthOptions::default();
    let mut options = SynthOptions {
        max_retries: if external { 0 } else { defaults.max_retries },
        ..defaults
    };
    let config = &settings.project;
    options.max_retries = config.max_retries.unwrap_or(options.max_retries);
    options.max_calls_per_build = config.max_calls_per_build.unwrap_or(options.max_calls_per_build);
    options.timeout_secs = config.timeout_secs.unwrap_or(options.timeout_secs);
    options.max_output_tokens = config.max_output_tokens;
    options.schema_in_prompt = config.schema_in_prompt.unwrap_or(options.schema_in_prompt);
    options.reply_format = config.reply_format.unwrap_or(options.reply_format);
    options.retry_history = config.retry_history.unwrap_or(options.retry_history);
    options.stop_on_repeat = config.stop_on_repeat.unwrap_or(options.stop_on_repeat);
    options.max_prompt_examples = config.max_prompt_examples.unwrap_or(options.max_prompt_examples);
    if let Some(recorded) = &recorded {
        recorded.apply(&mut options);
    }
    let mut cut = Vec::new();
    let user = &settings.user;
    if let Some(ceiling) = user.max_calls_per_build
        && options.max_calls_per_build > ceiling
    {
        let by = asked_by(config.max_calls_per_build.is_some());
        cut.push(format!(
            "{by} max_calls_per_build = {}; your ceiling of {ceiling} applies.",
            options.max_calls_per_build
        ));
        options.max_calls_per_build = ceiling;
    }
    if let Some(ceiling) = user.max_retries
        && options.max_retries > ceiling
    {
        let by = asked_by(config.max_retries.is_some());
        cut.push(format!(
            "{by} max_retries = {}; your ceiling of {ceiling} applies.",
            options.max_retries
        ));
        options.max_retries = ceiling;
    }
    if let Some(ceiling) = user.max_output_tokens {
        let asked = options
            .max_output_tokens
            .unwrap_or_else(|| default_max_output_tokens(name));
        if asked > ceiling {
            let by = asked_by(options.max_output_tokens.is_some());
            cut.push(format!(
                "{by} max_output_tokens = {asked}; your ceiling of {ceiling} applies."
            ));
            options.max_output_tokens = Some(ceiling);
        }
    }
    (options, cut)
}

/// How a notice says where a value a ceiling cut came from: the project's `velme.toml`, else the built-in default.
fn asked_by(project_set_it: bool) -> &'static str {
    if project_set_it {
        "The project asked for"
    } else {
        "The default is"
    }
}

/// The `max_output_tokens` of provider `name` when nothing sets one (D-110).
fn default_max_output_tokens(name: &str) -> u32 {
    if name == "ollama" {
        OLLAMA_DEFAULT_MAX_OUTPUT_TOKENS
    } else {
        DEFAULT_MAX_OUTPUT_TOKENS
    }
}

/// The provider of a build that doesn't name one (`tooling/40` §2.1).
pub const DEFAULT_PROVIDER: &str = "anthropic";

/// The provider a build uses: `--provider`, else the project's, else [`DEFAULT_PROVIDER`] (R-CLI-11).
pub fn provider_name<'a>(flags: &'a BuildFlags) -> &'a str {
    flags
        .provider
        .or(flags.settings.project.provider.as_deref())
        .unwrap_or(DEFAULT_PROVIDER)
}

/// The flags of `velme build` that choose and set up the provider (`tooling/40` §2.1).
pub struct BuildFlags<'a> {
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub external_url: Option<&'a str>,
    pub ollama_url: Option<&'a str>,
    /// The two config files (`tooling/40` §5.1).
    pub settings: &'a Settings,
    pub verbose: bool,
    /// `--locked`: no synthesis and no write (`tooling/40` R-CLI-04).
    pub locked: bool,
    /// `--offline`: no contact of any kind (`tooling/40` R-CLI-05).
    pub offline: bool,
}

/// Rewords the `VL0405` of `goal` in `diagnostics` for the setup that produced it: `unusable` when the provider couldn't
/// be used, else the unknown Anthropic model (`compiler/22` R-SYNTH-07); a missing Ollama model says so itself (R-SYNTH-24).
pub fn reword_not_configured(
    diagnostics: &mut [Diagnostic],
    provider: &str,
    model: &str,
    unusable: Option<&Diagnostic>,
    goal: &str,
) {
    let (message, help): (String, Option<Box<str>>) = if let Some(why) = unusable {
        (why.message.clone(), why.help.clone())
    } else if provider == "anthropic" {
        (
            format!("The Anthropic API doesn't know the model `{model}`."),
            Some("choose another model with `--model`, or set `VELME_MODEL`".into()),
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

/// The model of `--model`, else `VELME_MODEL`, else the project's `model`, trimmed; empty when none is set (`tooling/40` §5.2,
/// R-CLI-11).
pub fn model_name(flag: Option<&str>, settings: &Settings) -> String {
    flag.map(str::to_owned)
        .or_else(|| std::env::var("VELME_MODEL").ok().filter(|m| !m.trim().is_empty()))
        .or_else(|| settings.project.model.clone())
        .map(|m| m.trim().to_owned())
        .unwrap_or_default()
}

/// `VL0405` if `allowed_models` is set and `model` is not in it (`tooling/40` R-CLI-26, D-105): before any contact, and never
/// swapped for an allowed one.
fn allowed(model: &str, settings: &Settings, ollama: bool) -> Result<(), Diagnostic> {
    // Ollama names a model without a tag `:latest`, so both sides are normalized for it (R-SYNTH-24).
    let same = |listed: &str| {
        if ollama {
            normalize_model(listed.trim()) == normalize_model(model)
        } else {
            listed.trim() == model
        }
    };
    match &settings.user.allowed_models {
        Some(list) if !list.iter().any(|m| same(m)) => Err(not_configured(
            format!("The model `{model}` isn't in your allowed models."),
            &format!(
                "you allow {}; pick one with `--model` or `VELME_MODEL`",
                list.iter().map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")
            ),
        )),
        _ => Ok(()),
    }
}

/// The models a build would ask for, each in the allowed ones if there is a list: the model, and the project's `retry_model`
/// (R-CLI-26).
fn check_models(model: &str, settings: &Settings, ollama: bool) -> Result<(), Diagnostic> {
    allowed(model, settings, ollama)?;
    settings
        .project
        .retry_model
        .as_deref()
        .map_or(Ok(()), |retry| allowed(retry.trim(), settings, ollama))
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

/// The `anthropic` provider, for the model from `--model`, `VELME_MODEL` or the project (`tooling/40` §5.2, R-CLI-12). The
/// key comes only from the environment (R-SEC-05) and is read again at each request, never held here. It is built without
/// one: the identity step contacts nothing, so a build the store can answer needs no key, and a request that does need
/// it ends with `VL0405` (see [`key_problem`], which the caller words that with). A missing model can't be built
/// without, and is reported after a missing key.
pub fn anthropic(flags: &BuildFlags, options: &SynthOptions) -> Result<Chosen, Diagnostic> {
    let settings = flags.settings;
    let model = model_name(flags.model, settings);
    if model.is_empty() {
        return Err(key_problem().unwrap_or_else(|| {
            not_configured(
                "The Anthropic provider needs a model, and there isn't one.",
                "pass `--model ID`, or set `VELME_MODEL`, to the model id to use",
            )
        }));
    }
    check_models(&model, settings, false)?;
    let config = AnthropicConfig {
        retry_model: settings.project.retry_model.clone(),
        prompt_cache: settings.project.prompt_cache.unwrap_or(true),
        options: options.clone(),
        ..AnthropicConfig::new(model)
    };
    Ok((
        Box::new(Anthropic::new(config)),
        "Sending your plans, types, checks and examples to Anthropic to write the code.".to_owned(),
    ))
}

/// The `ollama` provider for the model from `--model`, `VELME_MODEL` or the project, at the server of `--ollama-url`,
/// `VELME_OLLAMA_URL` or the user-level config, else the local default (R-CLI-13). No key is needed.
pub fn ollama(flags: &BuildFlags, options: &SynthOptions) -> Result<Chosen, Diagnostic> {
    let settings = flags.settings;
    let model = model_name(flags.model, settings);
    if model.is_empty() {
        return Err(not_configured(
            "The Ollama provider needs a model, and there isn't one.",
            "pass `--model NAME`, or set `VELME_MODEL`, to a model the server has",
        ));
    }
    check_models(&model, settings, true)?;
    let url = first_set([
        flags.ollama_url.map(str::to_owned),
        std::env::var("VELME_OLLAMA_URL").ok(),
        settings.user.ollama_url.clone(),
    ])
    .unwrap_or_else(|| OLLAMA_URL.to_owned());
    let url = parse_url(url.trim(), "Ollama")?;
    let notice = format!(
        "Sending your plans, types, checks and examples to the Ollama server at {}, model {model}, to write the code.",
        url.host()
    );
    let config = OllamaConfig {
        retry_model: settings.project.retry_model.clone(),
        options: options.clone(),
        url: Some(url),
        ..OllamaConfig::new(model)
    };
    Ok((Box::new(Ollama::new(config)), notice))
}

/// The first of `sources`, in order of precedence, that isn't blank: an empty flag or variable doesn't hide a lower source.
fn first_set(sources: [Option<String>; 3]) -> Option<String> {
    sources.into_iter().flatten().find(|text| !text.trim().is_empty())
}

/// The URL `text` of the `what` service, or `VL0902` before any contact (`compiler/22` R-SYNTH-29, `tooling/40` R-CLI-13).
fn parse_url(text: &str, what: &str) -> Result<ExternalUrl, Diagnostic> {
    ExternalUrl::parse(text).map_err(|why| {
        Diagnostic::new(
            Code::InvalidInput,
            Span::default(),
            format!("The {what} URL can't be used: {why}."),
        )
        .with_help("give an https URL, or an http one to localhost, 127.0.0.1 or [::1], with no user name or password")
    })
}

/// The provider name and the flag values of a build that constructs no provider (`--locked`, `--offline`), checked as a
/// build would: the same `VL0902`, with no provider built and no key or token read (`tooling/40` R-CLI-21).
pub fn check_flags(name: &str, flags: &BuildFlags) -> Result<(), Diagnostic> {
    if !(["anthropic", "ollama", "external", "replay"].contains(&name)
        || (cfg!(feature = "test-provider") && name == "scripted"))
    {
        return Err(unknown_provider(name));
    }
    // A blank flag is already a usage error in `args` (D-111), so a URL given here is never blank.
    flags.external_url.map(|url| parse_url(url, "external")).transpose()?;
    flags
        .ollama_url
        .map(|url| parse_url(url, "Ollama"))
        .transpose()
        .map(|_| ())
}

/// The `external` provider for the URL from `--external-url`, `VELME_EXTERNAL_URL` or the user-level config (R-CLI-13; the
/// project's `velme.toml` is never a source), with the bearer token of `VELME_EXTERNAL_TOKEN` if there is one, the user's
/// `external_timeout_secs` and the certificates of `external_ca_file`. A URL that can't be used is `VL0902` before any contact
/// (`compiler/22` R-SYNTH-29); a token that can't be one is `VL0405`, as a malformed API key is, and is never left out; a CA
/// file that can't be read, or holds no certificate, is `VL0901`.
pub fn external(flags: &BuildFlags) -> Result<Chosen, Diagnostic> {
    let user = &flags.settings.user;
    let text = first_set([
        flags.external_url.map(str::to_owned),
        std::env::var("VELME_EXTERNAL_URL").ok(),
        user.external_url.clone(),
    ]);
    let Some(text) = text else {
        return Err(not_configured(
            "The external provider needs a URL, and there isn't one.",
            "pass `--external-url URL`, or set `VELME_EXTERNAL_URL` or `external_url` in your user-level config; the project's velme.toml can't name it",
        ));
    };
    let url = parse_url(&text, "external")?;
    let ca_pem = user.external_ca_file.as_deref().map(read_ca).transpose()?;
    let token = ExternalToken::lookup().map_err(|_| {
        not_configured(
            format!("The token in `{TOKEN_VARIABLE}` can't be used."),
            &format!(
                "a token is 16 or more visible ASCII characters with no spaces: fix `{TOKEN_VARIABLE}`, or unset it"
            ),
        )
    })?;
    let notice = format!(
        "Sending your plans, types, checks and examples to the external backend at {} to write the code.",
        url.host()
    );
    let mut config = ExternalConfig::new(url);
    config.token = token;
    config.ca_pem = ca_pem;
    if let Some(secs) = user.external_timeout_secs {
        config.timeout = std::time::Duration::from_secs(secs);
    }
    Ok((Box::new(External::new(config)), notice))
}

/// The bytes of the PEM file `path` (`external_ca_file`), which must hold at least one certificate (D-105).
fn read_ca(path: &str) -> Result<Vec<u8>, Diagnostic> {
    let shown = crate::display_path(path);
    let bytes = std::fs::read(path).map_err(|e| SourceFile::unreadable(&shown, &e))?;
    if has_certificate(&bytes) {
        Ok(bytes)
    } else {
        Err(SourceFile::unreadable(
            &shown,
            &std::io::Error::new(std::io::ErrorKind::InvalidData, "it holds no PEM certificate"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::is_generic_not_configured;

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
