//! The compiled-module cache on disk (`runtime/31` §7, D-48, D-116, D-120): what Cranelift made of an emitted module,
//! kept in one user-level directory under a name that says which module and which engine. A cached file is native
//! code, so nothing outside that directory is ever loaded (R-SBX-14, T-11), and the directory is the user's alone
//! (R-SBX-20): its owner and mode are checked on an open handle, and every file in it is reached through that handle.
//! A directory that fails a check turns the disk cache off for the process. There is no disk cache off Unix in v0.1.

use std::hash::{Hash as _, Hasher};
use std::path::{Path, PathBuf, is_separator};
use std::sync::OnceLock;

use wasmtime::Engine;

/// The extension of a cached file.
const EXTENSION: &str = "cwasm";

/// Why the loader did not read a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The path is not a file of the cache directory (R-SBX-14).
    Outside,
    /// The disk cache is off, for this reason (R-SBX-20).
    Unusable(Unusable),
}

/// Why the disk cache is off: from then on every module is compiled on each run, and the runtime can say why under
/// `--verbose` (R-SBX-20, D-120).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(unix), allow(dead_code))]
pub(crate) enum Unusable {
    /// There is no disk cache on this platform in v0.1.
    Platform,
    /// The directory could not be made or opened as a directory: it is a symbolic link, or something else.
    Directory,
    /// The directory belongs to another user.
    Owner,
    /// The directory is open to its group or to others.
    Open,
    /// A file in it is a symbolic link, is not a regular file, belongs to another user, is writable by others, or
    /// could not be read.
    File,
    /// Once made, it resolves inside the project after all (T-11).
    Misplaced(Misplaced),
}

impl std::fmt::Display for Unusable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unusable::Platform => "there is no compiled-module cache on this platform",
            Unusable::Directory => "it is not a directory, or is a symbolic link",
            Unusable::Owner => "it belongs to another user",
            Unusable::Open => "its group or other users have access to it",
            Unusable::File => "a file in it is a symbolic link, not a regular file, or not yours alone",
            Unusable::Misplaced(why) => return why.fmt(f),
        })
    }
}

/// Why a directory cannot hold the compiled-module cache of a project (T-11): the disk cache is then off for the
/// process, and the runtime can say why under `--verbose` (R-SBX-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misplaced {
    /// It is not an absolute path.
    Relative,
    /// It has a `.` or `..` component, which would be read before the directories it passes through are made.
    Dots,
    /// It, or the project's directory, could not be resolved: a symbolic link to nowhere, or no access.
    Unresolved,
    /// It lies inside the project once resolved.
    Inside,
}

impl std::fmt::Display for Misplaced {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Misplaced::Relative => "it is not an absolute path",
            Misplaced::Dots => "it has a `.` or `..` component",
            Misplaced::Unresolved => "it or the project's directory could not be resolved",
            Misplaced::Inside => "it is inside the project",
        })
    }
}

/// A directory the compiled-module cache may be kept in, for one project: absolute, with no `.` or `..` component,
/// and outside the project once both are resolved (R-SBX-14, T-11). It is the only way to give a sandbox a disk
/// cache, and it is checked again each time it is opened, before and after it is made:
///
/// ```compile_fail
/// let _ = velme_wasm::Sandbox::new(Some(std::path::PathBuf::from("/tmp/velme/wasm")));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDir {
    dir: PathBuf,
    /// The project's directory, resolved.
    project: PathBuf,
}

impl CacheDir {
    /// `dir` as the cache of the project at `project`, or why it can't be.
    pub fn new(dir: PathBuf, project: &Path) -> Result<CacheDir, Misplaced> {
        if !dir.is_absolute() {
            return Err(Misplaced::Relative);
        }
        // Read from the text, since `Path::components` drops a `.`, and a last `.` would follow a symbolic link.
        let text = dir.as_os_str().to_string_lossy();
        if text.split(is_separator).any(|part| part == "." || part == "..") {
            return Err(Misplaced::Dots);
        }
        let project = project.canonicalize().map_err(|_| Misplaced::Unresolved)?;
        let dir = CacheDir { dir, project };
        dir.outside()?;
        Ok(dir)
    }

    /// Whether the directory lies outside the project, resolved as far as it exists and the rest, which the cache
    /// would make as plain directories, taken as written.
    fn outside(&self) -> Result<(), Misplaced> {
        let (mut existing, mut rest) = (self.dir.as_path(), Vec::new());
        let mut resolved = loop {
            match existing.canonicalize() {
                Ok(resolved) => break resolved,
                // There but not resolvable: a link to nowhere, which making the rest would follow.
                Err(_) if existing.symlink_metadata().is_ok() => return Err(Misplaced::Unresolved),
                Err(_) => {
                    rest.extend(existing.file_name());
                    existing = existing.parent().ok_or(Misplaced::Unresolved)?;
                }
            }
        };
        resolved.extend(rest.into_iter().rev());
        if resolved.starts_with(&self.project) {
            return Err(Misplaced::Inside);
        }
        Ok(())
    }

    /// The directory, as given.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Whether the directory is still outside the project just before it is made: nothing swapped into its path since
    /// [`CacheDir::new`] has moved it inside (T-11).
    #[cfg(unix)]
    pub(crate) fn unmade(&self) -> Result<(), Unusable> {
        self.outside().map_err(Unusable::Misplaced)
    }

    /// Whether the directory, now made, still lies outside the project, resolved whole (T-11).
    #[cfg(unix)]
    pub(crate) fn made(&self) -> Result<(), Unusable> {
        let resolved = self.dir.canonicalize().map_err(|_| Unusable::Directory)?;
        if resolved.starts_with(&self.project) {
            return Err(Unusable::Misplaced(Misplaced::Inside));
        }
        Ok(())
    }
}

/// The cache directory and the engine whose output it holds.
#[derive(Debug)]
pub(crate) struct Cache {
    dir: CacheDir,
    /// Wasmtime's compatibility hash for the engine, as the second half of every file name (R-SBX-13).
    compat: String,
    /// Why the disk cache is off, once it is: it stays off for the process.
    off: OnceLock<Unusable>,
}

/// BLAKE3 as a [`Hasher`], for [`Engine::precompile_compatibility_hash`]: a name that is the same on every run.
struct Blake3(blake3::Hasher);

impl Hasher for Blake3 {
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    fn finish(&self) -> u64 {
        let hash = self.0.finalize();
        hash.as_bytes()
            .first_chunk()
            .map_or(0, |first| u64::from_le_bytes(*first))
    }
}

impl Cache {
    /// The cache in `dir` for `engine`; off from the start where there is no disk cache.
    pub(crate) fn new(dir: CacheDir, engine: &Engine) -> Cache {
        let mut hasher = Blake3(blake3::Hasher::new());
        engine.precompile_compatibility_hash().hash(&mut hasher);
        let off = OnceLock::new();
        if cfg!(not(unix)) {
            let _ = off.set(Unusable::Platform);
        }
        Cache {
            dir,
            compat: format!("{:016x}", hasher.finish()),
            off,
        }
    }

    /// Where the compiled form of the module `bytes` is kept: `<BLAKE3 of the module>-<compatibility hash>.cwasm`
    /// (R-SBX-13), so a change to the emitter, to Wasmtime or to the engine's configuration is a new name.
    pub(crate) fn path(&self, bytes: &[u8]) -> PathBuf {
        let module = blake3::hash(bytes).to_hex();
        self.dir.path().join(format!("{module}-{}.{EXTENSION}", self.compat))
    }

    /// Why the disk cache is off, if it is, with the directory it is about.
    pub(crate) fn off(&self) -> Option<(&Path, Unusable)> {
        self.off.get().map(|why| (self.dir.path(), *why))
    }

    /// The name of `path` in the directory, if it is a cached file directly in it (R-SBX-14): anything else is
    /// [`Refused::Outside`] before the disk is touched.
    fn name<'p>(&self, path: &'p Path) -> Result<&'p str, Refused> {
        let dir = self.dir.path();
        let inside = path.parent() == Some(dir)
            && path.extension().is_some_and(|extension| extension == EXTENSION)
            && path.file_name().is_some_and(|name| dir.join(name) == path);
        let name = path.file_name().and_then(|name| name.to_str());
        name.filter(|_| inside).ok_or(Refused::Outside)
    }

    /// Turns the disk cache off for `why`, and says so.
    fn refuse(&self, why: Unusable) -> Refused {
        Refused::Unusable(*self.off.get_or_init(|| why))
    }

    /// The directory, opened, if the disk cache is on (R-SBX-20).
    #[cfg(unix)]
    fn open(&self) -> Result<disk::Dir, Refused> {
        if let Some(why) = self.off.get() {
            return Err(Refused::Unusable(*why));
        }
        disk::Dir::open(&self.dir).map_err(|why| self.refuse(why))
    }

    /// The bytes of the cached file `path`, or `None` if there is none. Only a file directly in the cache directory
    /// is read, whatever `path` says (R-SBX-14); a directory or a file that fails a check of R-SBX-20 turns the disk
    /// cache off.
    pub(crate) fn read(&self, path: &Path) -> Result<Option<Vec<u8>>, Refused> {
        let name = self.name(path)?;
        #[cfg(unix)]
        {
            self.open()?.read(name).map_err(|why| self.refuse(why))
        }
        #[cfg(not(unix))]
        {
            let _ = name;
            Err(self.refuse(Unusable::Platform))
        }
    }

    /// Keeps `bytes` as the file `path`, whole or not at all: written under a temporary name and renamed into place
    /// (R-SBX-20). The cache is derived (D-12), so a failure is only a slower next run and is not reported.
    pub(crate) fn write(&self, path: &Path, bytes: &[u8]) {
        let Ok(name) = self.name(path) else { return };
        #[cfg(unix)]
        if let Ok(dir) = self.open() {
            dir.write(name, bytes);
        }
        #[cfg(not(unix))]
        let _ = (name, bytes);
    }

    /// Deletes the cached file `path`, which did not load: the next run writes a good one.
    pub(crate) fn discard(&self, path: &Path) {
        let Ok(name) = self.name(path) else { return };
        #[cfg(unix)]
        if let Ok(dir) = self.open() {
            dir.discard(name);
        }
        #[cfg(not(unix))]
        let _ = name;
    }
}

/// Whether a file of kind `kind`, mode `mode` and owner `owner` is the user `euid`'s alone (R-SBX-20): a directory
/// that is theirs with no access for group or others, or a regular file that is theirs and only they can write.
#[cfg(unix)]
pub(crate) fn verdict(
    kind: rustix::fs::FileType,
    mode: rustix::fs::Mode,
    owner: u32,
    euid: u32,
    directory: bool,
) -> Result<(), Unusable> {
    use rustix::fs::{FileType, Mode};
    let (want, shut, wrong) = if directory {
        (FileType::Directory, Mode::RWXG | Mode::RWXO, Unusable::Directory)
    } else {
        (FileType::RegularFile, Mode::WGRP | Mode::WOTH, Unusable::File)
    };
    if kind != want {
        Err(wrong)
    } else if owner != euid {
        Err(if directory { Unusable::Owner } else { Unusable::File })
    } else if mode.intersects(shut) {
        Err(if directory { Unusable::Open } else { Unusable::File })
    } else {
        Ok(())
    }
}

/// The directory on Unix, reached by path once and then only through its handle.
#[cfg(unix)]
mod disk {
    use std::fs::File;
    use std::io::{Read as _, Write as _};
    use std::os::fd::{AsFd, OwnedFd};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, open, openat, renameat, unlinkat};
    use rustix::io::Errno;
    use rustix::process::geteuid;

    use super::{CacheDir, Unusable, verdict};

    /// Makes the names of temporary files unique within this process.
    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Whether the open `fd` is the user's alone.
    fn owned(fd: impl AsFd, directory: bool, failed: Unusable) -> Result<(), Unusable> {
        let stat = fstat(fd).map_err(|_| failed)?;
        let (kind, mode) = (FileType::from_raw_mode(stat.st_mode), Mode::from_raw_mode(stat.st_mode));
        verdict(kind, mode, stat.st_uid, geteuid().as_raw(), directory)
    }

    /// The cache directory, open and checked.
    pub(super) struct Dir(OwnedFd);

    impl Dir {
        /// `dir`, made if it is missing with mode `0700` and checked again to lie outside the project before and after,
        /// then opened without following a symbolic link and checked on the handle: the user's, with no access for group or others. One
        /// that fails is never changed.
        pub(super) fn open(dir: &CacheDir) -> Result<Dir, Unusable> {
            use std::os::unix::fs::DirBuilderExt as _;
            let path = dir.path();
            // Made by path, and resolved again on either side; whatever is there is checked below on the handle, so a
            // swap in between is refused.
            dir.unmade()?;
            let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path);
            dir.made()?;
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let fd = open(path, flags, Mode::empty()).map_err(|_| Unusable::Directory)?;
            owned(&fd, true, Unusable::Directory)?;
            Ok(Dir(fd))
        }

        /// The bytes of the file `name`, or `None` if there is none: opened without following a symbolic link or
        /// blocking on a FIFO, and checked on the handle before it is read.
        pub(super) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, Unusable> {
            let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
            let fd = match openat(&self.0, name, flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(Errno::NOENT) => return Ok(None),
                Err(_) => return Err(Unusable::File),
            };
            owned(&fd, false, Unusable::File)?;
            let mut bytes = Vec::new();
            File::from(fd).read_to_end(&mut bytes).map_err(|_| Unusable::File)?;
            Ok(Some(bytes))
        }

        /// Writes `bytes` as the file `name` under a temporary name of this process, `0600`, and renames it into
        /// place; on a failure the temporary file is removed.
        pub(super) fn write(&self, name: &str, bytes: &[u8]) {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let temp = format!(".{name}.{}-{n}.tmp", std::process::id());
            let flags = OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            let Ok(fd) = openat(&self.0, temp.as_str(), flags, Mode::RUSR | Mode::WUSR) else {
                return;
            };
            let written = File::from(fd).write_all(bytes).is_ok();
            if !written || renameat(&self.0, temp.as_str(), &self.0, name).is_err() {
                let _ = unlinkat(&self.0, temp.as_str(), AtFlags::empty());
            }
        }

        /// Deletes the file `name`.
        pub(super) fn discard(&self, name: &str) {
            let _ = unlinkat(&self.0, name, AtFlags::empty());
        }
    }
}
