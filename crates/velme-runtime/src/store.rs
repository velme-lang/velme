//! The local artifact store (`runtime/32` §4): content-addressed, append-only, write-once files under
//! `.velme/artifacts/`.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use velme_builtins::limits::MIB;
use velme_diagnostics::{Code, Diagnostic, Span};
use velme_ir::limits::MAX_IR_BYTES;
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

/// The largest artifact file read: its IR, which re-validation limits to [`MAX_IR_BYTES`] (`compiler/21` §7), and
/// room for the manifest (R-ART-10). A longer file is no artifact, and is not read past this.
pub const MAX_ARTIFACT_BYTES: u64 = MAX_IR_BYTES as u64 + MIB / 16;

/// Makes the names of temporary files unique within this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The artifact store of one project (`runtime/32` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    velme: PathBuf,
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
            velme,
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
    /// every machine that runs the same compiler version (R-ART-05). Only validated IR can be stored; the caller stores
    /// only candidates that passed full verification (R-ART-11). An artifact already stored is left as it is; a file
    /// under its name with other bytes is not that artifact, and is replaced whole (R-ART-09).
    pub fn put(&self, manifest: &Manifest, ir: &ValidIr) -> Result<Fingerprint, StoreError> {
        let bytes = to_canonical_string(&Document {
            manifest,
            ir: ir.goal(),
        })
        .map_err(StoreError::Canonical)?;
        // Validation bounds the IR's canonical form, so only an oversized manifest can reach this; `get` would refuse it.
        if u64::try_from(bytes.len()).map_or(true, |len| len > MAX_ARTIFACT_BYTES) {
            return Err(StoreError::TooLarge { bytes: bytes.len() });
        }
        let id = Fingerprint::of_bytes(bytes.as_bytes());
        let path = self.path(id);
        refuse_links(&[&self.velme, &self.dir, &self.tmp]).map_err(StoreError::Io)?;
        let replace = match read_file(&path, MAX_ARTIFACT_BYTES) {
            Ok(Some(old)) if old == bytes.as_bytes() => return Ok(id),
            // A regular file with other bytes, or too many: not this artifact.
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            // Not a regular file, or unreadable: nothing the store wrote, so it isn't replaced blindly (R-ART-09).
            Err(e) => return Err(StoreError::Io(e)),
        };
        fs::create_dir_all(&self.dir).map_err(StoreError::Io)?;
        fs::create_dir_all(&self.tmp).map_err(StoreError::Io)?;
        let stem = format!("{ARTIFACT_PREFIX}{}", id.hex());
        let temp = write_temp(&self.tmp, &stem, bytes.as_bytes()).map_err(StoreError::Io)?;
        let placed = if replace {
            fs::rename(&temp, &path)
        } else {
            place(&temp, &path)
        };
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

    /// Reads the artifact `id`, checking that its bytes still hash to `id` and are the canonical JSON the store writes
    /// (R-ART-10). The IR in it is not validated and the manifest not cross-checked here: [`load`](crate::load) does
    /// both before trusting it.
    pub fn get(&self, id: Fingerprint) -> Result<Artifact, LoadError> {
        let path = self.path(id);
        let unreadable = |path: PathBuf, error| LoadError::Unreadable { path, error };
        refuse_links(&[&self.velme, &self.dir]).map_err(|e| unreadable(path.clone(), e))?;
        let bytes = match read_file(&path, MAX_ARTIFACT_BYTES) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return Err(LoadError::Damaged {
                    id,
                    reason: format!("is longer than the {MAX_ARTIFACT_BYTES} bytes an artifact may be"),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(LoadError::Unavailable { id }),
            Err(error) => return Err(unreadable(path, error)),
        };
        let found = Fingerprint::of_bytes(&bytes);
        if found != id {
            return Err(LoadError::Corrupt { id, found });
        }
        let text = String::from_utf8(bytes).map_err(|e| LoadError::Malformed {
            id,
            reason: e.to_string(),
        })?;
        let artifact: Artifact = from_json_str(&text).map_err(|e: ParseError| LoadError::Malformed {
            id,
            reason: e.to_string(),
        })?;
        // The store writes only canonical JSON, so other bytes that hash to their name were put there by hand.
        if to_canonical_string(&artifact).ok().as_deref() != Some(text.as_str()) {
            return Err(LoadError::Damaged {
                id,
                reason: "isn't in the canonical form the store writes".to_owned(),
            });
        }
        Ok(artifact)
    }
}

/// The bytes of the regular file `path`, or `None` if it holds more than `max` bytes, which are not read (R-ART-10). A
/// symbolic link, directory or other special file is an error: nothing Velme writes is one. On Unix the file is opened
/// without following a link and without blocking, so a link swapped in after the first look is refused and a FIFO
/// doesn't hang the read; what was opened is checked again.
pub(crate) fn read_file(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
    let not_a_file = || io::Error::other("it is not a regular file");
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(not_a_file());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        // What `O_NOFOLLOW` gives for a link.
        #[cfg(unix)]
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => return Err(not_a_file()),
        other => other?,
    };
    if !file.metadata()?.file_type().is_file() {
        return Err(not_a_file());
    }
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok(u64::try_from(bytes.len()).is_ok_and(|len| len <= max).then_some(bytes))
}

/// An error if any of `dirs` is a symbolic link: Velme writes only inside its own `.velme` directories, never through a
/// link out of them (`runtime/32` R-ART-09). A missing one is fine. Best effort: a directory swapped for a link after
/// this look is not caught.
pub(crate) fn refuse_links(dirs: &[&Path]) -> io::Result<()> {
    for dir in dirs {
        match fs::symlink_metadata(dir) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "`{}` is a symbolic link, which Velme doesn't follow",
                    dir.display()
                )));
            }
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
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
    /// Its canonical JSON is longer than [`MAX_ARTIFACT_BYTES`], so `get` would refuse it.
    TooLarge {
        /// Its length.
        bytes: usize,
    },
    /// Writing it failed, or its name holds something that isn't a readable regular file, or a store directory is a
    /// link: `VL0901` (R-ART-09).
    Io(io::Error),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canonical(error) => write!(f, "the artifact has no canonical form: {error}"),
            Self::TooLarge { bytes } => write!(
                f,
                "the artifact is {bytes} bytes long; at most {MAX_ARTIFACT_BYTES} are stored"
            ),
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
    /// The file is there but could not be read, or is not a regular file, or the store's directory is a link:
    /// `VL0901`.
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
    /// The file is longer than any artifact, or its bytes hash to its name but aren't the canonical JSON the store
    /// writes: `VL0703`.
    Damaged {
        /// The artifact asked for.
        id: Fingerprint,
        /// What is wrong with the file, completing "artifact … ".
        reason: String,
    },
    /// The bytes match their hash but are not an artifact document of this format, which fails re-validation: the
    /// lock entry is stale, `VL0702` (R-ART-14, D-86).
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
            Self::Corrupt { .. } | Self::Damaged { .. } => Code::ArtifactCorrupt,
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
            Self::Damaged { id, reason } => Diagnostic::new(
                self.code(),
                span,
                format!("The built version of `{goal}` was changed or damaged."),
            )
            .with_note(format!("artifact {id} {reason}")),
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
            Self::Damaged { id, reason } => write!(f, "artifact {id} is corrupt: it {reason}"),
            Self::Malformed { id, reason } => write!(f, "artifact {id} is not a valid artifact: {reason}"),
        }
    }
}

impl std::error::Error for LoadError {}
