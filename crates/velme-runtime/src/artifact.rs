//! The artifact document and its manifest (`runtime/32` §3).

use serde::{Deserialize, Serialize};
use velme_ir::{Fingerprint, Goal};
use velme_sema::hir::GoalKind;

/// An artifact document as stored: a verified IR goal and the manifest saying how it was produced (`runtime/32` §3).
/// Its canonical JSON is what the store addresses (R-ART-09). A loaded one is untrusted until its IR is validated and
/// its manifest cross-checked (R-ART-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    /// How it was produced.
    pub manifest: Manifest,
    /// The IR goal (`compiler/21` §2).
    pub ir: Goal,
}

/// How an artifact was produced (`runtime/32` §3). Deterministic: no timestamps, hostnames, usernames, token counts or
/// retry counts (R-ART-05), and no other field is accepted on input (AC-ART-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The goal's name.
    pub goal: String,
    /// What was synthesized for it.
    pub kind: GoalKind,
    /// The goal's signature (`runtime/32` §2).
    pub signature: Fingerprint,
    /// The goal's `contract_key` when it was built.
    pub contract_key: Fingerprint,
    /// The `synthesis_key` it was built under.
    pub synthesis_key: Fingerprint,
    /// The language version of the source.
    pub language_version: String,
    /// The compiler's full version, recorded here only (R-ART-04).
    pub compiler_version: String,
    /// The IR version.
    pub ir_version: String,
    /// The builtin catalog version, which is also the standard library version (R-ART-06).
    pub builtins_version: String,
    /// The prompt version, or an external backend's `request_version` (R-ART-21); none for a wired goal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_version: Option<String>,
    /// The provider id: `compiler` for a wired goal (R-ART-07), `external` for an external backend (R-ART-21).
    pub provider: String,
    /// The external backend's name, for `"provider": "external"` only (R-ART-21).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    /// The provider-reported model id; none for a wired goal (R-ART-07).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_version: Option<String>,
    /// The goal's children, mirroring the IR `calls` in source order (R-ART-08).
    pub children: Vec<Child>,
    /// What verification the IR passed.
    pub verification: Verification,
}

/// A child of a composite or wired goal, as the manifest lists it (R-ART-08).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Child {
    /// The `call` binding.
    pub binding: String,
    /// The child goal's name.
    pub goal: String,
    /// The child's signature, never its artifact (D-11).
    pub signature: Fingerprint,
}

/// The verification an artifact passed before it was stored (`compiler/22` §6, R-ART-11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Verification {
    /// `examples:` run.
    pub examples: u64,
    /// Generated inputs the checks ran on.
    pub generated_inputs: u64,
    /// The fingerprint of the whole input set.
    pub input_set: Fingerprint,
    /// The most fuel one run took.
    pub max_fuel_observed: u64,
}
