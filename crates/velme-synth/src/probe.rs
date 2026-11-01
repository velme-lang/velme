//! The scarcity probe (`compiler/22` R-SYNTH-65, D-162): before any goal is generated or synthesized, `velme check`
//! draws 64 raw values of every declared domain, seeded from the domain's own declaration hash, and reports a domain
//! whose condition admits fewer than 8 of them (`VL0210`). It needs no provider; `velme-check` only evaluates each
//! predicate (D-164).

use velme_check::Membership;
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Fingerprint, declaration_hash, decode_str, to_canonical_string};
use velme_sema::hir::{DomainId, Program};

use velme_builtins::execution::Limits;

use crate::generate::{Generator, MAX_INPUT_BYTES, Scope, Stream, output, seed};

/// How many raw draws the probe makes per domain (R-SYNTH-65).
pub const PROBE_DRAWS: u64 = 64;

/// The fewest valid draws a domain passes with (R-SYNTH-65).
pub const PROBE_MIN_VALID: usize = 8;

/// One raw probe draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeDraw {
    /// Its canonical JSON; `None` if a nested domain's draw failed, no `Number` holds a drawn point exactly, or the
    /// value can't fit in an input.
    pub value: Option<String>,
    /// Whether it satisfies the domain's whole condition and the input-size limits.
    pub valid: bool,
    /// Whether it was over the 256 KiB limit, checked before its condition, which then isn't evaluated.
    pub oversized: bool,
}

/// One domain's probe (R-SYNTH-65).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// The domain's name.
    pub domain: String,
    /// Its declaration hash (`runtime/32` R-ART-26), whose first eight bytes seed the probe.
    pub declaration_hash: Fingerprint,
    /// The seed: those bytes, little-endian.
    pub seed: u64,
    /// The [`PROBE_DRAWS`] draws, in order.
    pub draws: Vec<ProbeDraw>,
}

impl Probe {
    /// How many draws were valid, duplicates counted.
    pub fn valid(&self) -> usize {
        self.draws.iter().filter(|d| d.valid).count()
    }
}

/// Probes every domain of the checked `program`, whose source is `source`, used or not, in type-name order: a
/// `VL0210` for each scarce one, and a predicate's budget failure with its own code (R-SYNTH-65).
pub fn probe(program: &Program, source: &str) -> Vec<Diagnostic> {
    if program.domains.is_empty() {
        return Vec::new();
    }
    let membership = match Membership::new(program, source) {
        Ok(membership) => membership,
        Err(diagnostic) => return vec![diagnostic],
    };
    let mut ids: Vec<DomainId> = (0..program.domains.len()).map(DomainId).collect();
    ids.sort_by_key(|id| program.domain(*id).map(|d| d.name.as_str()));
    let mut found = Vec::new();
    for id in ids {
        match probe_domain(program, &membership, id) {
            Ok(probe) if probe.valid() < PROBE_MIN_VALID => {
                let span = program.domain(id).map(|d| d.span).unwrap_or_default();
                let mut d = Diagnostic::new(
                    Code::TooFewValidValues,
                    span,
                    format!(
                        "Domain `{}` produced only {}/{PROBE_DRAWS} valid values; at least {PROBE_MIN_VALID} are needed. \
                         Loosen its condition.",
                        probe.domain,
                        probe.valid()
                    ),
                );
                // Scarce only for size: no condition is to blame.
                if probe.draws.iter().all(|d| d.valid || d.oversized) {
                    d = d.with_note(format!(
                        "its values can't fit in a {} KiB input",
                        MAX_INPUT_BYTES / 1024
                    ));
                }
                found.push(d);
            }
            Ok(_) => {}
            Err(diagnostic) => found.push(diagnostic),
        }
    }
    found
}

/// The probe of the domain `id`: [`PROBE_DRAWS`] raw simple-valid draws, attempt `i` on the stream seeded with the
/// `(i + 1)`-th output of the stream seeded from the declaration hash (§7.3). The domain's own condition is evaluated
/// once per draw, never retried, and only on a value within the size limits; a nested field domain is drawn with its
/// retries (R-SYNTH-63). A predicate's budget failure, alone or past the probe's total (R-SYNTH-67), is the error, with
/// its code.
pub fn probe_domain(program: &Program, membership: &Membership, id: DomainId) -> Result<Probe, Diagnostic> {
    probe_domain_within(program, membership, id, Limits::SYSTEM.fuel)
}

/// [`probe_domain`] with each predicate evaluation's fuel at most `fuel`, and the probe's total 64 × `fuel`: for tests
/// of R-SYNTH-67, which a system budget makes too slow to reach.
#[doc(hidden)]
pub fn probe_domain_within(
    program: &Program,
    membership: &Membership,
    id: DomainId,
    fuel: u64,
) -> Result<Probe, Diagnostic> {
    let domain = program.domain(id).ok_or_else(Diagnostic::internal_error)?;
    let declaration_hash = declaration_hash(program, id)?;
    let seed = seed(declaration_hash);
    let mut generator = Generator::new(program, Some(membership), Scope::Probe, fuel);
    let mut draws = Vec::with_capacity(PROBE_DRAWS as usize);
    for attempt in 0..PROBE_DRAWS {
        let mut stream = Stream::new(output(seed, attempt + 1));
        // The size limits are the value's alone (R-SYNTH-65).
        let mut room = MAX_INPUT_BYTES;
        generator.oversized = false;
        let draw = match generator.simple_draw(id, &domain.base, &mut stream, &mut room)? {
            None => ProbeDraw {
                value: None,
                valid: false,
                oversized: generator.oversized,
            },
            Some(value) => {
                let text = to_canonical_string(&value).map_err(|_| Diagnostic::internal_error())?;
                let oversized = text.len() > MAX_INPUT_BYTES;
                let fits = !oversized && decode_str(&text, &domain.base, program).is_ok();
                let valid = fits && generator.admitted(id, &domain.base, &value)?;
                ProbeDraw {
                    value: Some(text),
                    valid,
                    oversized,
                }
            }
        };
        draws.push(draw);
    }
    Ok(Probe {
        domain: domain.name.clone(),
        declaration_hash,
        seed,
        draws,
    })
}
