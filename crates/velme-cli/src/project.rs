//! The project a source file belongs to (`tooling/40` R-CLI-20, D-82): where its `velme.lock` and `.velme/` are.

use std::io;
use std::path::{Component, Path, PathBuf};

/// The project configuration file (`tooling/40` §5.1), which marks the project root.
pub const CONFIG_FILE: &str = "velme.toml";

/// A source file's project.
pub struct Project {
    /// The project root.
    pub root: PathBuf,
    /// The source file's path from the root with `/` separators, as the lock records it (R-CLI-19).
    pub file: String,
}

impl Project {
    /// The project of the source file `file`: the directory of the nearest `velme.toml` above it, else the file's own
    /// directory (D-82). The path is resolved first — `..`, links, and on a case-insensitive file system the case of
    /// each name as stored — so every spelling of one file finds the same root and the same lock entry. A name on the
    /// way from the root that isn't UTF-8 is an error: the lock records the path as text (R-CLI-19).
    pub fn of(file: &Path) -> io::Result<Project> {
        let file = std::fs::canonicalize(file)?;
        let dir = file.parent().unwrap_or(Path::new(""));
        let root = dir
            .ancestors()
            .find(|ancestor| ancestor.join(CONFIG_FILE).is_file())
            .unwrap_or(dir);
        let relative = match (root.to_str(), file.to_str()) {
            (Some(root), Some(file)) => lock_path(root, file, cfg!(windows)),
            _ => None,
        };
        let relative = match relative {
            Some(relative) => relative,
            // Either path has a name that isn't UTF-8: say which one.
            None => {
                let bad = file
                    .strip_prefix(root)
                    .unwrap_or(&file)
                    .components()
                    .find_map(|c| match c {
                        Component::Normal(part) if part.to_str().is_none() => Some(part.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .unwrap_or_default();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("the name `{bad}` isn't UTF-8"),
                ));
            }
        };
        Ok(Project {
            root: root.to_path_buf(),
            file: relative,
        })
    }
}

/// `file` from `root` as the lock records it (`tooling/40` R-CLI-19, R-CLI-20): the names below the root joined with `/`,
/// whatever separator the platform writes. `windows` says that `\\` also separates names, as on Windows, where a drive
/// letter's case and a `\\?\` prefix don't tell one file from another; elsewhere a backslash is part of a name. `None` if `file`
/// isn't below `root`.
pub(crate) fn lock_path(root: &str, file: &str, windows: bool) -> Option<String> {
    let names = |path: &str| -> Vec<String> {
        let path = if windows {
            path.strip_prefix("\\\\?\\").unwrap_or(path)
        } else {
            path
        };
        path.split(|c| c == '/' || (windows && c == '\\'))
            .filter(|name| !name.is_empty() && *name != ".")
            .map(str::to_owned)
            .collect()
    };
    let (root, file) = (names(root), names(file));
    let n = root.len();
    let below = file.len() > n
        && root
            .iter()
            .zip(&file)
            .all(|(a, b)| if windows { a.eq_ignore_ascii_case(b) } else { a == b });
    below.then(|| file.get(n..).unwrap_or_default().join("/"))
}

#[cfg(test)]
mod tests {
    use super::lock_path;

    /// The same project on a Windows-style and a Linux-style path layout gives the same `file` field, with `/` (AC-CLI-16,
    /// R-CLI-19).
    #[test]
    fn ac_cli_16_windows_and_linux_layouts_give_the_same_lock_path() {
        let linux = lock_path("/home/ann/game", "/home/ann/game/levels/intro.velme", false);
        let windows = lock_path(
            "C:\\Users\\ann\\game",
            "C:\\Users\\ann\\game\\levels\\intro.velme",
            true,
        );
        let verbatim = lock_path(
            "\\\\?\\C:\\Users\\ann\\game",
            "\\\\?\\c:\\Users\\ann\\game\\levels\\intro.velme",
            true,
        );
        assert_eq!(linux.as_deref(), Some("levels/intro.velme"));
        assert_eq!(windows, linux);
        assert_eq!(verbatim, linux);
        assert_eq!(
            lock_path("C:\\game", "C:\\game\\a.velme", true).as_deref(),
            Some("a.velme")
        );
        // Off Windows a backslash belongs to the name.
        assert_eq!(lock_path("/p", "/p/a\\b.velme", false).as_deref(), Some("a\\b.velme"));
        // Not below the root, or the root itself.
        assert_eq!(lock_path("/p", "/q/a.velme", false), None);
        assert_eq!(lock_path("/p", "/p", false), None);
    }
}
