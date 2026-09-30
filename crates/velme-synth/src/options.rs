//! The `[synthesis]` settings the retry loop and the prompt read (`tooling/40` §5.1, `compiler/22` §4.1, D-44). None of
//! them changes which IR is accepted (INV-1, INV-2); the ones that change what is sent enter `input_version`
//! (R-SYNTH-40).

use serde::{Deserialize, Serialize};
use velme_ir::Fingerprint;

/// How much of the reply schema the prompt carries (R-SYNTH-35).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaInPrompt {
    /// One line per IR node kind and the question object; the schema itself goes only through the provider's
    /// constraint.
    #[default]
    Summary,
    /// The whole schema too.
    Full,
}

/// How a reply spells the IR (R-SYNTH-36).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReplyFormat {
    /// Canonical IR JSON.
    #[default]
    IrJson,
    /// The same tree with every property name and node `kind` tag replaced by a short alias.
    Compact,
}

/// Which earlier replies a retry turn carries (R-SYNTH-37).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetryHistory {
    /// Only the latest failed reply, and the primary diagnostic of each earlier attempt.
    #[default]
    Latest,
    /// Every earlier reply.
    All,
}

/// The settings of one build's synthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthOptions {
    /// The retries after the first attempt, 0..=3 (R-SYNTH-11); `external` defaults to 0 (R-SYNTH-30).
    pub max_retries: u32,
    /// The most provider calls in one build (R-SYNTH-21).
    pub max_calls_per_build: u32,
    /// Stop when two attempts in a row share a cause (R-SYNTH-37).
    pub stop_on_repeat: bool,
    /// Which earlier replies a retry carries.
    pub retry_history: RetryHistory,
    /// The examples the prompt shows, 0..=64 (R-SYNTH-38).
    pub max_prompt_examples: usize,
    /// How much of the schema the prompt shows.
    pub schema_in_prompt: SchemaInPrompt,
    /// How a reply spells the IR.
    pub reply_format: ReplyFormat,
    /// The most time one provider request may take, in seconds (`timeout_secs`); LLM providers only.
    pub timeout_secs: u64,
    /// The most tokens a reply may hold (`max_output_tokens`); `None` is the provider's default (D-110). It changes
    /// neither what is sent as prompt nor which IR is accepted, so it stays out of `input_version`.
    pub max_output_tokens: Option<u32>,
}

impl Default for SynthOptions {
    fn default() -> Self {
        SynthOptions {
            max_retries: 3,
            max_calls_per_build: 50,
            stop_on_repeat: true,
            retry_history: RetryHistory::Latest,
            max_prompt_examples: 8,
            schema_in_prompt: SchemaInPrompt::Summary,
            reply_format: ReplyFormat::IrJson,
            timeout_secs: 60,
            max_output_tokens: None,
        }
    }
}

impl SynthOptions {
    /// The `input_version` of an LLM provider (R-SYNTH-40): `prompt_version`, then the BLAKE3 of the options that change
    /// what is sent. Changing one changes `synthesis_key` and never `contract_key`, so it never re-synthesizes a locked
    /// goal.
    pub fn input_version(&self, prompt_version: &str) -> String {
        let doc = serde_json::json!({
            "schema_in_prompt": match self.schema_in_prompt {
                SchemaInPrompt::Summary => "summary",
                SchemaInPrompt::Full => "full",
            },
            "reply_format": match self.reply_format {
                ReplyFormat::IrJson => "ir-json",
                ReplyFormat::Compact => "compact",
            },
            "retry_history": match self.retry_history {
                RetryHistory::Latest => "latest",
                RetryHistory::All => "all",
            },
            "max_prompt_examples": self.max_prompt_examples,
        });
        let hash = Fingerprint::of(&doc).map(|f| f.to_string()).unwrap_or_default();
        format!("{prompt_version}+{hash}")
    }

    /// The prompt's settings.
    pub fn prompt(&self) -> PromptOptions {
        PromptOptions {
            schema_in_prompt: self.schema_in_prompt,
            reply_format: self.reply_format,
            max_prompt_examples: self.max_prompt_examples,
        }
    }
}

/// The settings that shape the text a request renders to (R-SYNTH-34..38).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptOptions {
    /// How much of the schema the prompt shows.
    pub schema_in_prompt: SchemaInPrompt,
    /// How a reply spells the IR.
    pub reply_format: ReplyFormat,
    /// The examples the prompt shows.
    pub max_prompt_examples: usize,
}

impl Default for PromptOptions {
    fn default() -> Self {
        SynthOptions::default().prompt()
    }
}
