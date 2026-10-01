//! What a process keeps in memory between runs: its emitted modules, by the hash of their IR, and what the sandbox
//! compiled them to, by the hash of their bytes (`runtime/31` R-SBX-13, D-133). Both are bounded, the least recently
//! used dropped first; dropping one only means making it again.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, PoisonError};

use velme_ir::{Fingerprint, ValidIr};

use crate::{EmitError, Module, emit};

/// Modules a process keeps of each kind: twice the leaves one run can call (`max_goal_calls`), so a run never makes
/// one twice.
pub(crate) const MAX_KEPT: usize = 2 * velme_builtins::limits::MAX_GOAL_CALLS as usize;

/// The module bytes a process keeps of each kind, whatever their count: the emitted modules themselves, and the
/// modules the code the sandbox compiled was compiled from, which grows with them. A real project's leaves are a few
/// KB each, so this binds only a process that loads many large ones, a fuzz run or a long test: [`MAX_KEPT`] modules
/// near `MAX_IR_BYTES`, and the code compiled from them, would be half a GB.
pub(crate) const MAX_KEPT_BYTES: usize = 64 * 1024 * 1024;

/// At most [`MAX_KEPT`] values by key, of at most [`MAX_KEPT_BYTES`] together, each with its bytes and when it was
/// last used. A value larger than that alone is kept alone.
pub(crate) struct Lru<K, V> {
    entries: HashMap<K, (V, usize, u64)>,
    bytes: usize,
    clock: u64,
}

impl<K, V> Default for Lru<K, V> {
    fn default() -> Self {
        Lru {
            entries: HashMap::new(),
            bytes: 0,
            clock: 0,
        }
    }
}

impl<K: Eq + Hash + Copy, V: Clone> Lru<K, V> {
    /// The value of `key`, if it is kept, marked as just used.
    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        self.clock += 1;
        let (value, _, used) = self.entries.get_mut(key)?;
        *used = self.clock;
        Some(value.clone())
    }

    /// Keeps `value`, of `bytes`, as that of `key`, or what another thread kept first, dropping the least recently
    /// used until it fits.
    pub(crate) fn keep(&mut self, key: K, value: V, bytes: usize) -> V {
        if let Some(kept) = self.get(&key) {
            return kept;
        }
        while !self.entries.is_empty()
            && (self.entries.len() >= MAX_KEPT || self.bytes.saturating_add(bytes) > MAX_KEPT_BYTES)
        {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(k, _)| *k);
            if let Some((_, dropped, _)) = oldest.and_then(|oldest| self.entries.remove(&oldest)) {
                self.bytes -= dropped;
            }
        }
        self.bytes += bytes;
        self.entries.insert(key, (value.clone(), bytes, self.clock));
        value
    }

    /// How many values are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether `key` is kept, without marking it used.
    #[cfg(test)]
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }
}

/// The modules a process has emitted, by the hash of their IR's canonical JSON, a decline too: a module is a pure
/// function of its IR (R-SBX-03), so each IR is emitted once a process (R-SBX-13, D-133).
#[derive(Default)]
pub struct Modules {
    kept: Mutex<Lru<Fingerprint, Result<Arc<Module>, EmitError>>>,
}

impl std::fmt::Debug for Modules {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Modules").finish_non_exhaustive()
    }
}

impl Modules {
    /// None kept yet.
    pub fn new() -> Modules {
        Modules::default()
    }

    /// The module of the leaf goal `ir`, as [`emit`] gives it: emitted the first time this process asks.
    ///
    /// Kept by the hash of the document alone, so what the emitter reads of `ir` must be the document's or derived
    /// from it: a field of the IR that is not part of the document (`#[serde(skip)]`, such as `Map`'s `items`) is the
    /// validator's to set, from the document, never the caller's. Then IR with the same canonical form has the same
    /// module.
    pub fn emit(&self, ir: &ValidIr) -> Result<Arc<Module>, EmitError> {
        // Validated IR always has a canonical form; were it to have none, it is emitted each time.
        let Some(key) = ir.fingerprint() else {
            return emit(ir).map(Arc::new);
        };
        if let Some(kept) = self.lock().get(&key) {
            return kept;
        }
        let emitted = emit(ir).map(Arc::new);
        let bytes = emitted.as_ref().map_or(0, |module| module.bytes().len());
        self.lock().keep(key, emitted, bytes)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Lru<Fingerprint, Result<Arc<Module>, EmitError>>> {
        self.kept.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// How many are kept, for the tests of [`MAX_KEPT`].
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}
