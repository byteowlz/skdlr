//! XDG-compliant path resolution for skdlr directories.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::{APP_NAME, SkdlrConfig};

/// Application paths for config, data, and state directories.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
}

impl AppPaths {
    /// Discover application paths.
    pub fn discover(override_path: Option<PathBuf>) -> Result<Self> {
        let config_file = match override_path {
            Some(path) => {
                let expanded = expand_path(path)?;
                if expanded.is_dir() {
                    expanded.join("config.toml")
                } else {
                    expanded
                }
            }
            None => default_config_dir()?.join("config.toml"),
        };

        let data_dir = default_data_dir()?;
        let db_path = data_dir.join("skdlr.db");

        Ok(Self {
            config_file,
            data_dir,
            db_path,
        })
    }

    /// Ensure all required directories exist.
    pub fn ensure_directories(&self) -> Result<()> {
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating data directory {}", self.data_dir.display()))?;
        Ok(())
    }
}

impl std::fmt::Display for AppPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "config: {}, data: {}, db: {}",
            self.config_file.display(),
            self.data_dir.display(),
            self.db_path.display()
        )
    }
}

/// Expand a `PathBuf`, resolving ~ and environment variables.
pub fn expand_path(path: PathBuf) -> Result<PathBuf> {
    if let Some(text) = path.to_str() {
        expand_str_path(text)
    } else {
        Ok(path)
    }
}

/// Expand a string path, resolving ~ and environment variables.
pub fn expand_str_path(text: &str) -> Result<PathBuf> {
    let expanded = shellexpand::full(text).context("expanding path")?;
    Ok(PathBuf::from(expanded.to_string()))
}

/// Resolve a base directory using the "option B" rules:
/// an explicit, absolute `XDG_*` value wins on any OS; otherwise on Windows the
/// platform variable (`%APPDATA%`/`%LOCALAPPDATA%`) is used, and on every other
/// OS (including macOS) the unix-relative path under `$HOME` is used.
fn resolve_base(
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
    win_dir: Option<PathBuf>,
    is_windows: bool,
    unix_rel: &str,
) -> Option<PathBuf> {
    if let Some(p) = xdg.filter(|p| p.is_absolute()) {
        return Some(p);
    }
    if is_windows {
        win_dir
    } else {
        home.map(|h| h.join(unix_rel))
    }
}

/// Resolve a base directory from the relevant environment variables.
fn base_dir(xdg_var: &str, unix_rel: &str, win_var: &str) -> Result<PathBuf> {
    resolve_base(
        env::var_os(xdg_var).map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
        env::var_os(win_var).map(PathBuf::from),
        cfg!(windows),
        unix_rel,
    )
    .ok_or_else(|| anyhow!("unable to determine base directory ({xdg_var})"))
}

/// Get the default configuration directory.
pub fn default_config_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_CONFIG_HOME", ".config", "APPDATA")?.join(APP_NAME))
}

/// Get the default data directory.
pub fn default_data_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_DATA_HOME", ".local/share", "APPDATA")?.join(APP_NAME))
}

/// Get the default state directory.
pub fn default_state_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")?.join(APP_NAME))
}

/// Get the default cache directory.
pub fn default_cache_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA")?.join(APP_NAME))
}

/// Write the default configuration file to the specified path.
pub fn write_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {parent:?}"))?;
    }

    let config = SkdlrConfig::default();
    let toml_str = toml::to_string_pretty(&config).context("serializing default config to TOML")?;
    let mut body = String::new();
    body.push_str("# Configuration for skdlr\n");
    body.push_str("# File: ");
    body.push_str(&path.display().to_string());
    body.push_str("\n\n");
    body.push_str(&toml_str);
    fs::write(path, body).with_context(|| format!("writing config file to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_absolute_wins() {
        let got = resolve_base(
            Some(PathBuf::from("/explicit/xdg")),
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("C:\\AppData")),
            false,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/explicit/xdg")));

        // Absolute XDG also wins on Windows.
        let got_win = resolve_base(
            Some(PathBuf::from("/explicit/xdg")),
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("C:\\AppData")),
            true,
            ".config",
        );
        assert_eq!(got_win, Some(PathBuf::from("/explicit/xdg")));
    }

    #[test]
    fn unix_uses_home_relative() {
        let got = resolve_base(
            None,
            Some(PathBuf::from("/home/user")),
            None,
            false,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/home/user/.config")));
    }

    #[test]
    fn windows_uses_win_dir() {
        let got = resolve_base(
            None,
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming")),
            true,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("C:\\Users\\u\\AppData\\Roaming")));
    }

    #[test]
    fn relative_xdg_is_ignored() {
        let got = resolve_base(
            Some(PathBuf::from("relative/xdg")),
            Some(PathBuf::from("/home/user")),
            None,
            false,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/home/user/.config")));
    }
}
