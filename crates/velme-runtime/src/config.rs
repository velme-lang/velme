//! `velme.toml` and the user-level config file, read and validated (`tooling/40` §5.1, R-CLI-11, R-CLI-25, D-105). This
//! module only parses text; finding the files and reading them is the CLI's, so every message about a bad value is
//! composed here (R-CLI-09) and names the key, the file, what was expected and what was found.

use velme_diagnostics::{Code, Diagnostic, Span};
use velme_synth::{ReplyFormat, RetryHistory, SchemaInPrompt};

/// The file name a project's configuration has (`tooling/40` §5.1), which also marks the project root.
pub const PROJECT_FILE: &str = "velme.toml";

/// The most `max_calls_per_build` any config may name.
const MAX_CALLS: i64 = 100_000;
/// The most `timeout_secs` (and `external_timeout_secs`) any config may name.
const MAX_TIMEOUT: i64 = 3_600;
/// The most `max_output_tokens` any config may name.
const MAX_TOKENS: i64 = 1_000_000;

/// The project's `[budget]`: defaults that can only tighten the system caps (D-8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BudgetConfig {
    /// `cpu`, in milliseconds.
    pub cpu_ms: Option<u64>,
    /// `memory`, in bytes.
    pub memory_bytes: Option<u64>,
    /// `calls`.
    pub calls: Option<u64>,
    /// `depth`.
    pub depth: Option<u64>,
}

impl BudgetConfig {
    /// The limits a goal's `budget` line lowers: the system caps, lowered by this `[budget]` and never raised past them
    /// (D-8, R-ART-23).
    pub fn effective(&self) -> velme_sema::hir::Budget {
        use velme_builtins::limits::FUEL_PER_MS;
        let system = velme_sema::hir::Budget::SYSTEM;
        let lower = |own: Option<u64>, cap: u64| own.map_or(cap, |own| own.min(cap));
        velme_sema::hir::Budget {
            max_fuel: lower(self.cpu_ms.map(|ms| ms.saturating_mul(FUEL_PER_MS)), system.max_fuel),
            max_memory: lower(self.memory_bytes, system.max_memory),
            max_goal_calls: lower(self.calls, system.max_goal_calls),
            max_call_depth: lower(self.depth, system.max_call_depth),
        }
    }
}

/// What a project's `velme.toml` sets (`tooling/40` §5.1). Every setting is optional; the service URLs are never here
/// (R-CLI-13).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    /// `[synthesis] provider`.
    pub provider: Option<String>,
    /// `[synthesis] model`.
    pub model: Option<String>,
    /// `[synthesis] retry_model`.
    pub retry_model: Option<String>,
    /// `[synthesis] max_retries`, 0..=3.
    pub max_retries: Option<u32>,
    /// `[synthesis] timeout_secs`.
    pub timeout_secs: Option<u64>,
    /// `[synthesis] max_output_tokens`.
    pub max_output_tokens: Option<u32>,
    /// `[synthesis] max_calls_per_build`.
    pub max_calls_per_build: Option<u32>,
    /// `[synthesis] replay_dir`, relative and inside the project (R-CLI-18).
    pub replay_dir: Option<String>,
    /// `[synthesis] prompt_cache`.
    pub prompt_cache: Option<bool>,
    /// `[synthesis] schema_in_prompt`.
    pub schema_in_prompt: Option<SchemaInPrompt>,
    /// `[synthesis] reply_format`.
    pub reply_format: Option<ReplyFormat>,
    /// `[synthesis] retry_history`.
    pub retry_history: Option<RetryHistory>,
    /// `[synthesis] stop_on_repeat`.
    pub stop_on_repeat: Option<bool>,
    /// `[synthesis] max_prompt_examples`, 0..=64.
    pub max_prompt_examples: Option<usize>,
    /// `[budget]`.
    pub budget: BudgetConfig,
}

/// What the user-level config sets: where plans go and what a build may spend, and nothing else (D-105).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserConfig {
    /// `external_url`.
    pub external_url: Option<String>,
    /// `external_ca_file`, an absolute path.
    pub external_ca_file: Option<String>,
    /// `external_timeout_secs`.
    pub external_timeout_secs: Option<u64>,
    /// `ollama_url`.
    pub ollama_url: Option<String>,
    /// `allowed_models`.
    pub allowed_models: Option<Vec<String>>,
    /// The ceiling on `max_calls_per_build`.
    pub max_calls_per_build: Option<u32>,
    /// The ceiling on `max_retries`.
    pub max_retries: Option<u32>,
    /// The ceiling on `max_output_tokens`.
    pub max_output_tokens: Option<u32>,
}

impl ProjectConfig {
    /// The settings in `text`, the content of the file shown as `file`; `VL0902` for bad TOML, an unknown key, a wrong type
    /// or an out-of-range value, and for a service URL, which only the user may name (R-CLI-13).
    pub fn parse(text: &str, file: &str) -> Result<ProjectConfig, Diagnostic> {
        let mut r = Reader::new(text, file)?;
        let mut config = ProjectConfig::default();
        let mut project = r.table("project")?;
        r.string(&mut project, "project", "language")?;
        r.finish(&project, "project")?;
        let mut s = r.table("synthesis")?;
        for url in ["external_url", "ollama_url"] {
            if s.contains_key(url) {
                return Err(r.service_url(url));
            }
        }
        config.provider = r
            .choice(
                &mut s,
                "synthesis",
                "provider",
                &[
                    ("anthropic", "anthropic"),
                    ("ollama", "ollama"),
                    ("external", "external"),
                    ("replay", "replay"),
                    ("scripted", "scripted"),
                ],
            )?
            .map(str::to_owned);
        config.model = r.string(&mut s, "synthesis", "model")?;
        config.retry_model = r.string(&mut s, "synthesis", "retry_model")?;
        config.max_retries = r.int(&mut s, "synthesis", "max_retries", 0, 3)?.map(to_u32);
        config.timeout_secs = r.int(&mut s, "synthesis", "timeout_secs", 1, MAX_TIMEOUT)?.map(to_u64);
        config.max_output_tokens = r
            .int(&mut s, "synthesis", "max_output_tokens", 1, MAX_TOKENS)?
            .map(to_u32);
        config.max_calls_per_build = r
            .int(&mut s, "synthesis", "max_calls_per_build", 1, MAX_CALLS)?
            .map(to_u32);
        config.replay_dir = r.inside(&mut s, "synthesis", "replay_dir")?;
        config.prompt_cache = r.boolean(&mut s, "synthesis", "prompt_cache")?;
        config.schema_in_prompt = r.choice(
            &mut s,
            "synthesis",
            "schema_in_prompt",
            &[("summary", SchemaInPrompt::Summary), ("full", SchemaInPrompt::Full)],
        )?;
        config.reply_format = r.choice(
            &mut s,
            "synthesis",
            "reply_format",
            &[("ir-json", ReplyFormat::IrJson), ("compact", ReplyFormat::Compact)],
        )?;
        config.retry_history = r.choice(
            &mut s,
            "synthesis",
            "retry_history",
            &[("latest", RetryHistory::Latest), ("all", RetryHistory::All)],
        )?;
        config.stop_on_repeat = r.boolean(&mut s, "synthesis", "stop_on_repeat")?;
        config.max_prompt_examples = r
            .int(&mut s, "synthesis", "max_prompt_examples", 0, 64)?
            .map(|n| usize::try_from(n).unwrap_or(0));
        r.finish(&s, "synthesis")?;
        let mut b = r.table("budget")?;
        config.budget = BudgetConfig {
            cpu_ms: r.quantity(&mut b, "cpu", &[("ms", 1)])?,
            memory_bytes: r.quantity(&mut b, "memory", &[("kb", 1024), ("mb", 1024 * 1024)])?,
            calls: r.int(&mut b, "budget", "calls", 1, i64::MAX)?.map(to_u64),
            depth: r.int(&mut b, "budget", "depth", 1, i64::MAX)?.map(to_u64),
        };
        r.finish(&b, "budget")?;
        let mut a = r.table("artifacts")?;
        if let Some(dir) = a.remove("dir") {
            // The store stays at `.velme/artifacts` (R-CLI-18); a path outside the project is wrong before it is unavailable.
            if let toml::Value::String(text) = &dir
                && !inside_project(text)
            {
                return Err(r.bad("artifacts.dir", "a relative path inside the project", &describe(&dir)));
            }
            return Err(Diagnostic::new(
                Code::InvalidInput,
                Span::default(),
                format!("`artifacts.dir` in `{}` isn't available yet.", r.file),
            )
            .with_help("artifacts are kept in `.velme/artifacts`; remove the key"));
        }
        r.finish(&a, "artifacts")?;
        r.finish_root()?;
        Ok(config)
    }
}

impl UserConfig {
    /// The settings in `text`, the content of the user-level file shown as `file`; the same `VL0902` as
    /// [`ProjectConfig::parse`], and any key beyond the eight the user-level file holds is one (D-105).
    pub fn parse(text: &str, file: &str) -> Result<UserConfig, Diagnostic> {
        let mut r = Reader::new(text, file)?;
        let mut s = r.table("synthesis")?;
        let config = UserConfig {
            external_url: r.string(&mut s, "synthesis", "external_url")?,
            external_ca_file: r.absolute(&mut s, "synthesis", "external_ca_file")?,
            external_timeout_secs: r
                .int(&mut s, "synthesis", "external_timeout_secs", 1, MAX_TIMEOUT)?
                .map(to_u64),
            ollama_url: r.string(&mut s, "synthesis", "ollama_url")?,
            allowed_models: r.strings(&mut s, "synthesis", "allowed_models")?,
            max_calls_per_build: r
                .int(&mut s, "synthesis", "max_calls_per_build", 1, MAX_CALLS)?
                .map(to_u32),
            max_retries: r.int(&mut s, "synthesis", "max_retries", 0, 3)?.map(to_u32),
            max_output_tokens: r
                .int(&mut s, "synthesis", "max_output_tokens", 1, MAX_TOKENS)?
                .map(to_u32),
        };
        r.finish(&s, "synthesis")?;
        r.finish_root()?;
        Ok(config)
    }
}

/// A range-checked integer as `u32`: the checks keep it in range, so the fallback is never used.
fn to_u32(n: i64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// A range-checked integer as `u64`.
fn to_u64(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// The parsed file and its name, for the messages.
struct Reader<'a> {
    file: &'a str,
    root: toml::Table,
}

impl<'a> Reader<'a> {
    fn new(text: &str, file: &'a str) -> Result<Self, Diagnostic> {
        match text.parse::<toml::Table>() {
            Ok(root) => Ok(Reader { file, root }),
            Err(error) => {
                let line = error
                    .span()
                    .map(|span| text.get(..span.start).unwrap_or_default().matches('\n').count() + 1);
                let key = line.map_or_else(|| "the file".to_owned(), |line| format!("line {line}"));
                let found = format!("\"{}\"", error.message());
                Err(Self::bad_at(file, &key, "valid TOML", &found))
            }
        }
    }

    fn bad_at(file: &str, key: &str, expected: &str, found: &str) -> Diagnostic {
        Diagnostic::new(
            Code::InvalidInput,
            Span::default(),
            format!("`{key}` in `{file}` should be {expected}, but got {found}."),
        )
    }

    fn bad(&self, key: &str, expected: &str, found: &str) -> Diagnostic {
        Self::bad_at(self.file, key, expected, found)
    }

    /// The table `name` of the root (empty if there is none), taken out of it.
    fn table(&mut self, name: &str) -> Result<toml::Table, Diagnostic> {
        match self.root.remove(name) {
            None => Ok(toml::Table::new()),
            Some(toml::Value::Table(table)) => Ok(table),
            Some(other) => Err(self.bad(name, "a table", &describe(&other))),
        }
    }

    /// An error for any key left in `table`, the first in name order.
    fn finish(&self, table: &toml::Table, name: &str) -> Result<(), Diagnostic> {
        match table.keys().next() {
            None => Ok(()),
            Some(key) => Err(self.bad(&format!("{name}.{key}"), "a setting Velme knows", "an unknown key")),
        }
    }

    fn finish_root(&self) -> Result<(), Diagnostic> {
        match self.root.keys().next() {
            None => Ok(()),
            Some(key) => Err(self.bad(key, "a section Velme knows", "an unknown key")),
        }
    }

    /// The `VL0902` for a service URL in a project's file: only the user may name where plans go (R-CLI-13, D-105).
    fn service_url(&self, key: &str) -> Diagnostic {
        self.bad(&format!("synthesis.{key}"), "left out of a project's file", "a value")
            .with_help(
                "a project can't say where your plans go: use the flag, the environment variable or your user-level config",
            )
    }

    fn string(&self, table: &mut toml::Table, section: &str, key: &str) -> Result<Option<String>, Diagnostic> {
        match table.remove(key) {
            None => Ok(None),
            Some(toml::Value::String(text)) if !text.trim().is_empty() => Ok(Some(text)),
            Some(other) => Err(self.bad(&format!("{section}.{key}"), "a non-empty string", &describe(&other))),
        }
    }

    /// A string that is an absolute path.
    fn absolute(&self, table: &mut toml::Table, section: &str, key: &str) -> Result<Option<String>, Diagnostic> {
        let Some(text) = self.string(table, section, key)? else {
            return Ok(None);
        };
        if std::path::Path::new(&text).is_absolute() || text.starts_with('/') {
            Ok(Some(text))
        } else {
            Err(self.bad(
                &format!("{section}.{key}"),
                "an absolute path",
                &describe(&toml::Value::String(text)),
            ))
        }
    }

    /// A string that is a relative path staying inside the project (R-CLI-18).
    fn inside(&self, table: &mut toml::Table, section: &str, key: &str) -> Result<Option<String>, Diagnostic> {
        let Some(text) = self.string(table, section, key)? else {
            return Ok(None);
        };
        if inside_project(&text) {
            Ok(Some(text))
        } else {
            Err(self.bad(
                &format!("{section}.{key}"),
                "a relative path inside the project",
                &describe(&toml::Value::String(text)),
            ))
        }
    }

    fn strings(&self, table: &mut toml::Table, section: &str, key: &str) -> Result<Option<Vec<String>>, Diagnostic> {
        let path = format!("{section}.{key}");
        match table.remove(key) {
            None => Ok(None),
            Some(toml::Value::Array(items)) => {
                let mut out = Vec::new();
                for item in items {
                    match item {
                        toml::Value::String(text) if !text.trim().is_empty() => out.push(text),
                        other => return Err(self.bad(&path, "a list of non-empty strings", &describe(&other))),
                    }
                }
                Ok(Some(out))
            }
            Some(other) => Err(self.bad(&path, "a list of non-empty strings", &describe(&other))),
        }
    }

    fn boolean(&self, table: &mut toml::Table, section: &str, key: &str) -> Result<Option<bool>, Diagnostic> {
        match table.remove(key) {
            None => Ok(None),
            Some(toml::Value::Boolean(b)) => Ok(Some(b)),
            Some(other) => Err(self.bad(&format!("{section}.{key}"), "true or false", &describe(&other))),
        }
    }

    fn int(
        &self,
        table: &mut toml::Table,
        section: &str,
        key: &str,
        min: i64,
        max: i64,
    ) -> Result<Option<i64>, Diagnostic> {
        let path = format!("{section}.{key}");
        match table.remove(key) {
            None => Ok(None),
            Some(toml::Value::Integer(n)) if (min..=max).contains(&n) => Ok(Some(n)),
            Some(toml::Value::Integer(n)) => {
                let expected = if max == i64::MAX {
                    format!("a whole number of at least {min}")
                } else {
                    format!("a whole number from {min} to {max}")
                };
                Err(self.bad(&path, &expected, &n.to_string()))
            }
            Some(other) => Err(self.bad(&path, "a whole number", &describe(&other))),
        }
    }

    /// The word among `words` that `key` holds.
    fn choice<T: Copy>(
        &self,
        table: &mut toml::Table,
        section: &str,
        key: &str,
        words: &[(&str, T)],
    ) -> Result<Option<T>, Diagnostic> {
        let Some(text) = self.string(table, section, key)? else {
            return Ok(None);
        };
        match words.iter().find(|(word, _)| *word == text) {
            Some((_, value)) => Ok(Some(*value)),
            None => {
                let list: Vec<&str> = words.iter().map(|(word, _)| *word).collect();
                Err(self.bad(
                    &format!("{section}.{key}"),
                    &format!("one of {}", list.join(" or ")),
                    &describe(&toml::Value::String(text)),
                ))
            }
        }
    }

    /// A budget value such as `"50ms"`: digits and one of `units`, as bytes or milliseconds (D-8, D-78).
    fn quantity(&self, table: &mut toml::Table, key: &str, units: &[(&str, u64)]) -> Result<Option<u64>, Diagnostic> {
        let path = format!("budget.{key}");
        let names: Vec<String> = units.iter().map(|(unit, _)| format!("`{unit}`")).collect();
        let expected = format!(
            "a whole number with the unit {}, such as \"16{}\"",
            names.join(" or "),
            first(units)
        );
        match table.remove(key) {
            None => Ok(None),
            Some(toml::Value::String(text)) => {
                let digits = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
                let unit = text.get(digits.len()..).unwrap_or_default();
                let scale = units.iter().find(|(u, _)| *u == unit).map(|(_, scale)| *scale);
                match (digits.parse::<u64>(), scale) {
                    (Ok(n), Some(scale)) if n >= 1 && digits.bytes().all(|b| b.is_ascii_digit()) => {
                        n.checked_mul(scale).map(Some).ok_or_else(|| {
                            self.bad(
                                &path,
                                "a quantity Velme can hold",
                                &describe(&toml::Value::String(text.clone())),
                            )
                        })
                    }
                    _ => Err(self.bad(&path, &expected, &describe(&toml::Value::String(text)))),
                }
            }
            Some(other) => Err(self.bad(&path, &expected, &describe(&other))),
        }
    }
}

fn first(units: &[(&str, u64)]) -> &'static str {
    match units.first() {
        Some(("ms", _)) => "ms",
        _ => "mb",
    }
}

/// Whether `path` is relative and stays inside the project: no leading `/` or `\`, no drive, no `..` (R-CLI-18).
pub fn inside_project(path: &str) -> bool {
    let p = std::path::Path::new(path);
    !path.is_empty()
        && !p.is_absolute()
        && !path.starts_with(['/', '\\'])
        && path.as_bytes().get(1) != Some(&b':')
        && path.split(['/', '\\']).all(|part| part != "..")
}

/// What a value is, in words for a message.
fn describe(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => format!("{text:?}"),
        toml::Value::Integer(n) => n.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(_) => "a date".to_owned(),
        toml::Value::Array(_) => "a list".to_owned(),
        toml::Value::Table(_) => "a table".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(result: Result<impl std::fmt::Debug, Diagnostic>) -> String {
        result.expect_err("an error").message
    }

    #[test]
    fn an_empty_file_sets_nothing() {
        assert_eq!(ProjectConfig::parse("", "velme.toml"), Ok(ProjectConfig::default()));
        assert_eq!(UserConfig::parse("", "config.toml"), Ok(UserConfig::default()));
    }

    #[test]
    fn the_documented_project_file_is_read() {
        let text = "[project]\nlanguage = \"velme/0.1\"\n[synthesis]\nprovider = \"ollama\"\nmodel = \"m\"\nmax_retries = 2\n\
                    reply_format = \"compact\"\nmax_prompt_examples = 4\n[budget]\ncpu = \"50ms\"\nmemory = \"16mb\"\n\
                    calls = 128\n";
        let config = ProjectConfig::parse(text, "velme.toml").expect("valid");
        assert_eq!(config.provider.as_deref(), Some("ollama"));
        assert_eq!(config.max_retries, Some(2));
        assert_eq!(config.reply_format, Some(ReplyFormat::Compact));
        assert_eq!(config.budget.cpu_ms, Some(50));
        assert_eq!(config.budget.memory_bytes, Some(16 * 1024 * 1024));
    }

    /// `[budget]` only lowers the system caps, and a quantity too big to hold is `VL0902`, not unset.
    #[test]
    fn a_budget_lowers_the_caps_and_never_raises_them() {
        let system = velme_sema::hir::Budget::SYSTEM;
        assert_eq!(BudgetConfig::default().effective(), system);
        let big = BudgetConfig {
            cpu_ms: Some(u64::MAX),
            memory_bytes: Some(u64::MAX),
            calls: Some(u64::MAX),
            depth: Some(u64::MAX),
        };
        assert_eq!(big.effective(), system);
        let small = BudgetConfig {
            calls: Some(2),
            ..BudgetConfig::default()
        };
        assert_eq!(small.effective().max_goal_calls, 2);
        assert_eq!(small.effective().max_fuel, system.max_fuel);
        assert!(
            message(ProjectConfig::parse(
                "[budget]\nmemory = \"99999999999999999mb\"\n",
                "velme.toml"
            ))
            .starts_with("`budget.memory` in `velme.toml` should be")
        );
    }

    #[test]
    fn a_wrong_type_or_range_says_what_it_expected_and_found() {
        let wrong = |text| message(ProjectConfig::parse(text, "velme.toml"));
        assert_eq!(
            wrong("[synthesis]\nmax_retries = 9\n"),
            "`synthesis.max_retries` in `velme.toml` should be a whole number from 0 to 3, but got 9."
        );
        assert_eq!(
            wrong("[synthesis]\nmodel = 4\n"),
            "`synthesis.model` in `velme.toml` should be a non-empty string, but got 4."
        );
        assert_eq!(
            wrong("[synthesis]\nreply_format = \"xml\"\n"),
            "`synthesis.reply_format` in `velme.toml` should be one of ir-json or compact, but got \"xml\"."
        );
        assert_eq!(
            wrong("[synthesis]\nnope = 1\n"),
            "`synthesis.nope` in `velme.toml` should be a setting Velme knows, but got an unknown key."
        );
        assert_eq!(
            wrong("[synthesis\n"),
            "`line 1` in `velme.toml` should be valid TOML, but got \"unclosed table, expected `]`\"."
        );
    }

    #[test]
    fn a_service_url_in_a_project_is_refused() {
        for key in ["external_url", "ollama_url"] {
            let error = ProjectConfig::parse(&format!("[synthesis]\n{key} = \"http://127.0.0.1\"\n"), "velme.toml")
                .expect_err("refused");
            assert!(error.message.contains(&format!("synthesis.{key}")), "{}", error.message);
        }
    }

    #[test]
    fn artifact_and_replay_paths_stay_inside_the_project() {
        assert!(inside_project(".velme/artifacts") && inside_project("tests/fixtures/synth"));
        for path in ["/etc", "../outside", "a/../../b", "\\share", "C:/x", ""] {
            assert!(!inside_project(path), "{path}");
        }
    }

    #[test]
    fn the_user_file_holds_only_its_eight_keys() {
        let text = "[synthesis]\nexternal_url = \"https://x.example\"\nexternal_ca_file = \"/etc/ca.pem\"\n\
                    external_timeout_secs = 5\nollama_url = \"http://localhost:1\"\nallowed_models = [\"a\", \"b\"]\n\
                    max_calls_per_build = 3\nmax_retries = 1\nmax_output_tokens = 100\n";
        let config = UserConfig::parse(text, "config.toml").expect("valid");
        assert_eq!(config.allowed_models, Some(vec!["a".to_owned(), "b".to_owned()]));
        assert_eq!(config.max_output_tokens, Some(100));
        assert!(UserConfig::parse("[synthesis]\nmodel = \"x\"\n", "config.toml").is_err());
        assert!(UserConfig::parse("[project]\n", "config.toml").is_err());
        assert!(UserConfig::parse("[synthesis]\nexternal_ca_file = \"rel.pem\"\n", "config.toml").is_err());
    }
}
