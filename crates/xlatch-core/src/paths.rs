//! XDG-compliant path resolution for application directories.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::{APP_NAME, AppConfig};

/// Application paths for config, data, and state directories.
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Path to the configuration file.
    pub config_file: PathBuf,
    /// Directory for persistent application data.
    pub data_dir: PathBuf,
    /// Directory for application state files.
    pub state_dir: PathBuf,
}

impl AppPaths {
    /// Discover application paths, optionally overriding the config file location.
    ///
    /// # Errors
    ///
    /// Returns an error if paths cannot be resolved or expanded.
    pub fn discover(override_path: Option<&Path>) -> Result<Self> {
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

        if config_file.parent().is_none() {
            return Err(anyhow!(
                "invalid config file path: {}",
                config_file.display()
            ));
        }

        let data_dir = default_data_dir()?;
        let state_dir = default_state_dir()?;

        Ok(Self {
            config_file,
            data_dir,
            state_dir,
        })
    }

    /// Apply path overrides from configuration.
    ///
    /// # Errors
    ///
    /// Returns an error if override paths cannot be expanded.
    pub fn apply_overrides(mut self, cfg: &AppConfig) -> Result<Self> {
        if let Some(ref data_override) = cfg.paths.data_dir {
            self.data_dir = expand_str_path(data_override)?;
        }
        if let Some(ref state_override) = cfg.paths.state_dir {
            self.state_dir = expand_str_path(state_override)?;
        }
        Ok(self)
    }

    /// Ensure all required directories exist.
    ///
    /// # Errors
    ///
    /// Returns an error if directories cannot be created.
    pub fn ensure_directories(&self) -> Result<()> {
        fs::create_dir_all(&self.data_dir)
            .with_context(|| format!("creating data directory {}", self.data_dir.display()))?;
        fs::create_dir_all(&self.state_dir)
            .with_context(|| format!("creating state directory {}", self.state_dir.display()))?;
        Ok(())
    }

    /// Log directory creation in dry-run mode.
    pub fn log_dry_run(&self) {
        log::info!(
            "dry-run: would ensure data dir {} and state dir {}",
            self.data_dir.display(),
            self.state_dir.display()
        );
    }
}

impl std::fmt::Display for AppPaths {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "config: {}, data: {}, state: {}",
            self.config_file.display(),
            self.data_dir.display(),
            self.state_dir.display()
        )
    }
}

/// Expand a `PathBuf`, resolving ~ and environment variables.
///
/// # Errors
///
/// Returns an error if shell expansion fails.
pub fn expand_path(path: &Path) -> Result<PathBuf> {
    path.to_str()
        .map_or_else(|| Ok(path.to_path_buf()), expand_str_path)
}

/// Expand a string path, resolving ~ and environment variables.
///
/// # Errors
///
/// Returns an error if shell expansion fails.
pub fn expand_str_path(text: &str) -> Result<PathBuf> {
    let expanded = shellexpand::full(text).context("expanding path")?;
    Ok(PathBuf::from(expanded.to_string()))
}

/// Resolve a base directory deterministically (pure; unit-tested below).
///
/// An explicit, absolute `XDG_*` path wins on any OS; otherwise `~/<unix_rel>`
/// on all unix **including macOS** (deliberately NOT `~/Library/Application
/// Support` for CLI tools), or the Windows known directory.
fn resolve_base(
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
    win_dir: Option<PathBuf>,
    is_windows: bool,
    unix_rel: &str,
) -> Option<PathBuf> {
    if let Some(path) = xdg.filter(|p| p.is_absolute()) {
        return Some(path);
    }
    if is_windows {
        win_dir
    } else {
        home.map(|home| home.join(unix_rel))
    }
}

/// Read the relevant env vars and resolve a base dir via [`resolve_base`].
/// Zero-dependency: no `dirs` crate (whose macOS default we deliberately avoid).
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
///
/// `XDG_CONFIG_HOME` wins on any OS; else `~/.config` on unix (incl. macOS) or
/// `%APPDATA%` on Windows.
///
/// # Errors
///
/// Returns an error if no base directory can be determined.
pub fn default_config_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_CONFIG_HOME", ".config", "APPDATA")?.join(APP_NAME))
}

/// Get the default data directory (`XDG_DATA_HOME`; else `~/.local/share` / `%APPDATA%`).
///
/// # Errors
///
/// Returns an error if no base directory can be determined.
pub fn default_data_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_DATA_HOME", ".local/share", "APPDATA")?.join(APP_NAME))
}

/// Get the default state directory (`XDG_STATE_HOME`; else `~/.local/state` / `%LOCALAPPDATA%`).
///
/// # Errors
///
/// Returns an error if no base directory can be determined.
pub fn default_state_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")?.join(APP_NAME))
}

/// Get the default cache directory (`XDG_CACHE_HOME`; else `~/.cache` / `%LOCALAPPDATA%`).
///
/// # Errors
///
/// Returns an error if no base directory can be determined.
pub fn default_cache_dir() -> Result<PathBuf> {
    Ok(base_dir("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA")?.join(APP_NAME))
}

/// Write the default configuration file to the specified path.
///
/// # Errors
///
/// Returns an error if the file cannot be written or the directory cannot be created.
pub fn write_default_config(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating config directory {}", parent.display()))?;
    }

    let config = AppConfig::default();
    let toml_str = toml::to_string_pretty(&config).context("serializing default config to TOML")?;
    let mut body = default_config_header(path);
    body.push_str(&toml_str);
    fs::write(path, body).with_context(|| format!("writing config file to {}", path.display()))
}

fn default_config_header(path: &Path) -> String {
    let mut buffer = String::new();
    buffer.push_str("# Configuration for ");
    buffer.push_str(APP_NAME);
    buffer.push('\n');
    buffer.push_str("# File: ");
    buffer.push_str(&path.display().to_string());
    buffer.push('\n');
    buffer.push('\n');
    buffer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_absolute_path_wins_on_any_os() {
        let got = resolve_base(
            Some(PathBuf::from("/explicit/xdg")),
            Some(PathBuf::from("/home/u")),
            Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
            true,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("/explicit/xdg")));
    }

    #[test]
    fn unix_incl_macos_uses_home_dotpath_not_library() {
        // is_windows = false covers Linux AND macOS — deliberately ~/.config, never ~/Library.
        let got = resolve_base(None, Some(PathBuf::from("/home/u")), None, false, ".config");
        assert_eq!(got, Some(PathBuf::from("/home/u/.config")));
    }

    #[test]
    fn windows_uses_known_dir() {
        let got = resolve_base(
            None,
            Some(PathBuf::from("C:/Users/u")),
            Some(PathBuf::from("C:/Users/u/AppData/Roaming")),
            true,
            ".config",
        );
        assert_eq!(got, Some(PathBuf::from("C:/Users/u/AppData/Roaming")));
    }

    #[test]
    fn relative_xdg_is_ignored() {
        let got = resolve_base(
            Some(PathBuf::from("relative/path")),
            Some(PathBuf::from("/home/u")),
            None,
            false,
            ".local/state",
        );
        assert_eq!(got, Some(PathBuf::from("/home/u/.local/state")));
    }
}
