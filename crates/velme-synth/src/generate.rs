//! Generated test inputs (`compiler/22` §7, D-96): a pure function of the goal's signature, its examples and its
//! `contract_key`, frozen per `language_version` (R-SYNTH-48). Every rule below is the spec's; a change to any of them is
//! a breaking change.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value as Json};
use velme_builtins::execution::{Error, Failure, Limits};
use velme_builtins::{Number, Value};
use velme_check::{Membership, Refused};
use velme_diagnostics::{Code, Diagnostic};
use velme_ir::{Fingerprint, decode_str, decode_value, encode_value, to_canonical_string};
use velme_sema::hir::{DomainId, DomainRule, DomainType, GoalId, Program, Type};

use crate::domain::{Interval, Lengths};

/// The most inputs a goal is checked on, examples included (R-SYNTH-18).
const MAX_INPUTS: usize = 64;
/// The most generated inputs the boundary stage may fill, then the pairwise stage (R-SYNTH-18).
const BOUNDARY_CAP: usize = 16;
const PAIRWISE_CAP: usize = 40;
/// The most random attempts (R-SYNTH-18).
const MAX_RANDOM_ATTEMPTS: u64 = 256;
/// The most bytes of canonical JSON one input may have (R-SYNTH-18): 256 KiB.
pub(crate) const MAX_INPUT_BYTES: usize = 262_144;

/// SplitMix64's increment (`language/14` R-BLT-04).
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// One input to check a candidate on: one value per parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct TestInput {
    /// The canonical JSON of the object mapping each parameter name to its value: the input's identity (§7).
    pub json: String,
    /// One value per parameter, in declaration order.
    pub args: Vec<Value>,
}

/// The inputs of one goal, in the order they are checked (R-SYNTH-15), and how many each stage gave.
#[derive(Debug, Clone, PartialEq)]
pub struct TestInputs {
    /// Examples first, then boundary, pairwise and random inputs.
    pub inputs: Vec<TestInput>,
    /// How many are the goal's `examples:` (R-SYNTH-18).
    pub examples: usize,
    /// How many boundary inputs were accepted.
    pub boundary: usize,
    /// How many pairwise inputs were accepted.
    pub pairwise: usize,
    /// How many random inputs were accepted.
    pub random: usize,
    /// How many times a random domain value was redrawn after a candidate outside its domain (R-SYNTH-63).
    pub retries: u64,
}

/// The inputs to check goal `goal` of `program`, whose source is `source`, on: its `examples` (one value per parameter
/// each), then the generated ones for `contract_key` (§7). A domain predicate that runs out of budget while values are
/// generated, alone or past the goal's total (R-SYNTH-67), ends generation with its own code, placed at the domain
/// (R-SYNTH-63).
pub fn test_inputs(
    program: &Program,
    goal: GoalId,
    source: &str,
    contract_key: Fingerprint,
    examples: &[Vec<Value>],
) -> Result<TestInputs, Diagnostic> {
    test_inputs_within(program, goal, source, contract_key, examples, Limits::SYSTEM.fuel)
}

/// [`test_inputs`] with each predicate evaluation's fuel at most `fuel`, and the goal's total 256 × `fuel`: for tests
/// of R-SYNTH-67, which a system budget makes too slow to reach.
#[doc(hidden)]
pub fn test_inputs_within(
    program: &Program,
    goal: GoalId,
    source: &str,
    contract_key: Fingerprint,
    examples: &[Vec<Value>],
    fuel: u64,
) -> Result<TestInputs, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
    // Only a program with domains has predicates to evaluate (R-SYNTH-63).
    let membership = if program.domains.is_empty() {
        None
    } else {
        Some(Membership::new(program, source)?)
    };
    let mut generator = Generator::new(program, membership.as_ref(), Scope::Goal, fuel);
    let names: Vec<&str> = target.params.iter().map(|p| p.name.as_str()).collect();
    let types: Vec<&Type> = target.params.iter().map(|p| &p.ty).collect();
    let mut set = Accepted {
        program,
        names: &names,
        types: &types,
        seen: HashSet::new(),
        inputs: Vec::new(),
    };
    for example in examples {
        let args = example
            .iter()
            .map(|value| {
                let text = encode_value(value).map_err(|_| Diagnostic::internal_error())?;
                serde_json::from_str(&text).map_err(|_| Diagnostic::internal_error())
            })
            .collect::<Result<Vec<Json>, Diagnostic>>()?;
        // Examples are never dropped and never de-duplicated among themselves (R-SYNTH-18).
        let json = object(&names, &args)?;
        set.seen.insert(json.clone());
        set.inputs.push(TestInput {
            json,
            args: example.clone(),
        });
    }
    let e = set.inputs.len();
    let slots = MAX_INPUTS.saturating_sub(e);
    let generated = |set: &Accepted<'_>| set.inputs.len() - e;

    // Stage 2: the diagonal of the parameters' boundary sets; none if one of them is empty (R-SYNTH-64).
    let boundary_sets = types
        .iter()
        .map(|ty| generator.values(ty, Kind::Boundary))
        .collect::<Result<Vec<_>, _>>()?;
    for row in diagonal(&boundary_sets) {
        if generated(&set) >= slots.min(BOUNDARY_CAP) {
            break;
        }
        set.offer_slots(row)?;
    }
    let boundary = generated(&set);

    // Stage 3: the pairwise rows over the small sets; none if one of them is empty (R-SYNTH-64).
    let small = types
        .iter()
        .map(|ty| generator.values(ty, Kind::Small))
        .collect::<Result<Vec<_>, _>>()?;
    let sizes: Vec<usize> = small.iter().map(Vec::len).collect();
    for row in pairwise(&sizes) {
        if generated(&set) >= slots.min(PAIRWISE_CAP) {
            break;
        }
        let args = row
            .iter()
            .zip(&small)
            .filter_map(|(i, options)| options.get(*i).cloned())
            .collect();
        set.offer_slots(args)?;
    }
    let pairwise_count = generated(&set) - boundary;

    // Stage 4: per-attempt streams seeded from the contract key. A value draw that fails skips its attempt, which
    // still counts (R-SYNTH-63).
    let seed = seed(contract_key);
    // The input object's braces, quoted names, colons and commas, before any value.
    let frame = 1 + names.iter().map(|n| n.len() + 4).sum::<usize>().max(1);
    let mut attempt = 0;
    while generated(&set) < slots && attempt < MAX_RANDOM_ATTEMPTS {
        let mut stream = Stream::new(output(seed, attempt + 1));
        attempt += 1;
        let mut room = MAX_INPUT_BYTES.saturating_sub(frame);
        let mut args = Vec::with_capacity(types.len());
        for ty in &types {
            match generator.draw(ty, &mut stream, &mut room)? {
                Some(value) => args.push(value),
                None => break,
            }
        }
        if args.len() == types.len() {
            set.offer(args)?;
        }
    }
    let random = generated(&set) - boundary - pairwise_count;
    Ok(TestInputs {
        inputs: set.inputs,
        examples: e,
        boundary,
        pairwise: pairwise_count,
        random,
        retries: generator.retries,
    })
}

/// The boundary and small sets `B(ty)` and `S(ty)` of a type of `program`, whose source is `source`, as canonical JSON
/// (§7.1, R-SYNTH-62–64), for golden tests of the generator; a value that can't fit in an input shows as `oversized`.
#[doc(hidden)]
pub fn value_sets(program: &Program, source: &str, ty: &Type) -> Result<(Vec<String>, Vec<String>), Diagnostic> {
    let membership = Membership::new(program, source)?;
    let mut generator = Generator::new(program, Some(&membership), Scope::Goal, Limits::SYSTEM.fuel);
    let mut shown = |kind| -> Result<Vec<String>, Diagnostic> {
        generator
            .values(ty, kind)?
            .iter()
            .map(|slot| match slot {
                Some((v, _)) => to_canonical_string(v).map_err(|_| Diagnostic::internal_error()),
                None => Ok("oversized".to_owned()),
            })
            .collect()
    };
    Ok((shown(Kind::Boundary)?, shown(Kind::Small)?))
}

/// The inputs accepted so far.
struct Accepted<'a> {
    program: &'a Program,
    names: &'a [&'a str],
    types: &'a [&'a Type],
    seen: HashSet<String>,
    inputs: Vec<TestInput>,
}

impl Accepted<'_> {
    /// Takes the candidate row of set values, unless one of them can't fit in an input (R-SYNTH-18), as [`Self::offer`].
    fn offer_slots(&mut self, row: Vec<Slot>) -> Result<(), Diagnostic> {
        match row.into_iter().map(|slot| slot.map(|(v, _)| v)).collect() {
            Some(args) => self.offer(args),
            None => Ok(()),
        }
    }

    /// Takes the candidate `args` unless its canonical JSON equals an earlier input's, is over 256 KiB, or input
    /// decoding would refuse it (R-SYNTH-18).
    fn offer(&mut self, args: Vec<Json>) -> Result<(), Diagnostic> {
        let json = object(self.names, &args)?;
        if json.len() > MAX_INPUT_BYTES || self.seen.contains(&json) {
            return Ok(());
        }
        let mut values = Vec::with_capacity(args.len());
        for (arg, ty) in args.iter().zip(self.types) {
            let text = to_canonical_string(arg).map_err(|_| Diagnostic::internal_error())?;
            match decode_str(&text, ty, self.program) {
                Ok(value) => values.push(value),
                Err(_) => return Ok(()),
            }
        }
        self.seen.insert(json.clone());
        self.inputs.push(TestInput { json, args: values });
        Ok(())
    }
}

/// The canonical JSON of the object mapping each parameter name to its value.
fn object(names: &[&str], args: &[Json]) -> Result<String, Diagnostic> {
    let map: Map<String, Json> = names
        .iter()
        .zip(args)
        .map(|(name, value)| ((*name).to_owned(), value.clone()))
        .collect();
    to_canonical_string(&Json::Object(map)).map_err(|_| Diagnostic::internal_error())
}

/// A JSON number written as `text`, exactly.
fn number(text: &str) -> Json {
    serde_json::from_str(text).unwrap_or(Json::Null)
}

/// Which value set (§7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Boundary,
    Small,
}

/// The retries after the initial candidate a random domain value gets (R-SYNTH-63): at most 17 candidates.
const RETRIES: u64 = 16;

/// The most members of a closed set its boundary set takes (R-SYNTH-62).
const SET_BOUNDARY: usize = 16;

/// A value of a finite set and the bytes of its canonical JSON, or `None` for one that can't fit in an input: it keeps
/// its place, so the others keep theirs, and every input holding it is skipped (R-SYNTH-18, R-SYNTH-62).
pub(crate) type Slot = Option<(Json, usize)>;

/// What one generator's predicate evaluations may spend in total, as multiples of one evaluation's `max_fuel`
/// (R-SYNTH-67, D-167).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    /// One domain's probe: 64 × `max_fuel`.
    Probe,
    /// One goal's generation: 256 × `max_fuel`.
    Goal,
}

impl Scope {
    fn factor(self) -> u64 {
        match self {
            Scope::Probe => 64,
            Scope::Goal => 256,
        }
    }

    fn what(self) -> &'static str {
        match self {
            Scope::Probe => "checking one domain for scarcity",
            Scope::Goal => "generating one goal's test inputs",
        }
    }
}

/// The value sets and random values of one program's types (§7.1, §7.3, §7.5): each domain's finite sets made and
/// filtered once, and the retries its random values took.
pub(crate) struct Generator<'a> {
    program: &'a Program,
    /// The program's domain predicates; `None` when it has no domains.
    membership: Option<&'a Membership>,
    sets: HashMap<(DomainId, Kind), Vec<Slot>>,
    /// Redraws after a candidate outside its domain (R-SYNTH-63).
    pub(crate) retries: u64,
    /// One predicate evaluation's fuel: the system's, but for tests.
    fuel: u64,
    /// Which total the evaluations share, and what they have spent of it (R-SYNTH-67).
    scope: Scope,
    spent: u64,
    /// Whether a value was refused for its size since this was last cleared.
    pub(crate) oversized: bool,
}

impl<'a> Generator<'a> {
    pub(crate) fn new(program: &'a Program, membership: Option<&'a Membership>, scope: Scope, fuel: u64) -> Self {
        Generator {
            program,
            membership,
            sets: HashMap::new(),
            retries: 0,
            fuel: fuel.min(Limits::SYSTEM.fuel),
            scope,
            spent: 0,
            oversized: false,
        }
    }

    /// The ordered set `B(ty)` or `S(ty)` (§7.1), domains filtered by their conditions (R-SYNTH-62–64). A composite is
    /// sized from its parts before it is built.
    pub(crate) fn values(&mut self, ty: &Type, kind: Kind) -> Result<Vec<Slot>, Diagnostic> {
        let boundary = kind == Kind::Boundary;
        Ok(match ty {
            Type::Domain(id, base) => {
                if let Some(set) = self.sets.get(&(*id, kind)) {
                    return Ok(set.clone());
                }
                let set = self.domain_values(*id, base, kind)?;
                self.sets.insert((*id, kind), set.clone());
                set
            }
            Type::Number => number_set(kind).iter().map(|n| scalar(number(n))).collect(),
            Type::Boolean => vec![scalar(Json::Bool(true)), scalar(Json::Bool(false))],
            Type::Text => text_set(kind)
                .iter()
                .map(|t| scalar(Json::String((*t).to_owned())))
                .collect(),
            Type::Nothing | Type::Error => vec![scalar(Json::Null)],
            Type::Optional(inner) => {
                let mut all = vec![scalar(Json::Null)];
                all.extend(self.values(inner, kind)?);
                all
            }
            Type::List(inner) => {
                let item = self.values(inner, kind)?;
                let at = |i: usize| item.get(i % item.len().max(1)).cloned().flatten();
                if item.is_empty() {
                    // An empty item set leaves the empty list (R-SYNTH-64).
                    vec![list(Vec::new())]
                } else if boundary {
                    vec![
                        list(Vec::new()),
                        list(vec![at(0)]),
                        list(vec![at(0), at(1), at(2)]),
                        list(vec![at(0), at(0)]),
                    ]
                } else {
                    vec![list(Vec::new()), list(vec![at(0)]), list(vec![at(0), at(1)])]
                }
            }
            Type::Record(id) => {
                let Some(record) = self.program.record(*id) else {
                    return Ok(vec![scalar(Json::Null)]);
                };
                let sets = record
                    .fields
                    .iter()
                    .map(|f| self.values(&f.ty, kind))
                    .collect::<Result<Vec<_>, _>>()?;
                let names: Vec<&str> = record.fields.iter().map(|f| f.name.as_str()).collect();
                diagonal(&sets)
                    .into_iter()
                    .map(|row| object_slot(&names, row))
                    .collect()
            }
        })
    }

    /// A domain's finite set (R-SYNTH-62–63): a closed set's first members; else the base set projected into the
    /// simple constraints with the edges appended, de-duplicated, then filtered by the whole condition in order. A
    /// candidate over the size limit is dropped before its condition is evaluated: text too many scalars long is never
    /// built ([`oversized`]), and the bytes of the rest are measured here.
    fn domain_values(&mut self, id: DomainId, base: &Type, kind: Kind) -> Result<Vec<Slot>, Diagnostic> {
        let domain = self.program.domain(id).ok_or_else(Diagnostic::internal_error)?;
        let candidates: Vec<Json> = match (&domain.rule, base) {
            (DomainRule::OneOf { members }, _) => {
                let take = if kind == Kind::Boundary { SET_BOUNDARY } else { 2 };
                return Ok(members
                    .iter()
                    .take(take)
                    .map(|m| scalar(Json::String(m.clone())))
                    .collect());
            }
            (DomainRule::Where { .. }, Type::Number) => {
                let interval = Interval::of(&domain.constraints)?;
                let mut all: Vec<Number> = number_set(kind)
                    .iter()
                    .filter_map(|n| Number::parse(n))
                    .map(|n| interval.project(n))
                    .collect();
                all.extend(interval.lower_edge());
                all.extend(interval.upper_edge());
                all.iter().map(|n| number(&n.to_string())).collect()
            }
            (DomainRule::Where { .. }, Type::Text) => {
                let lengths = Lengths::of(&domain.constraints)?;
                let mut all: Vec<Json> = text_set(kind)
                    .iter()
                    .filter_map(|t| projected_text(t, &lengths))
                    .collect();
                if domain.constraints.min_length.is_some() {
                    all.extend(repeated_a(lengths.least()));
                }
                if let Some(Some(hi)) = lengths.greatest() {
                    all.extend(repeated_a(hi));
                }
                all
            }
            (DomainRule::Where { .. }, _) => self.values(base, kind)?.into_iter().flatten().map(|(v, _)| v).collect(),
        };
        let mut kept: Vec<Json> = Vec::with_capacity(candidates.len());
        for candidate in dedup(candidates) {
            let fits = to_canonical_string(&candidate).is_ok_and(|text| text.len() <= MAX_INPUT_BYTES);
            if fits && self.admitted(id, base, &candidate)? {
                kept.push(candidate);
            }
        }
        Ok(dedup(kept).into_iter().map(scalar).collect())
    }

    /// Whether `value`, a value of the domain `id`'s base `base`, is a member: a closed set's draws always are; else
    /// its whole condition, evaluated by `velme-check` on a fresh system budget within what the scope has left
    /// (R-CHK-15, R-SYNTH-67). A value error is not membership; a budget failure keeps its code, reported against the
    /// domain.
    pub(crate) fn admitted(&mut self, id: DomainId, base: &Type, value: &Json) -> Result<bool, Diagnostic> {
        let domain = self.program.domain(id).ok_or_else(Diagnostic::internal_error)?;
        if matches!(domain.rule, DomainRule::OneOf { .. }) {
            return Ok(true);
        }
        let membership = self.membership.ok_or_else(Diagnostic::internal_error)?;
        // A value no `Number` holds is no member.
        let Ok(value) = decode_value(value, base, self.program) else {
            return Ok(false);
        };
        let limit = self.scope.factor().saturating_mul(self.fuel);
        let left = limit.saturating_sub(self.spent);
        if left == 0 {
            return Err(self.exhausted(domain, limit));
        }
        let fuel = left.min(self.fuel);
        let (result, spent) = membership.member_within(id, &value, fuel);
        self.spent = self.spent.saturating_add(spent);
        match result {
            Ok(()) => Ok(true),
            Err(Refused::Outside(_)) => Ok(false),
            Err(Refused::Stopped(failure)) if fuel < self.fuel && matches!(failure.error, Error::OutOfFuel { .. }) => {
                Err(self.exhausted(domain, limit))
            }
            Err(Refused::Stopped(failure)) => Err(stopped(domain, &failure)),
        }
    }

    /// The `VL0601` of a scope whose predicate evaluations spent their whole total, `limit` (R-SYNTH-67).
    fn exhausted(&self, domain: &DomainType, limit: u64) -> Diagnostic {
        Diagnostic::new(
            Code::BudgetExceeded,
            domain.span,
            format!(
                "Checking generated values against `{}` took too many steps in total.",
                domain.name
            ),
        )
        .with_note(format!(
            "{} may spend {limit} steps (fuel) on domain conditions",
            self.scope.what()
        ))
        .with_help("make its `where` condition cheaper")
    }
}

/// A domain predicate's budget failure (R-SYNTH-63, R-SYNTH-65), worded for the domain and placed at its declaration.
fn stopped(domain: &DomainType, failure: &Failure) -> Diagnostic {
    let name = &domain.name;
    match failure.error {
        Error::OutOfFuel { max_fuel } => Diagnostic::new(
            Code::BudgetExceeded,
            domain.span,
            format!("Checking whether a value is a `{name}` took too many steps."),
        )
        .with_note(format!("it may take {max_fuel} steps (fuel)"))
        .with_help("make its `where` condition cheaper"),
        Error::OutOfMemory { max_memory } => Diagnostic::new(
            Code::MemoryLimitExceeded,
            domain.span,
            format!("Checking whether a value is a `{name}` needed more memory than it's allowed."),
        )
        .with_note(format!("it may allocate {max_memory} bytes"))
        .with_help("make its `where` condition cheaper"),
        _ => failure.diagnostic(name, domain.span),
    }
}

/// The slot of a scalar: its canonical size, or `None` if it can't fit in an input.
fn scalar(value: Json) -> Slot {
    let bytes = to_canonical_string(&value).ok()?.len();
    (bytes <= MAX_INPUT_BYTES).then_some((value, bytes))
}

/// The slot of a list of `items`, sized before it is built: `None` if an item or the whole can't fit in an input.
fn list(items: Vec<Slot>) -> Slot {
    let items: Vec<(Json, usize)> = items.into_iter().collect::<Option<_>>()?;
    let bytes = 2 + items.iter().map(|(_, b)| b + 1).sum::<usize>().saturating_sub(1);
    (bytes <= MAX_INPUT_BYTES).then(|| (Json::Array(items.into_iter().map(|(v, _)| v).collect()), bytes))
}

/// The slot of a record with the field `names` and the values `row`, sized as [`list`].
fn object_slot(names: &[&str], row: Vec<Slot>) -> Slot {
    let row: Vec<(Json, usize)> = row.into_iter().collect::<Option<_>>()?;
    let bytes = 1 + names
        .iter()
        .zip(&row)
        .map(|(n, (_, b))| n.len() + 4 + b)
        .sum::<usize>()
        .max(1);
    (bytes <= MAX_INPUT_BYTES).then(|| {
        let fields = names
            .iter()
            .map(|n| (*n).to_owned())
            .zip(row.into_iter().map(|(v, _)| v));
        (Json::Object(fields.collect::<Map<String, Json>>()), bytes)
    })
}

/// `B(Number)` or `S(Number)` (§7.1), as exact decimal text.
fn number_set(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Boundary => &["0", "1", "-1", "0.5", "1000000", "-1000000"],
        Kind::Small => &["0", "1", "-1"],
    }
}

/// `B(Text)` or `S(Text)` (§7.1).
fn text_set(kind: Kind) -> &'static [&'static str] {
    match kind {
        Kind::Boundary => &["", "a", "Lina", "\u{e9}\u{1f642}"],
        Kind::Small => &["", "a"],
    }
}

/// Whether text of `scalars` scalars, at least that many bytes, could never fit in one input (R-SYNTH-18): checked
/// before it is built (R-SYNTH-62).
fn oversized(scalars: u128) -> bool {
    scalars.saturating_add(2) > MAX_INPUT_BYTES as u128
}

/// `"a"` repeated `n` times, unless it can't fit in an input.
fn repeated_a(n: u128) -> Option<Json> {
    if oversized(n) {
        return None;
    }
    Some(Json::String("a".repeat(usize::try_from(n).ok()?)))
}

/// `text` projected into the admissible lengths (R-SYNTH-62): truncated on a scalar boundary, or `"a"` appended;
/// `None` if the result can't fit in an input.
fn projected_text(text: &str, lengths: &Lengths) -> Option<Json> {
    let mut scalars = text.chars().count() as u128;
    let mut text = text.to_owned();
    if let Some(Some(hi)) = lengths.greatest()
        && scalars > hi
    {
        text = text.chars().take(usize::try_from(hi).ok()?).collect();
        scalars = hi;
    }
    let lo = lengths.least();
    if scalars < lo {
        if oversized(lo) {
            return None;
        }
        text.push_str(&"a".repeat(usize::try_from(lo - scalars).ok()?));
    }
    Some(Json::String(text))
}

/// `values` with each value after its first occurrence removed.
fn dedup(values: Vec<Json>) -> Vec<Json> {
    let mut kept: Vec<Json> = Vec::with_capacity(values.len());
    for value in values {
        if !kept.contains(&value) {
            kept.push(value);
        }
    }
    kept
}

/// The diagonal of ordered sets: `max |Xⱼ|` rows, row `i` taking `Xⱼ[i mod |Xⱼ|]` (§7.1); one empty row for no sets,
/// and none if a set is empty (R-SYNTH-64).
fn diagonal<T: Clone>(sets: &[Vec<T>]) -> Vec<Vec<T>> {
    if sets.iter().any(Vec::is_empty) {
        return Vec::new();
    }
    let rows = sets.iter().map(Vec::len).max().unwrap_or(1);
    (0..rows)
        .map(|i| sets.iter().filter_map(|set| set.get(i % set.len()).cloned()).collect())
        .collect()
}

/// The pairwise rows over positions with `sizes[j]` values each, as index tuples (§7.2).
fn pairwise(sizes: &[usize]) -> Vec<Vec<usize>> {
    let k = sizes.len();
    // A parameter without values leaves no rows (R-SYNTH-64).
    if sizes.contains(&0) {
        return Vec::new();
    }
    match k {
        0 => return Vec::new(),
        1 => return (0..sizes.first().copied().unwrap_or(0)).map(|i| vec![i]).collect(),
        _ => {}
    }
    let size = |j: usize| sizes.get(j).copied().unwrap_or(0);
    // Every pair (j, a, l, b) with j < l, still to be covered, in lexicographic order.
    let mut uncovered = std::collections::BTreeSet::new();
    for j in 0..k {
        for l in j + 1..k {
            for a in 0..size(j) {
                for b in 0..size(l) {
                    uncovered.insert((j, a, l, b));
                }
            }
        }
    }
    let mut rows = Vec::new();
    while let Some(&(j, a, l, b)) = uncovered.iter().next() {
        let mut row: Vec<Option<usize>> = vec![None; k];
        set(&mut row, j, a);
        set(&mut row, l, b);
        for r in 0..k {
            if r == j || r == l {
                continue;
            }
            // The value that covers the most pairs still to be covered, the least on a tie.
            let mut best = (0usize, 0usize);
            for v in 0..size(r) {
                let covers = row
                    .iter()
                    .enumerate()
                    .filter_map(|(f, chosen)| chosen.map(|c| (f, c)))
                    .filter(|&(f, c)| {
                        let pair = if f < r { (f, c, r, v) } else { (r, v, f, c) };
                        uncovered.contains(&pair)
                    })
                    .count();
                if v == 0 || covers > best.0 {
                    best = (covers, v);
                }
            }
            set(&mut row, r, best.1);
        }
        let row: Vec<usize> = row.into_iter().map(Option::unwrap_or_default).collect();
        for p in 0..k {
            for q in p + 1..k {
                if let (Some(x), Some(y)) = (row.get(p), row.get(q)) {
                    uncovered.remove(&(p, *x, q, *y));
                }
            }
        }
        rows.push(row);
    }
    rows
}

fn set(row: &mut [Option<usize>], at: usize, value: usize) {
    if let Some(slot) = row.get_mut(at) {
        *slot = Some(value);
    }
}

/// The seed: the first 8 bytes of the digest `contract_key` spells, read as a little-endian `u64` (§7.3).
pub(crate) fn seed(key: Fingerprint) -> u64 {
    let hex = key.hex();
    let mut bytes = [0u8; 8];
    for (i, byte) in bytes.iter_mut().enumerate() {
        let pair = hex.get(2 * i..2 * i + 2).unwrap_or("00");
        *byte = u8::from_str_radix(pair, 16).unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}

/// The `j`-th output (`j` ≥ 1) of the SplitMix64 stream seeded with `seed` (§7.3).
pub(crate) fn output(seed: u64, j: u64) -> u64 {
    let mut z = seed.wrapping_add(j.wrapping_mul(GAMMA));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One random attempt's stream.
pub(crate) struct Stream {
    seed: u64,
    drawn: u64,
}

impl Stream {
    /// The stream seeded with `seed`, nothing drawn yet.
    pub(crate) fn new(seed: u64) -> Self {
        Stream { seed, drawn: 0 }
    }

    /// `⌊x · n / 2⁶⁴⌋` for the stream's next output `x` (§7.3).
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        u64::try_from(self.points(u128::from(n))).unwrap_or(0)
    }

    /// `below(min(count, 2⁶⁴))`: an index among the first 2⁶⁴ of `count` points, the 128-bit product returning the
    /// output itself at 2⁶⁴ (§7.3, R-SYNTH-62).
    pub(crate) fn points(&mut self, count: u128) -> u128 {
        self.drawn += 1;
        let x = output(self.seed, self.drawn);
        (u128::from(x) * count.min(1 << 64)) >> 64
    }
}

/// The alphabet random text draws from (§7.3): `a`..`z`, `A`, `Z`, `0`, a space, `é` and `🙂`.
fn alphabet() -> Vec<char> {
    let mut all: Vec<char> = ('a'..='z').collect();
    all.extend(['A', 'Z', '0', ' ', '\u{e9}', '\u{1f642}']);
    all
}

/// §7.3's Number draw, as exact decimal text.
fn base_number(stream: &mut Stream) -> String {
    if stream.below(2) == 0 {
        let n = i64::try_from(stream.below(2001)).unwrap_or(0) - 1000;
        n.to_string()
    } else {
        let hundredths = i64::try_from(stream.below(200_001)).unwrap_or(0) - 100_000;
        hundredths_text(hundredths)
    }
}

impl Generator<'_> {
    /// Takes `bytes` from `room`, what the input being drawn has left (R-SYNTH-18); `false`, and the value refused for
    /// its size, if they aren't there.
    fn take(&mut self, room: &mut usize, bytes: usize) -> bool {
        match room.checked_sub(bytes) {
            Some(left) => {
                *room = left;
                true
            }
            None => {
                self.oversized = true;
                false
            }
        }
    }

    /// `value`, a scalar, if its canonical JSON fits in `room`.
    fn sized(&mut self, value: Json, room: &mut usize) -> Option<Json> {
        let bytes = to_canonical_string(&value).ok()?.len();
        self.take(room, bytes).then_some(value)
    }

    /// Text of `length` scalars drawn from the alphabet (§7.3); `None`, before anything is built, if it can't fit in
    /// `room` (R-SYNTH-62). Every scalar of the alphabet is one or more bytes, none escaped.
    fn text(&mut self, length: u128, stream: &mut Stream, room: &mut usize) -> Option<Json> {
        if length.saturating_add(2) > *room as u128 {
            self.oversized = true;
            return None;
        }
        let letters = alphabet();
        let mut text = String::new();
        for _ in 0..length {
            let at = usize::try_from(stream.below(32)).unwrap_or(0);
            text.extend(letters.get(at));
        }
        self.sized(Json::String(text), room)
    }

    /// A value of `ty`, drawn depth first (§7.3); `None` if a domain value in it ran out of retries or a part can't
    /// fit in `room`, what the input has left, which fails the whole candidate (R-SYNTH-63). A list or record takes
    /// its brackets and separators from `room` before its parts are drawn, so nothing over the limit is built.
    pub(crate) fn draw(
        &mut self,
        ty: &Type,
        stream: &mut Stream,
        room: &mut usize,
    ) -> Result<Option<Json>, Diagnostic> {
        Ok(match ty {
            Type::Domain(id, base) => return self.domain_draw(*id, base, stream, room),
            Type::Number => self.sized(number(&base_number(stream)), room),
            Type::Boolean => self.sized(Json::Bool(stream.below(2) == 1), room),
            Type::Text => {
                let length = stream.points(17);
                self.text(length, stream, room)
            }
            Type::Nothing | Type::Error => self.sized(Json::Null, room),
            Type::Optional(inner) => {
                if stream.below(4) == 0 {
                    self.sized(Json::Null, room)
                } else {
                    return self.draw(inner, stream, room);
                }
            }
            Type::List(inner) => {
                let length = stream.below(9);
                let frame = usize::try_from(length).unwrap_or(usize::MAX).saturating_add(1).max(2);
                if !self.take(room, frame) {
                    return Ok(None);
                }
                let mut items = Vec::new();
                for _ in 0..length {
                    let Some(item) = self.draw(inner, stream, room)? else {
                        return Ok(None);
                    };
                    items.push(item);
                }
                Some(Json::Array(items))
            }
            Type::Record(id) => {
                let Some(record) = self.program.record(*id) else {
                    return Ok(Some(Json::Null));
                };
                let frame = 1 + record.fields.iter().map(|f| f.name.len() + 4).sum::<usize>().max(1);
                if !self.take(room, frame) {
                    return Ok(None);
                }
                let mut fields = Map::new();
                for f in &record.fields {
                    let Some(value) = self.draw(&f.ty, stream, room)? else {
                        return Ok(None);
                    };
                    fields.insert(f.name.clone(), value);
                }
                Some(Json::Object(fields))
            }
        })
    }

    /// A value of the domain `id`: a simple-valid candidate, redrawn from the same stream while it isn't a member, up
    /// to [`RETRIES`] times (R-SYNTH-63). A record domain redraws the whole record. Each candidate starts from the same
    /// `room`; one that can't fit fails the draw, unretried.
    fn domain_draw(
        &mut self,
        id: DomainId,
        base: &Type,
        stream: &mut Stream,
        room: &mut usize,
    ) -> Result<Option<Json>, Diagnostic> {
        let start = *room;
        for attempt in 0..=RETRIES {
            if attempt > 0 {
                self.retries += 1;
            }
            *room = start;
            let Some(candidate) = self.simple_draw(id, base, stream, room)? else {
                return Ok(None);
            };
            if self.admitted(id, base, &candidate)? {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    /// One raw draw of the domain `id`, within its simple constraints but not yet held to its condition (R-SYNTH-62):
    /// a closed set's member, an interval's point, a length-bounded text, or a record's fields; `None` if a nested
    /// draw failed, the value can't fit in `room`, or no `Number` holds the drawn point exactly (D-167).
    pub(crate) fn simple_draw(
        &mut self,
        id: DomainId,
        base: &Type,
        stream: &mut Stream,
        room: &mut usize,
    ) -> Result<Option<Json>, Diagnostic> {
        let domain = self.program.domain(id).ok_or_else(Diagnostic::internal_error)?;
        Ok(match (&domain.rule, base) {
            (DomainRule::OneOf { members }, _) => {
                let at = stream.points(members.len() as u128);
                let member = usize::try_from(at).ok().and_then(|at| members.get(at));
                match member {
                    Some(m) => self.sized(Json::String(m.clone()), room),
                    None => None,
                }
            }
            (DomainRule::Where { .. }, Type::Number) => {
                let drawn = Interval::of(&domain.constraints)?.draw(stream, |stream| {
                    Number::parse(&base_number(stream)).unwrap_or(Number::ZERO)
                });
                match drawn {
                    Some(n) => self.sized(number(&n.to_string()), room),
                    None => None,
                }
            }
            (DomainRule::Where { .. }, Type::Text) => match Lengths::of(&domain.constraints)?.draw(stream) {
                Some(length) => self.text(length, stream, room),
                None => None,
            },
            (DomainRule::Where { .. }, _) => return self.draw(base, stream, room),
        })
    }
}

/// `hundredths / 100` as the shortest exact decimal: `-50270` is `-502.7`.
fn hundredths_text(hundredths: i64) -> String {
    let sign = if hundredths < 0 { "-" } else { "" };
    let magnitude = hundredths.unsigned_abs();
    let (whole, cents) = (magnitude / 100, magnitude % 100);
    if cents == 0 {
        format!("{sign}{whole}")
    } else if cents % 10 == 0 {
        format!("{sign}{whole}.{}", cents / 10)
    } else {
        format!("{sign}{whole}.{cents:02}")
    }
}
