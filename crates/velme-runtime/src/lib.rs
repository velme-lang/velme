//! Velme `runtime` crate: see `compiler/20` §2 for its responsibility.
#![forbid(unsafe_code)]

mod artifact;
mod clock;
mod explain;
mod input;
mod leaf;
mod lock;
mod locked;
mod plan;
mod registry;
mod sched;
mod store;
mod trace;

pub use artifact::{ARTIFACT_FORMAT, Artifact, ArtifactFormat, Child, Kind, Manifest, Verification};
pub use clock::{Clock, SystemClock};
pub use explain::explain;
pub use input::{MAX_INPUT_BYTES, decode_inputs, find_goal, read_input};
pub use leaf::{run_leaf, test_leaf};
pub use lock::{Entry, LOCK_FILE, LOCK_VERSION, Lock, LockError, MAX_LOCK_BYTES};
pub use locked::{Cause, EntryError, LockedGoal, RecordChange, Versioned, load};
pub use registry::Registry;
pub use sched::{CallRun, CallStatus, CheckRun, Failed, GoalRun, Options, Timing, run_goal, run_goal_peak};
pub use store::{ARTIFACTS_DIR, LoadError, MAX_ARTIFACT_BYTES, Store, StoreError, TMP_DIR, VELME_DIR};
pub use trace::{OrderedValue, TRACE_VERSION, Trace};
