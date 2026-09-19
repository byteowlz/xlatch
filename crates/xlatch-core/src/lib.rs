//! `CrossLatch` contracts, authorization, persistence, local control, and execution.

pub mod config;
pub mod error;
pub mod paths;
mod save_file;
pub mod schema;

pub use config::{AppConfig, LogLevel, LoggingConfig, PathsConfig, RuntimeConfig};
pub use error::{CoreError, Result};
pub use paths::{AppPaths, default_cache_dir};
pub use schema::{generate_example_config, generate_schema, write_generated_files};

/// Application name used for config directories and environment prefix.
/// Override this constant when scaffolding a new project.
pub const APP_NAME: &str = "xlatch";

/// Returns the environment variable prefix for this application.
#[must_use]
pub fn env_prefix() -> String {
    APP_NAME
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Returns the default parallelism based on available CPU cores.
#[must_use]
pub fn default_parallelism() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

pub mod approval;
pub mod auth;
pub mod capability;
pub mod enrollment;
pub mod execution;
pub mod executor;
pub mod local;
pub mod service;
pub mod store;
pub mod worker;

pub mod history;

#[cfg(unix)]
pub mod host_trust;

pub mod recovery;
