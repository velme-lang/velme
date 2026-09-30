//! Generated test inputs (`compiler/22` §7, D-96): a pure function of the goal's signature, its examples and its
//! `contract_key`, frozen per `language_version` (R-SYNTH-48). Every rule below is the spec's; a change to any of them is
//! a breaking change.

use std::collections::HashSet;

use serde_json::{Map, Value as Json};
use velme_builtins::Value;
use velme_diagnostics::Diagnostic;
use velme_ir::{Fingerprint, decode_str, encode_value, to_canonical_string};
use velme_sema::hir::{GoalId, Program, Type};

/// The most inputs a goal is checked on, examples included (R-SYNTH-18).
const MAX_INPUTS: usize = 64;
/// The most generated inputs the boundary stage may fill, then the pairwise stage (R-SYNTH-18).
const BOUNDARY_CAP: usize = 16;
const PAIRWISE_CAP: usize = 40;
/// The most random attempts (R-SYNTH-18).
const MAX_RANDOM_ATTEMPTS: u64 = 256;
/// The most bytes of canonical JSON one input may have (R-SYNTH-18): 256 KiB.
const MAX_INPUT_BYTES: usize = 262_144;

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
}

/// The inputs to check goal `goal` of `program` on: its `examples` (one value per parameter each), then the generated
/// ones for `contract_key` (§7).
pub fn test_inputs(
    program: &Program,
    goal: GoalId,
    contract_key: Fingerprint,
    examples: &[Vec<Value>],
) -> Result<TestInputs, Diagnostic> {
    let target = program.goals.get(goal.0).ok_or_else(Diagnostic::internal_error)?;
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

    // Stage 2: the diagonal of the parameters' boundary sets.
    let boundary_sets: Vec<Vec<Json>> = types.iter().map(|ty| values(program, ty, Kind::Boundary)).collect();
    for row in diagonal(&boundary_sets) {
        if generated(&set) >= slots.min(BOUNDARY_CAP) {
            break;
        }
        set.offer(row)?;
    }
    let boundary = generated(&set);

    // Stage 3: the pairwise rows over the small sets.
    let small: Vec<Vec<Json>> = types.iter().map(|ty| values(program, ty, Kind::Small)).collect();
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
        set.offer(args)?;
    }
    let pairwise_count = generated(&set) - boundary;

    // Stage 4: per-attempt streams seeded from the contract key.
    let seed = seed(contract_key);
    let mut attempt = 0;
    while generated(&set) < slots && attempt < MAX_RANDOM_ATTEMPTS {
        let mut stream = Stream {
            seed: output(seed, attempt + 1),
            drawn: 0,
        };
        attempt += 1;
        let args = types.iter().map(|ty| draw(program, ty, &mut stream)).collect();
        set.offer(args)?;
    }
    let random = generated(&set) - boundary - pairwise_count;
    Ok(TestInputs {
        inputs: set.inputs,
        examples: e,
        boundary,
        pairwise: pairwise_count,
        random,
    })
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Boundary,
    Small,
}

/// The ordered set `B(ty)` or `S(ty)` (§7.1).
fn values(program: &Program, ty: &Type, kind: Kind) -> Vec<Json> {
    let boundary = kind == Kind::Boundary;
    match ty {
        Type::Number if boundary => ["0", "1", "-1", "0.5", "1000000", "-1000000"]
            .iter()
            .map(|n| number(n))
            .collect(),
        Type::Number => ["0", "1", "-1"].iter().map(|n| number(n)).collect(),
        Type::Boolean => vec![Json::Bool(true), Json::Bool(false)],
        Type::Text if boundary => ["", "a", "Lina", "\u{e9}\u{1f642}"]
            .iter()
            .map(|t| Json::String((*t).to_owned()))
            .collect(),
        Type::Text => ["", "a"].iter().map(|t| Json::String((*t).to_owned())).collect(),
        Type::Nothing | Type::Error => vec![Json::Null],
        Type::Optional(inner) => {
            let mut all = vec![Json::Null];
            all.extend(values(program, inner, kind));
            all
        }
        Type::List(inner) => {
            let item = values(program, inner, kind);
            let at = |i: usize| item.get(i % item.len().max(1)).cloned().unwrap_or(Json::Null);
            if boundary {
                vec![
                    Json::Array(Vec::new()),
                    Json::Array(vec![at(0)]),
                    Json::Array(vec![at(0), at(1), at(2)]),
                    Json::Array(vec![at(0), at(0)]),
                ]
            } else {
                vec![
                    Json::Array(Vec::new()),
                    Json::Array(vec![at(0)]),
                    Json::Array(vec![at(0), at(1)]),
                ]
            }
        }
        Type::Record(id) => {
            let Some(record) = program.record(*id) else {
                return vec![Json::Null];
            };
            let sets: Vec<Vec<Json>> = record.fields.iter().map(|f| values(program, &f.ty, kind)).collect();
            diagonal(&sets)
                .into_iter()
                .map(|row| {
                    Json::Object(
                        record
                            .fields
                            .iter()
                            .map(|f| f.name.clone())
                            .zip(row)
                            .collect::<Map<String, Json>>(),
                    )
                })
                .collect()
        }
    }
}

/// The diagonal of ordered sets: `max |Xⱼ|` rows, row `i` taking `Xⱼ[i mod |Xⱼ|]` (§7.1); one empty row for no sets.
fn diagonal(sets: &[Vec<Json>]) -> Vec<Vec<Json>> {
    let rows = sets.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..rows)
        .map(|i| {
            sets.iter()
                .filter_map(|set| set.get(i % set.len().max(1)).cloned())
                .collect()
        })
        .collect()
}

/// The pairwise rows over positions with `sizes[j]` values each, as index tuples (§7.2).
fn pairwise(sizes: &[usize]) -> Vec<Vec<usize>> {
    let k = sizes.len();
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
fn seed(key: Fingerprint) -> u64 {
    let hex = key.hex();
    let mut bytes = [0u8; 8];
    for (i, byte) in bytes.iter_mut().enumerate() {
        let pair = hex.get(2 * i..2 * i + 2).unwrap_or("00");
        *byte = u8::from_str_radix(pair, 16).unwrap_or(0);
    }
    u64::from_le_bytes(bytes)
}

/// The `j`-th output (`j` ≥ 1) of the SplitMix64 stream seeded with `seed` (§7.3).
fn output(seed: u64, j: u64) -> u64 {
    let mut z = seed.wrapping_add(j.wrapping_mul(GAMMA));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One random attempt's stream.
struct Stream {
    seed: u64,
    drawn: u64,
}

impl Stream {
    /// `⌊x · n / 2⁶⁴⌋` for the stream's next output `x` (§7.3).
    fn below(&mut self, n: u64) -> u64 {
        self.drawn += 1;
        let x = output(self.seed, self.drawn);
        u64::try_from((u128::from(x) * u128::from(n)) >> 64).unwrap_or(0)
    }
}

/// The alphabet random text draws from (§7.3): `a`..`z`, `A`, `Z`, `0`, a space, `é` and `🙂`.
fn alphabet() -> Vec<char> {
    let mut all: Vec<char> = ('a'..='z').collect();
    all.extend(['A', 'Z', '0', ' ', '\u{e9}', '\u{1f642}']);
    all
}

/// A value of `ty`, drawn depth first (§7.3).
fn draw(program: &Program, ty: &Type, stream: &mut Stream) -> Json {
    match ty {
        Type::Number => {
            if stream.below(2) == 0 {
                let n = i64::try_from(stream.below(2001)).unwrap_or(0) - 1000;
                number(&n.to_string())
            } else {
                let hundredths = i64::try_from(stream.below(200_001)).unwrap_or(0) - 100_000;
                number(&hundredths_text(hundredths))
            }
        }
        Type::Boolean => Json::Bool(stream.below(2) == 1),
        Type::Text => {
            let letters = alphabet();
            let length = stream.below(17);
            let mut text = String::new();
            for _ in 0..length {
                let at = usize::try_from(stream.below(32)).unwrap_or(0);
                text.extend(letters.get(at));
            }
            Json::String(text)
        }
        Type::Nothing | Type::Error => Json::Null,
        Type::Optional(inner) => {
            if stream.below(4) == 0 {
                Json::Null
            } else {
                draw(program, inner, stream)
            }
        }
        Type::List(inner) => {
            let length = stream.below(9);
            Json::Array((0..length).map(|_| draw(program, inner, stream)).collect())
        }
        Type::Record(id) => match program.record(*id) {
            Some(record) => Json::Object(
                record
                    .fields
                    .iter()
                    .map(|f| (f.name.clone(), draw(program, &f.ty, stream)))
                    .collect(),
            ),
            None => Json::Null,
        },
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
