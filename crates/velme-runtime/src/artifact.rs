//! The artifact document and its manifest (`runtime/32` §3).

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use velme_ir::{Fingerprint, Goal};
use velme_sema::hir::GoalKind;

/// The artifact format this build reads and writes, the manifest's `format` (`runtime/32` §3, D-86).
pub const ARTIFACT_FORMAT: &str = "velme-artifact/1";

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
    /// The artifact format: an artifact of another format, or of none, is not read (D-86).
    pub format: ArtifactFormat,
    /// The goal's name.
    pub goal: String,
    /// What was synthesized for it.
    pub kind: Kind,
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

/// The manifest's `format`: always [`ARTIFACT_FORMAT`], the only value it reads (D-86).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ArtifactFormat;

impl Serialize for ArtifactFormat {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(ARTIFACT_FORMAT)
    }
}

impl<'de> Deserialize<'de> for ArtifactFormat {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let format = String::deserialize(deserializer)?;
        if format == ARTIFACT_FORMAT {
            return Ok(ArtifactFormat);
        }
        Err(de::Error::custom(format!(
            "it is artifact format `{format}`, but this version of Velme reads `{ARTIFACT_FORMAT}`"
        )))
    }
}

/// What was synthesized for a goal, as the manifest spells it (`runtime/32` §3): the artifact's own names, so a change
/// to the compiler's types can't change a stored artifact's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// No `call` block: the whole body.
    #[serde(rename = "leaf")]
    Leaf,
    /// A `call` block without `result`: the tail (D-5).
    #[serde(rename = "composite")]
    Composite,
    /// A `call` block ending in `result`: nothing (D-4).
    #[serde(rename = "wired")]
    Wired,
}

impl From<GoalKind> for Kind {
    fn from(kind: GoalKind) -> Self {
        match kind {
            GoalKind::Leaf => Kind::Leaf,
            GoalKind::Composite => Kind::Composite,
            GoalKind::Wired => Kind::Wired,
        }
    }
}
