//! No-follow file primitives for the files Velme writes and reads back: the artifact store and `velme.lock` in
//! `velme-runtime` (`runtime/32` R-ART-09, R-ART-10), and the replay fixtures here (`compiler/22` R-SYNTH-43). They live
//! in this crate because the runtime sits above it (INV-9) and both need the same rules; nothing here knows about
//! providers.

use std::fs::{self, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Makes the names of temporary files unique within this process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The bytes of the regular file `path`, or `None` if it holds more than `max` bytes, which are not read (R-ART-10). A
/// symbolic link, directory or other special file is an error: nothing Velme writes is one. On Unix the file is opened
/// without following a link and without blocking, so a link swapped in after the first look is refused and a FIFO
/// doesn't hang the read; what was opened is checked again.
pub fn read_file(path: &Path, max: u64) -> io::Result<Option<Vec<u8>>> {
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
pub fn refuse_links(dirs: &[&Path]) -> io::Result<()> {
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
pub fn write_temp(tmp: &Path, stem: &str, bytes: &[u8]) -> io::Result<PathBuf> {
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

/// Makes the new entry in `dir` durable, as the file's own `sync_all` does not. Only Unix can open a directory to
/// sync it.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    if cfg!(unix) {
        fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

/// Writes `bytes` as the file `name` in `dir`, whole or not at all: to a flushed temporary file beside it, then renamed
/// over the old one. Nothing is written through a link: `dir` being a symbolic link, or `name` already being anything but
/// a regular file, is an error (R-ART-09). Creates `dir` if it is missing.
pub fn write_file(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    refuse_links(&[dir])?;
    let path = dir.join(name);
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => return Err(io::Error::other("it is not a regular file")),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    fs::create_dir_all(dir)?;
    let temp = write_temp(dir, name, bytes)?;
    let placed = fs::rename(&temp, &path);
    if placed.is_err() {
        let _ = fs::remove_file(&temp);
    }
    placed.and_then(|()| sync_dir(dir))
}
