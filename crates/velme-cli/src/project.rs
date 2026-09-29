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
    /// each name as stored — so every spelling of one file finds the same root and the same lock entry.
    pub fn of(file: &Path) -> io::Result<Project> {
        let file = std::fs::canonicalize(file)?;
        let dir = file.parent().unwrap_or(Path::new(""));
        let root = dir
            .ancestors()
            .find(|ancestor| ancestor.join(CONFIG_FILE).is_file())
            .unwrap_or(dir);
        let relative: Vec<String> = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .components()
            .filter_map(|c| match c {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        Ok(Project {
            root: root.to_path_buf(),
            file: relative.join("/"),
        })
    }
}
