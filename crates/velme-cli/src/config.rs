//! Finding, reading and validating the two config files (`tooling/40` §5.1, R-CLI-25, D-105): the project's `velme.toml`
//! (or the `--config` file in its place) and the user-level file. Reading files and the environment is done here; what a file
//! may hold, and the words for what is wrong, are the runtime's.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use velme_diagnostics::Diagnostic;
use velme_runtime::{PROJECT_FILE, ProjectConfig, UserConfig};
use velme_syntax::SourceFile;

/// The most a config file may hold: it is text a person wrote.
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// The settings a command runs with.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub project: ProjectConfig,
    pub user: UserConfig,
}

/// Where the user-level file is kept, by platform (R-CLI-25).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Unix and macOS.
    Unix,
    Windows,
}

impl Platform {
    /// The platform this binary runs on.
    pub const fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}

/// The user-level config file (R-CLI-25): `$XDG_CONFIG_HOME/velme/config.toml`, else `~/.config/velme/config.toml` on Unix
/// and macOS, and `%APPDATA%\velme\config.toml` on Windows. `get` reads the environment; a variable that is empty, or on
/// Unix not an absolute path, is not set (the XDG rule). `None` when the home can't be found.
pub fn user_config_path(get: &dyn Fn(&str) -> Option<OsString>, platform: Platform) -> Option<PathBuf> {
    Some(under_base(get, platform, ("APPDATA", "XDG_CONFIG_HOME", ".config"))?.join("config.toml"))
}

/// The user-level WASM module cache (`runtime/31` R-SBX-13, D-48): `$XDG_CACHE_HOME/velme/wasm`, else `~/.cache/velme/wasm`
/// on Unix and macOS, and `%LOCALAPPDATA%\velme\wasm` on Windows. `get` reads the environment, as for
/// [`user_config_path`].
pub fn wasm_cache_path(get: &dyn Fn(&str) -> Option<OsString>, platform: Platform) -> Option<PathBuf> {
    Some(under_base(get, platform, ("LOCALAPPDATA", "XDG_CACHE_HOME", ".cache"))?.join("wasm"))
}

/// `velme` under the platform's base directory, given as the Windows variable, the XDG variable and the directory under
/// `~` that the XDG variable defaults to.
fn under_base(
    get: &dyn Fn(&str) -> Option<OsString>,
    platform: Platform,
    (windows, xdg, dot_dir): (&str, &str, &str),
) -> Option<PathBuf> {
    let set = |name: &str| get(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = match platform {
        Platform::Windows => set(windows)?,
        Platform::Unix => match set(xdg).filter(|p| p.is_absolute()) {
            Some(xdg) => xdg,
            None => set("HOME")?.join(dot_dir),
        },
    };
    Some(base.join("velme"))
}

/// The text of the config file at `path`, shown as `shown`: `None` if it isn't there and `optional`; `VL0901` if it can't be
/// read.
fn read(path: &Path, shown: &str, optional: bool) -> Result<Option<String>, Diagnostic> {
    let read = || -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).is_ok_and(|len| len > MAX_CONFIG_BYTES) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "it is too big"));
        }
        Ok(bytes)
    };
    match read() {
        Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| {
            SourceFile::unreadable(
                shown,
                &std::io::Error::new(std::io::ErrorKind::InvalidData, "it isn't UTF-8"),
            )
        }),
        Err(e) if optional && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(SourceFile::unreadable(shown, &e)),
    }
}

/// The config file at `path`, shown as `shown`, parsed by `parse`; the default if it isn't there and `optional`. A problem
/// in the file has no place in the source file; it is about this one (D-111).
fn parse_file<T: Default>(
    path: &Path,
    shown: String,
    optional: bool,
    parse: fn(&str, &str) -> Result<T, Diagnostic>,
) -> Result<T, Diagnostic> {
    read(path, &shown, optional)
        .and_then(|text| text.map_or_else(|| Ok(T::default()), |text| parse(&text, &shown)))
        .map_err(|d| d.with_file(shown))
}

/// The project's settings: `flag`'s file if `--config` gave one, else the `velme.toml` in `root` if there is one. The flag
/// changes where settings come from and nothing else: the root, the lock and `.velme/` stay where the source file put
/// them (R-CLI-25).
fn project(root: &Path, flag: Option<&str>) -> Result<ProjectConfig, Diagnostic> {
    match flag {
        Some(flag) => parse_file(
            &PathBuf::from(flag),
            crate::display_path(flag),
            false,
            ProjectConfig::parse,
        ),
        None => parse_file(
            &root.join(PROJECT_FILE),
            PROJECT_FILE.to_owned(),
            true,
            ProjectConfig::parse,
        ),
    }
}

/// The user-level settings, from the file at `path` if there is one (R-CLI-25).
fn user(path: Option<PathBuf>) -> Result<UserConfig, Diagnostic> {
    let Some(path) = path else {
        return Ok(UserConfig::default());
    };
    let shown = crate::display_path(&path.to_string_lossy());
    parse_file(&path, shown, true, UserConfig::parse)
}

/// Both config files of a command on the project at `root`, each validated; every file that is wrong is reported, the
/// project's first (R-CLI-25).
pub fn load(root: &Path, config_flag: Option<&str>) -> Result<Settings, Vec<Diagnostic>> {
    let get = |name: &str| std::env::var_os(name);
    let (project, user) = (
        project(root, config_flag),
        user(user_config_path(&get, Platform::current())),
    );
    match (project, user) {
        (Ok(project), Ok(user)) => Ok(Settings { project, user }),
        (project, user) => Err(project.err().into_iter().chain(user.err()).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(*v))
    }

    /// The user-level file is where R-CLI-25 puts it on each platform (AC-CLI-22).
    #[test]
    fn ac_cli_22_the_user_file_is_found_at_the_location_of_each_platform() {
        let path = |vars: &[(&str, &str)], platform| user_config_path(&env(vars), platform);
        // The XDG variables count only when absolute, and `/x` is not absolute on a Windows host.
        let (x, c) = if cfg!(windows) { ("C:/x", "C:/c") } else { ("/x", "/c") };
        assert_eq!(
            path(&[("XDG_CONFIG_HOME", x), ("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from(x).join("velme/config.toml"))
        );
        assert_eq!(
            path(&[("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from("/h/.config/velme/config.toml"))
        );
        // A relative or empty XDG_CONFIG_HOME is not set.
        assert_eq!(
            path(&[("XDG_CONFIG_HOME", "rel"), ("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from("/h/.config/velme/config.toml"))
        );
        assert_eq!(
            path(&[("XDG_CONFIG_HOME", ""), ("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from("/h/.config/velme/config.toml"))
        );
        assert_eq!(
            path(
                &[("APPDATA", "C:\\Users\\a\\AppData\\Roaming"), ("HOME", "/h")],
                Platform::Windows
            ),
            Some(
                PathBuf::from("C:\\Users\\a\\AppData\\Roaming")
                    .join("velme")
                    .join("config.toml")
            )
        );
        assert_eq!(path(&[], Platform::Unix), None);
        let cache = |vars: &[(&str, &str)], platform| wasm_cache_path(&env(vars), platform);
        assert_eq!(
            cache(&[("XDG_CACHE_HOME", c), ("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from(c).join("velme/wasm"))
        );
        assert_eq!(
            cache(&[("HOME", "/h")], Platform::Unix),
            Some(PathBuf::from("/h/.cache/velme/wasm"))
        );
        assert_eq!(path(&[("XDG_CONFIG_HOME", x)], Platform::Windows), None);
    }
}
