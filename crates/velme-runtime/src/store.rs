//! The local artifact store (`runtime/32` §4): content-addressed, append-only, write-once files under
//! `.velme/artifacts/`.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::{CanonicalError, Fingerprint, Goal, ParseError, ValidIr, from_json_str, to_canonical_string};

use crate::artifact::{Artifact, Manifest};

/// The project directory Velme keeps its files in (`runtime/32` §4).
pub const VELME_DIR: &str = ".velme";

/// The store's directory inside [`VELME_DIR`].
pub const ARTIFACTS_DIR: &str = "artifacts";

/// Where artifacts are written before they are placed, inside [`VELME_DIR`]: beside the store, so on the same file
/// system, and never in it, so a crash mid-write leaves nothing under [`ARTIFACTS_DIR`] (R-ART-09).
pub const TMP_DIR: &str = "tmp";

/// An artifact file's name is this prefix, the artifact's hex digits and [`ARTIFACT_EXTENSION`].
const ARTIFACT_PREFIX: &str = "b3-";

/// See [`ARTIFACT_PREFIX`].
const ARTIFACT_EXTENSION: &str = ".json";

/// Makes the names of temporary files unique within this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The artifact store of one project (`runtime/32` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    dir: PathBuf,
    tmp: PathBuf,
}

/// The document as written: [`Artifact`], borrowing an already validated goal.
#[derive(Serialize)]
struct Document<'a> {
    manifest: &'a Manifest,
    ir: &'a Goal,
}

impl Store {
    /// The store of the project whose root is `project`: `<project>/.velme/artifacts/`.
    pub fn new(project: &Path) -> Self {
        let velme = project.join(VELME_DIR);
        Store {
            dir: velme.join(ARTIFACTS_DIR),
            tmp: velme.join(TMP_DIR),
        }
    }

    /// The store's directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the artifact `id` is stored: `b3-<hex>.json`.
    pub fn path(&self, id: Fingerprint) -> PathBuf {
        self.dir
            .join(format!("{ARTIFACT_PREFIX}{}{ARTIFACT_EXTENSION}", id.hex()))
    }

    /// Stores the artifact of `manifest` and `ir` and returns its address: the hash of its canonical JSON, the same on
    /// every machine (R-ART-05). Only validated IR can be stored; the caller stores only candidates that passed full
    /// verification (R-ART-11). An artifact already stored is left as it is (R-ART-09).
    pub fn put(&self, manifest: &Manifest, ir: &ValidIr) -> Result<Fingerprint, StoreError> {
        let bytes = to_canonical_string(&Document {
            manifest,
            ir: ir.goal(),
        })
        .map_err(StoreError::Canonical)?;
        let id = Fingerprint::of_bytes(bytes.as_bytes());
        let path = self.path(id);
        if path.try_exists().map_err(StoreError::Io)? {
            return Ok(id);
        }
        fs::create_dir_all(&self.dir).map_err(StoreError::Io)?;
        fs::create_dir_all(&self.tmp).map_err(StoreError::Io)?;
        let stem = format!("{ARTIFACT_PREFIX}{}", id.hex());
        let temp = write_temp(&self.tmp, &stem, bytes.as_bytes()).map_err(StoreError::Io)?;
        let placed = place(&temp, &path);
        // Leaves no temporary file behind, whether or not the artifact was placed; a rename already moved it.
        let removed = match fs::remove_file(&temp) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
        placed
            .and(removed)
            .and_then(|()| sync_dir(&self.dir))
            .map_err(StoreError::Io)?;
        Ok(id)
    }

    /// Reads the artifact `id`, checking that its bytes still hash to `id` (R-ART-10). The IR in it is not validated
    /// and the manifest not cross-checked here: [`load`](crate::load) does both before trusting it.
    pub fn get(&self, id: Fingerprint) -> Result<Artifact, LoadError> {
        let path = self.path(id);
        let bytes = fs::read(&path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => LoadError::Unavailable { id },
            _ => LoadError::Unreadable { path, error },
        })?;
        let found = Fingerprint::of_bytes(&bytes);
        if found != id {
            return Err(LoadError::Corrupt { id, found });
        }
        let text = String::from_utf8(bytes).map_err(|e| LoadError::Malformed {
            id,
            reason: e.to_string(),
        })?;
        from_json_str(&text).map_err(|e: ParseError| LoadError::Malformed {
            id,
            reason: e.to_string(),
        })
    }
}

/// Writes `bytes` to a new file in the directory `tmp`, named after `stem` and flushed to disk, so it can be placed
/// whole.
pub(crate) fn write_temp(tmp: &Path, stem: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    loop {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let temp = tmp.join(format!(".{stem}.{}-{n}.tmp", std::process::id()));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
            // Left behind by an earlier process with the same id: take the next name.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            other => other?,
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        if let Err(e) = written {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        return Ok(temp);
    }
}

/// Gives the complete file `temp` the name `path` without replacing a file already there (R-ART-09): a hard link is
/// atomic and fails if `path` exists. On a file system without hard links it falls back to a rename, which is still
/// atomic; a writer racing it can then only replace the file with the same bytes, since the name is their hash.
fn place(temp: &Path, path: &Path) -> io::Result<()> {
    match fs::hard_link(temp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(_) if !path.try_exists()? => fs::rename(temp, path),
        Err(_) => Ok(()),
    }
}

/// Makes the new entry in `dir` durable, as the file's own `sync_all` does not. Only Unix can open a directory to
/// sync it.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Why an artifact could not be stored.
#[derive(Debug)]
pub enum StoreError {
    /// The artifact has no canonical JSON.
    Canonical(CanonicalError),
    /// Writing it failed.
    Io(io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canonical(error) => write!(f, "the artifact has no canonical form: {error}"),
            Self::Io(error) => write!(f, "the artifact could not be written: {error}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Why a stored artifact could not be read (`runtime/32` R-ART-10).
#[derive(Debug)]
pub enum LoadError {
    /// There is no file for it: `VL0701`.
    Unavailable {
        /// The artifact asked for.
        id: Fingerprint,
    },
    /// The file is there but could not be read: `VL0901`. Rebuilding cannot fix it, since the store never rewrites
    /// an existing name (R-ART-09).
    Unreadable {
        /// The artifact's file.
        path: PathBuf,
        /// Why it could not be read.
        error: io::Error,
    },
    /// The file's bytes no longer hash to its name: `VL0703`.
    Corrupt {
        /// The artifact asked for.
        id: Fingerprint,
        /// What the bytes hash to.
        found: Fingerprint,
    },
    /// The bytes match their hash but are not an artifact document, which fails re-validation: the lock entry is
    /// stale, `VL0702` (R-ART-14).
    Malformed {
        /// The artifact asked for.
        id: Fingerprint,
        /// What is wrong with it.
        reason: String,
    },
}

impl LoadError {
    /// The diagnostic code.
    pub fn code(&self) -> Code {
        match self {
            Self::Unavailable { .. } => Code::ArtifactUnavailable,
            Self::Unreadable { .. } => Code::FileError,
            Self::Corrupt { .. } => Code::ArtifactCorrupt,
            Self::Malformed { .. } => Code::LockStale,
        }
    }

    /// The failure to load the artifact of the goal `goal` declared at `span`, worded as in `reference/90`.
    pub fn diagnostic(&self, goal: &str, span: Span) -> Diagnostic {
        match self {
            Self::Unavailable { id } => Diagnostic::new(
                self.code(),
                span,
                format!("The built version of `{goal}` is missing — run `velme build`."),
            )
            .with_note(format!("there is no file for artifact {id}")),
            Self::Unreadable { path, error } => {
                Diagnostic::new(self.code(), span, format!("I couldn't open `{}`.", path.display()))
                    .with_note(format!("it holds the built version of `{goal}`: {error}"))
            }
            Self::Corrupt { id, found } => Diagnostic::new(
                self.code(),
                span,
                format!("The built version of `{goal}` was changed or damaged."),
            )
            .with_note(format!("artifact {id} now hashes to {found}")),
            Self::Malformed { id, reason } => Diagnostic::new(
                self.code(),
                span,
                format!("`{goal}` changed since it was last built — run `velme build`."),
            )
            .with_note(format!("artifact {id} is not a valid artifact: {reason}")),
        }
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { id } => write!(f, "artifact {id} is not in the store"),
            Self::Unreadable { path, error } => write!(f, "`{}` could not be read: {error}", path.display()),
            Self::Corrupt { id, found } => write!(f, "artifact {id} is corrupt: its bytes hash to {found}"),
            Self::Malformed { id, reason } => write!(f, "artifact {id} is not a valid artifact: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {}
