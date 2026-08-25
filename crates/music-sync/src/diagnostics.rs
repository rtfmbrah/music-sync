//! Read-only local environment diagnostics.

use std::env;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
    if config.service.enabled {
        checks.push(check_tool_path("yt-dlp", &config.service.yt_dlp, true));
        checks.push(check_tool_path("ffmpeg", &config.service.ffmpeg, true));
        checks.push(check_tool_path("ffprobe", &config.service.ffprobe, true));
        checks.push(check_tool_path("fpcalc", &config.service.fpcalc, true));
        if let Some(cookie_file) = &config.service.yt_dlp_cookie_file {
            checks.push(check_secret_file("yt-dlp_cookies", cookie_file));
        }
        checks.push(check_same_filesystem(
            &config.state_directory,
            &config.library_directory,
        ));
        checks.push(check_endpoint_security(config));
        checks.push(check_secret_contract(config));
    } else {
        for tool in ["yt-dlp", "ffmpeg", "ffprobe", "fpcalc"] {
            checks.push(check_tool(tool));
        }
    }
    DoctorReport { checks }
}

fn check_secret_file(name: &str, path: &Path) -> DiagnosticCheck {
    let secure = fs::metadata(path).is_ok_and(|metadata| {
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        return metadata.permissions().mode() & 0o077 == 0;
        #[cfg(not(unix))]
        true
    });
    DiagnosticCheck {
        name: format!("secret_{name}"),
        status: if secure {
            CheckStatus::Pass
        } else {
            CheckStatus::Failure
        },
        message: if secure {
            format!(
                "Secret file exists with restricted permissions: {}",
                path.display()
            )
        } else {
            format!(
                "Secret file must be regular and inaccessible to group/others: {}",
                path.display()
            )
        },
    }
}

fn check_tool_path(name: &str, path: &Path, required: bool) -> DiagnosticCheck {
    let executable = path.is_file() && is_executable(path);
    if executable {
        DiagnosticCheck {
            name: format!("tool_{name}"),
            status: CheckStatus::Pass,
            message: format!("Configured {name} is executable: {}", path.display()),
        }
    } else {
        DiagnosticCheck {
            name: format!("tool_{name}"),
            status: if required {
                CheckStatus::Failure
            } else {
                CheckStatus::Warning
            },
            message: format!(
                "Configured {name} is not an executable file: {}",
                path.display()
            ),
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}
#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn check_same_filesystem(state: &Path, library: &Path) -> DiagnosticCheck {
    let state_anchor = if state.exists() {
        Some(state.to_path_buf())
    } else {
        existing_parent(state)
    };
    let library_anchor = if library.exists() {
        Some(library.to_path_buf())
    } else {
        existing_parent(library)
    };
    #[cfg(unix)]
    if let (Some(state), Some(library)) = (state_anchor, library_anchor)
        && let (Ok(state), Ok(library)) = (fs::metadata(&state), fs::metadata(&library))
    {
        let same = state.dev() == library.dev();
        return DiagnosticCheck {
            name: "staging_library_filesystem".into(),
            status: if same {
                CheckStatus::Pass
            } else {
                CheckStatus::Failure
            },
            message: if same {
                "State staging and library are on the same filesystem".into()
            } else {
                "State staging and library are on different filesystems; atomic hard-link commit cannot work".into()
            },
        };
    }
    DiagnosticCheck {
        name: "staging_library_filesystem".into(),
        status: CheckStatus::Warning,
        message: "Could not determine whether state staging and library share a filesystem".into(),
    }
}

fn check_endpoint_security(config: &AppConfig) -> DiagnosticCheck {
    let endpoints = [
        &config.service.musicbrainz_endpoint,
        &config.service.cover_art_endpoint,
        &config.service.lyrics_endpoint,
        &config.service.listenbrainz_endpoint,
    ];
    let secure = endpoints.iter().all(|endpoint| {
        endpoint.starts_with("https://")
            || endpoint.starts_with("http://127.0.0.1:")
            || endpoint.starts_with("http://[::1]:")
    });
    DiagnosticCheck {
        name: "service_endpoints".into(),
        status: if secure {
            CheckStatus::Pass
        } else {
            CheckStatus::Failure
        },
        message: if secure {
            "Service endpoints use HTTPS (or explicit loopback fixtures)".into()
        } else {
            "Every service endpoint must use HTTPS outside loopback fixtures".into()
        },
    }
}

fn check_secret_contract(config: &AppConfig) -> DiagnosticCheck {
    if config.discovery.navidrome_url.is_none() {
        return DiagnosticCheck {
            name: "navidrome_secrets".into(),
            status: CheckStatus::Pass,
            message: "Navidrome taste import is not configured".into(),
        };
    }
    let present =
        env::var_os("NAVIDROME_TOKEN").is_some() && env::var_os("NAVIDROME_SALT").is_some();
    DiagnosticCheck {
        name: "navidrome_secrets".into(),
        status: if present {
            CheckStatus::Pass
        } else {
            CheckStatus::Failure
        },
        message: if present {
            "Required Navidrome secret variables are present".into()
        } else {
            "NAVIDROME_TOKEN and NAVIDROME_SALT are required without exposing their values".into()
        },
    }
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
            service: Default::default(),
        };

        let report = run_doctor(&config);

        assert!(!report.is_healthy());
        assert!(!state.exists());
        assert!(!playlists.exists());
        Ok(())
    }

    #[test]
    fn enabled_service_requires_explicit_executable_tools() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = tempfile::tempdir()?;
        let library = root.path().join("library");
        let playlists = root.path().join("playlists");
        fs::create_dir(&library)?;
        fs::create_dir(&playlists)?;
        let mut config = AppConfig {
            state_directory: root.path().join("state"),
            library_directory: library,
            playlist_directory: playlists,
            concurrency: Default::default(),
            discovery: Default::default(),
            service: Default::default(),
        };
        config.service.enabled = true;
        config.service.user_agent = Some("fixture@example.invalid".into());
        config.service.yt_dlp = "/missing/yt-dlp".into();
        config.service.ffmpeg = "/missing/ffmpeg".into();
        config.service.ffprobe = "/missing/ffprobe".into();
        config.service.fpcalc = "/missing/fpcalc".into();
        let report = run_doctor(&config);
        assert!(!report.is_healthy());
        assert_eq!(
            report
                .checks
                .iter()
                .filter(
                    |check| check.name.starts_with("tool_") && check.status == CheckStatus::Failure
                )
                .count(),
            4
        );
        Ok(())
    }
}
