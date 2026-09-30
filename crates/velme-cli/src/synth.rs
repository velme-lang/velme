//! The `ollama` and `external` providers as `velme build` chooses them (`tooling/40` §2.1, R-CLI-12, R-CLI-13,
//! `compiler/22` R-SYNTH-24..29): what they need, where each setting comes from, and the notice that says what is sent
//! (`tooling/41` R-SEC-12).

use velme_diagnostics::{Code, Diagnostic, Span};
use velme_synth::{
    CommandError, External, ExternalCommand, ExternalConfig, OLLAMA_URL, Ollama, OllamaConfig, SynthBackend,
};

use crate::project::Project;

/// A provider and the notice that says what it sends.
pub type Chosen = (Box<dyn SynthBackend>, String);

fn not_configured(message: impl Into<String>, help: &str) -> Diagnostic {
    Diagnostic::new(Code::ProviderNotConfigured, Span::default(), message).with_help(help)
}

/// The model of `--model`, else `VELME_MODEL`, trimmed; empty when neither is set (`tooling/40` §5.2).
pub fn model_name(flag: Option<&str>) -> String {
    flag.map(str::to_owned)
        .or_else(|| std::env::var("VELME_MODEL").ok())
        .map(|m| m.trim().to_owned())
        .unwrap_or_default()
}

/// The `ollama` provider for the model from `--model`, else `VELME_MODEL`. No key is needed; the server is the default
/// local one, since the config file that sets `ollama_url` comes with M6.
pub fn ollama(flag: Option<&str>) -> Result<Chosen, Diagnostic> {
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
    Ok((Box::new(Ollama::new(OllamaConfig::new(model))), notice))
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
    use super::split_words;

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
}
