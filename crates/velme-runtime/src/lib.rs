//! Velme `runtime` crate: see `compiler/20` §2 for its responsibility.
#![forbid(unsafe_code)]

mod artifact;
mod store;

pub use artifact::{Artifact, Child, Manifest, Verification};
pub use store::{ARTIFACTS_DIR, LoadError, Store, StoreError, TMP_DIR, VELME_DIR};
