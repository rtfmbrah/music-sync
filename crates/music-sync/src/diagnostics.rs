//! Read-only local environment diagnostics.

use std::env;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::AppConfig;
use crate::persistence::{CURRENT_SCHEMA_VERSION, Database};

/// Overall severity of a diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    /// The check succeeded.
    Pass,
    /// The condition is not immediately fatal but needs attention.
    Warning,
    /// The configuration cannot support normal operation.
    Failure,
}

/// One named diagnostic result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticCheck {
    /// Stable machine-readable check name.
    pub name: String,
    /// Check severity.
    pub status: CheckStatus,
    /// Human-readable result in English.
    pub message: String,
}

/// Complete report from local, non-network diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// Individual checks in deterministic order.
    pub checks: Vec<DiagnosticCheck>,
}

impl DoctorReport {
    /// Returns true when no check failed.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.checks
            .iter()
            .all(|check| check.status != CheckStatus::Failure)
    }
}

/// Runs truthful, read-only checks without contacting providers.
#[must_use]
pub fn run_doctor(config: &AppConfig) -> DoctorReport {
    let mut checks = vec![DiagnosticCheck {
        name: "configuration".into(),
        status: CheckStatus::Pass,
        message: "Configuration is valid".into(),
    }];
    checks.push(check_directory(
        "state_directory",
        &config.state_directory,
        true,
    ));
    checks.push(check_directory(
        "library_directory",
        &config.library_directory,
        false,
    ));
    checks.push(check_directory(
        "playlist_directory",
        &config.playlist_directory,
        true,
    ));
    checks.push(check_database(&config.database_path()));
    for tool in ["yt-dlp", "ffmpeg", "ffprobe", "fpcalc"] {
        checks.push(check_tool(tool));
    }
    DoctorReport { checks }
}

fn check_directory(name: &str, path: &Path, may_be_created: bool) -> DiagnosticCheck {
    if path.is_dir() {
        return DiagnosticCheck {
            name: name.into(),
            status: CheckStatus::Pass,
            message: format!("Directory is accessible: {}", path.display()),
        };
    }
    if !path.exists() && may_be_created && existing_parent(path).is_some() {
        return DiagnosticCheck {
            name: name.into(),
            status: CheckStatus::Warning,
            message: format!(
                "Directory does not exist yet and may be created by a future mutating command: {}",
                path.display()
            ),
        };
    }
    DiagnosticCheck {
        name: name.into(),
        status: CheckStatus::Failure,
        message: format!("Directory is not accessible: {}", path.display()),
    }
}

fn existing_parent(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .skip(1)
        .find(|parent| parent.is_dir())
        .map(Path::to_path_buf)
}

fn check_database(path: &Path) -> DiagnosticCheck {
    if !path.exists() {
        return DiagnosticCheck {
            name: "database".into(),
            status: CheckStatus::Warning,
            message: format!("Database has not been initialized: {}", path.display()),
        };
    }
    match Database::inspect_read_only(path) {
        Ok(inspection) if inspection.version == CURRENT_SCHEMA_VERSION => DiagnosticCheck {
            name: "database".into(),
            status: CheckStatus::Pass,
            message: format!("Database is healthy at schema {}", inspection.version),
        },
        Ok(inspection) => DiagnosticCheck {
            name: "database".into(),
            status: CheckStatus::Warning,
            message: format!(
                "Database schema is {}, while this build expects {}",
                inspection.version, CURRENT_SCHEMA_VERSION
            ),
        },
        Err(error) => DiagnosticCheck {
            name: "database".into(),
            status: CheckStatus::Failure,
            message: error.to_string(),
        },
    }
}

fn check_tool(name: &str) -> DiagnosticCheck {
    match find_executable(name) {
        Some(path) => DiagnosticCheck {
            name: format!("tool_{name}"),
            status: CheckStatus::Pass,
            message: format!("Found {name}: {}", path.display()),
        },
        None => DiagnosticCheck {
            name: format!("tool_{name}"),
            status: CheckStatus::Warning,
            message: format!("Optional future media tool is not available on PATH: {name}"),
        },
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConcurrencyConfig, DiscoveryConfig};

    #[test]
    fn doctor_is_read_only_and_reports_missing_library() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let state = root.path().join("state");
        let playlists = root.path().join("playlists");
        let library = root.path().join("missing-library");
        let config = AppConfig {
            state_directory: state.clone(),
            library_directory: library,
            playlist_directory: playlists.clone(),
            concurrency: ConcurrencyConfig::default(),
            discovery: DiscoveryConfig::default(),
        };

        let report = run_doctor(&config);

        assert!(!report.is_healthy());
        assert!(!state.exists());
        assert!(!playlists.exists());
        Ok(())
    }
}
