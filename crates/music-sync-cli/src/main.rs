//! Terminal entry point for music-sync.

#![forbid(unsafe_code)]

use std::error::Error;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{ArgAction, Parser, Subcommand};
use music_sync::acquisition::{
    AcquisitionRunOutcome, run_one_acquisition, run_pending_acquisitions,
};
use music_sync::adoption::{
    AdoptionApplyReport, AdoptionReport, HashLimit, ProbeLimit, apply_library, scan_library,
    scan_library_with_hash, scan_library_with_probe, scan_library_with_probe_and_hash,
};
use music_sync::config::AppConfig;
use music_sync::content_hash::Sha256FileHasher;
use music_sync::diagnostics::{CheckStatus, DoctorReport, run_doctor};
use music_sync::fingerprint::{Fpcalc, reconcile_artifact_fingerprints};
use music_sync::health::reconcile_artifact_health;
use music_sync::media_probe::Ffprobe;
use music_sync::persistence::Database;
use music_sync::playlist::{PlaylistMaterializationReport, materialize_playlists};
use music_sync::provider::{SourceId, SourceSnapshot, is_supported_youtube_url};
use music_sync::repair::{assess_repair_eligibility, generate_repair_candidates};
use music_sync::sync::{
    SourceSyncResult, SyncBoundaries, SyncDirectories, SyncLimits, SyncRunReport, run_sync,
};
use music_sync::yt_dlp::YtDlp;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "music-sync",
    version,
    about = "Resilient, provider-agnostic music library management",
    long_about = None
)]
struct Cli {
    /// Increase log verbosity (-v verbose, -vv debug, -vvv trace).
    #[arg(short, long, action = ArgAction::Count, global = true)]
    verbose: u8,

    /// Emit machine-readable JSON where supported.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check configuration, local paths, database health, and media tools.
    Doctor {
        /// TOML configuration file to validate.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Inspect or adopt an existing generic music library.
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    /// Inspect remote source membership without downloading media.
    Source {
        #[command(subcommand)]
        command: SourceCommand,
    },
    /// Process durable media acquisition jobs.
    Acquisition {
        #[command(subcommand)]
        command: AcquisitionCommand,
    },
    /// Assess and process conservative lost-media repair cases.
    Repair {
        #[command(subcommand)]
        command: RepairCommand,
    },
    /// Materialize Navidrome-compatible playlists from durable collections.
    Playlist {
        #[command(subcommand)]
        command: PlaylistCommand,
    },
    /// Run one bounded source, acquisition, and playlist synchronization cycle.
    Sync {
        #[command(subcommand)]
        command: SyncCommand,
    },
    /// Show read-only durable operational state without contacting providers.
    Status {
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum newest warning/error events to show (1 through 100).
        #[arg(long, default_value = "20")]
        recent_events: NonZeroUsize,
    },
}

#[derive(Debug, Subcommand)]
enum RepairCommand {
    /// Check unhealthy originals and persist only definitive repair eligibility.
    Assess {
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum unhealthy originals checked once in stable order.
        #[arg(long, default_value = "100")]
        max_items: NonZeroUsize,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// Per-original provider deadline in seconds.
        #[arg(long, default_value = "60")]
        timeout_seconds: NonZeroU64,
    },
    /// Generate search candidates without treating text results as identity evidence.
    Generate {
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum eligible repair cases processed once.
        #[arg(long, default_value = "20")]
        max_cases: NonZeroUsize,
        /// Maximum provider candidates retained per case.
        #[arg(long, default_value = "5")]
        max_candidates: NonZeroUsize,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// Per-search provider deadline in seconds.
        #[arg(long, default_value = "60")]
        timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum SyncCommand {
    /// Reconcile active sources, acquire pending media, and materialize playlists.
    Run {
        /// TOML configuration containing all application directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum acquisition jobs attempted once during this run.
        #[arg(long, default_value = "100")]
        max_jobs: NonZeroUsize,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// ffprobe executable path.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
        /// Per-source enumeration deadline in seconds.
        #[arg(long, default_value = "60")]
        source_timeout_seconds: NonZeroU64,
        /// Per-job yt-dlp deadline in seconds.
        #[arg(long, default_value = "600")]
        download_timeout_seconds: NonZeroU64,
        /// Per-job ffprobe deadline in seconds.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
        /// Maximum healthy artifacts fingerprint-reconciled after acquisition.
        #[arg(long, default_value = "100")]
        max_fingerprints: NonZeroUsize,
        /// Maximum audio seconds consumed per fingerprint.
        #[arg(long, default_value = "120")]
        fingerprint_audio_seconds: NonZeroU64,
        /// fpcalc executable path.
        #[arg(long, default_value = "fpcalc")]
        fpcalc: PathBuf,
        /// Per-artifact fpcalc deadline in seconds.
        #[arg(long, default_value = "60")]
        fingerprint_timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum PlaylistCommand {
    /// Atomically materialize all reconciled collections as M3U8 files.
    Materialize {
        /// TOML configuration containing state, library, and playlist directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum AcquisitionCommand {
    /// Show bounded newest-first durable acquisition history without mutation.
    History {
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum history entries to return (1 through 1000).
        #[arg(long, default_value = "100")]
        limit: NonZeroUsize,
    },
    /// Explicitly release one deferred acquisition for a future run.
    Retry {
        /// Durable deferred acquisition job ID.
        job_id: i64,
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Mark abandoned running acquisitions deferred after confirming no sync is active.
    RecoverRunning {
        /// TOML configuration identifying the application database.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Claim and process at most one pending or deferred acquisition.
    RunOne {
        /// TOML configuration containing state and managed library directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// ffprobe executable path.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
        /// yt-dlp deadline in seconds.
        #[arg(long, default_value = "600")]
        download_timeout_seconds: NonZeroU64,
        /// ffprobe deadline in seconds.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
    },
    /// Process a bounded snapshot of pending and deferred acquisitions.
    RunPending {
        /// TOML configuration containing state and managed library directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum jobs attempted once during this batch.
        #[arg(long, default_value = "100")]
        max_jobs: NonZeroUsize,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// ffprobe executable path.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
        /// yt-dlp deadline in seconds per job.
        #[arg(long, default_value = "600")]
        download_timeout_seconds: NonZeroU64,
        /// ffprobe deadline in seconds per job.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum SourceCommand {
    /// Add or reactivate a persistent YouTube source.
    Add {
        /// HTTPS YouTube video or playlist URL.
        url: String,
        /// music-sync-owned SQLite database.
        #[arg(long)]
        database: PathBuf,
        /// Optional user-facing source name.
        #[arg(long)]
        name: Option<String>,
    },
    /// List configured active sources.
    List {
        /// music-sync-owned SQLite database.
        #[arg(long)]
        database: PathBuf,
        /// Include inactive sources.
        #[arg(long)]
        all: bool,
    },
    /// Deactivate a source without deleting history or media.
    Remove {
        /// Durable source ID.
        id: i64,
        /// music-sync-owned SQLite database.
        #[arg(long)]
        database: PathBuf,
    },
    /// Enumerate and transactionally reconcile one active configured source.
    Reconcile {
        /// Durable source ID.
        id: i64,
        /// music-sync-owned SQLite database.
        #[arg(long)]
        database: PathBuf,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// Enumeration deadline in seconds.
        #[arg(long, default_value = "60")]
        timeout_seconds: NonZeroU64,
    },
    /// Enumerate one YouTube video or playlist through yt-dlp.
    Enumerate {
        /// YouTube video or playlist URL.
        url: String,
        /// yt-dlp executable path.
        #[arg(long, default_value = "yt-dlp")]
        yt_dlp: PathBuf,
        /// Enumeration deadline in seconds.
        #[arg(long, default_value = "60")]
        timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum LibraryCommand {
    /// Derive bounded raw Chromaprint evidence for healthy registered artifacts.
    Fingerprint {
        /// TOML configuration containing state and library directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum healthy artifacts selected in stable ID order.
        #[arg(long, default_value = "100")]
        max_artifacts: NonZeroUsize,
        /// Maximum audio seconds fingerprinted per artifact.
        #[arg(long, default_value = "120")]
        max_audio_seconds: NonZeroU64,
        /// fpcalc executable path.
        #[arg(long, default_value = "fpcalc")]
        fpcalc: PathBuf,
        /// Per-artifact fpcalc deadline in seconds.
        #[arg(long, default_value = "60")]
        timeout_seconds: NonZeroU64,
    },
    /// Reconcile registered artifact health without modifying media files.
    Health {
        /// TOML configuration containing state and library directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum artifacts checked in stable ID order.
        #[arg(long, default_value = "1000")]
        max_artifacts: NonZeroUsize,
        /// ffprobe executable path.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
        /// Per-artifact ffprobe deadline in seconds.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
    },
    /// Scan a library recursively without modifying any file.
    Adopt {
        /// Existing music-library directory to scan.
        path: PathBuf,
        /// Inspect recognized media with ffprobe; remains read-only.
        #[arg(long)]
        probe: bool,
        /// Probe at most this many media files, in deterministic path order.
        #[arg(long, requires = "probe")]
        max_probes: Option<NonZeroUsize>,
        /// Stream recognized media through SHA-256; remains read-only.
        #[arg(long)]
        hash: bool,
        /// Hash at most this many media files, in deterministic path order.
        #[arg(long, requires = "hash")]
        max_hashes: Option<NonZeroUsize>,
        /// Per-file ffprobe timeout in seconds.
        #[arg(long, default_value = "10", requires = "probe")]
        probe_timeout_seconds: NonZeroU64,
        /// Persist recognized media transactionally in music-sync SQLite.
        #[arg(long, requires = "database", conflicts_with_all = ["probe", "hash"])]
        apply: bool,
        /// music-sync-owned SQLite database used only with explicit apply.
        #[arg(long, requires = "apply")]
        database: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    initialize_logging(cli.verbose);
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            error!(error = %error, "command failed");
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn Error>> {
    match cli.command {
        Command::Repair {
            command:
                RepairCommand::Generate {
                    config,
                    max_cases,
                    max_candidates,
                    yt_dlp,
                    timeout_seconds,
                },
        } => {
            if max_candidates.get() > 50 {
                return Err("maximum repair candidates per case must not exceed 50".into());
            }
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let adapter = YtDlp::new(yt_dlp, Duration::from_secs(timeout_seconds.get()));
            let report = generate_repair_candidates(
                &mut database,
                &adapter,
                max_cases.get(),
                max_candidates.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Repair cases selected: {}", report.selected_cases);
                println!("Candidates generated:  {}", report.generated);
                println!("Generation failures:   {}", report.failures.len());
                for failure in &report.failures {
                    println!(
                        "  Provider item {}: {}",
                        failure.provider_item_id, failure.message
                    );
                }
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Repair {
            command:
                RepairCommand::Assess {
                    config,
                    max_items,
                    yt_dlp,
                    timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let adapter = YtDlp::new(yt_dlp, Duration::from_secs(timeout_seconds.get()));
            let report = assess_repair_eligibility(&mut database, &adapter, max_items.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Originals selected:              {}", report.selected);
                println!("Originals available:             {}", report.available);
                println!(
                    "Originals permanently unavailable: {}",
                    report.permanently_unavailable
                );
                println!(
                    "Availability changes:            {}",
                    report.availability_changed
                );
                println!(
                    "Repair cases inserted:           {}",
                    report.eligibility.inserted
                );
                println!(
                    "Repair cases reopened:           {}",
                    report.eligibility.reopened
                );
                println!(
                    "Repair cases cancelled:          {}",
                    report.eligibility.cancelled
                );
                println!("Assessment failures:             {}", report.failures.len());
                for failure in &report.failures {
                    println!(
                        "  Provider item {}: {}",
                        failure.provider_item_id, failure.message
                    );
                }
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Status {
            config,
            recent_events,
        } => {
            if recent_events.get() > 100 {
                return Err("recent event limit must not exceed 100".into());
            }
            let config = AppConfig::from_file(&config)?;
            let report = Database::operational_status_read_only(
                &config.database_path(),
                recent_events.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Schema version:          {}", report.schema_version);
                println!("Sources active:          {}", report.active_sources);
                println!("Sources inactive:        {}", report.inactive_sources);
                println!("Collections:             {}", report.collections);
                println!("Memberships active:      {}", report.active_memberships);
                println!(
                    "Memberships unresolved:  {}",
                    report.unresolved_active_memberships
                );
                println!("Jobs pending:            {}", report.jobs.pending);
                println!("Jobs running:            {}", report.jobs.running);
                println!("Jobs succeeded:          {}", report.jobs.succeeded);
                println!("Jobs failed:             {}", report.jobs.failed);
                println!("Jobs deferred:           {}", report.jobs.deferred);
                println!("Artifacts healthy:       {}", report.artifacts.healthy);
                println!("Artifacts unknown:       {}", report.artifacts.unknown);
                println!("Artifacts missing:       {}", report.artifacts.missing);
                println!("Artifacts corrupt:       {}", report.artifacts.corrupt);
                println!("Repairs eligible:        {}", report.repairs.eligible);
                println!("Repairs unresolved:      {}", report.repairs.unresolved);
                println!("Repairs verified:        {}", report.repairs.verified);
                println!("Repairs cancelled:       {}", report.repairs.cancelled);
                println!("Playlist outputs:        {}", report.playlist_outputs);
                println!("Recent warning/errors:   {}", report.recent_events.len());
                for event in report.recent_events {
                    println!(
                        "  {} {} {} {}: {}",
                        event.created_at, event.level, event.component, event.event, event.message
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Doctor { config } => {
            info!(config = %config.display(), "running local diagnostics");
            let config = AppConfig::from_file(&config)?;
            let report = run_doctor(&config);
            render_doctor(&report, cli.json)?;
            Ok(if report.is_healthy() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Library {
            command:
                LibraryCommand::Fingerprint {
                    config,
                    max_artifacts,
                    max_audio_seconds,
                    fpcalc,
                    timeout_seconds,
                },
        } => {
            let maximum_audio_seconds = u32::try_from(max_audio_seconds.get())
                .map_err(|_| "maximum fingerprint audio seconds exceeds u32")?;
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let adapter = Fpcalc::new(
                fpcalc,
                Duration::from_secs(timeout_seconds.get()),
                maximum_audio_seconds,
            );
            let report = reconcile_artifact_fingerprints(
                &mut database,
                &config.library_directory,
                &adapter,
                max_artifacts.get(),
                maximum_audio_seconds,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Artifacts selected: {}", report.selected);
                println!("Fingerprints recorded: {}", report.recorded);
                println!("Fingerprints unchanged: {}", report.unchanged);
                println!("Fingerprint failures: {}", report.failures.len());
                for failure in &report.failures {
                    println!("  Artifact {}: {}", failure.artifact_id, failure.message);
                }
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Library {
            command:
                LibraryCommand::Health {
                    config,
                    max_artifacts,
                    ffprobe,
                    probe_timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let report = reconcile_artifact_health(
                &mut database,
                &config.library_directory,
                &probe,
                &Sha256FileHasher,
                max_artifacts.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Artifacts selected: {}", report.selected);
                println!("Artifacts checked:  {}", report.checked);
                println!("Health changed:      {}", report.changed);
                println!("Healthy:             {}", report.healthy);
                println!("Missing:             {}", report.missing);
                println!("Corrupt:             {}", report.corrupt);
                println!("Check failures:      {}", report.failures.len());
                for failure in &report.failures {
                    println!("  Artifact {}: {}", failure.artifact_id, failure.message);
                }
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Library {
            command:
                LibraryCommand::Adopt {
                    path,
                    probe,
                    max_probes,
                    hash,
                    max_hashes,
                    probe_timeout_seconds,
                    apply,
                    database,
                },
        } => {
            info!(path = %path.display(), "scanning library for adoption");
            if apply {
                let database_path = database.ok_or("--apply requires --database")?;
                let mut database = Database::open(&database_path)?;
                let report = apply_library(&path, &mut database)?;
                render_adoption_apply(&report, cli.json)?;
                return Ok(ExitCode::SUCCESS);
            }
            let probe_limit = max_probes.map_or(ProbeLimit::All, ProbeLimit::Files);
            let hash_limit = max_hashes.map_or(HashLimit::All, HashLimit::Files);
            let report = if probe && hash {
                let adapter = Ffprobe::new(
                    PathBuf::from("ffprobe"),
                    Duration::from_secs(probe_timeout_seconds.get()),
                );
                scan_library_with_probe_and_hash(
                    &path,
                    &adapter,
                    probe_limit,
                    &Sha256FileHasher,
                    hash_limit,
                )?
            } else if probe {
                let adapter = Ffprobe::new(
                    PathBuf::from("ffprobe"),
                    Duration::from_secs(probe_timeout_seconds.get()),
                );
                scan_library_with_probe(&path, &adapter, probe_limit)?
            } else if hash {
                scan_library_with_hash(&path, &Sha256FileHasher, hash_limit)?
            } else {
                scan_library(&path)?
            };
            render_adoption(&report, cli.json, "music-sync library adopt (dry run)")?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Source {
            command:
                SourceCommand::Enumerate {
                    url,
                    yt_dlp,
                    timeout_seconds,
                },
        } => {
            let adapter = YtDlp::new(yt_dlp, Duration::from_secs(timeout_seconds.get()));
            let snapshot = adapter.enumerate(&url)?;
            render_source_snapshot(&snapshot, cli.json)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Source {
            command:
                SourceCommand::Add {
                    url,
                    database,
                    name,
                },
        } => {
            if !is_supported_youtube_url(&url) {
                return Err("source URL must be an HTTPS youtube.com or youtu.be URL".into());
            }
            if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
                return Err("source name must not be empty".into());
            }
            let mut database = Database::open(&database)?;
            let result = database.add_source(&url, name.as_deref())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("Source ID:   {}", result.id.0);
                println!("Inserted:    {}", result.inserted);
                println!("Reactivated: {}", result.reactivated);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Source {
            command: SourceCommand::List { database, all },
        } => {
            let database = Database::open(&database)?;
            let sources = database.list_sources(all)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&sources)?);
            } else if sources.is_empty() {
                println!("No configured sources.");
            } else {
                for source in sources {
                    println!(
                        "{}  {}  {}  {}",
                        source.id.0,
                        if source.active { "active" } else { "inactive" },
                        source.name.as_deref().unwrap_or("[unnamed]"),
                        source.url
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Source {
            command: SourceCommand::Remove { id, database },
        } => {
            if id <= 0 {
                return Err("source ID must be greater than zero".into());
            }
            let mut database = Database::open(&database)?;
            let deactivated = database.deactivate_source(SourceId(id))?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "id": id,
                        "deactivated": deactivated
                    }))?
                );
            } else {
                println!("Source ID:   {id}");
                println!("Deactivated: {deactivated}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Source {
            command:
                SourceCommand::Reconcile {
                    id,
                    database,
                    yt_dlp,
                    timeout_seconds,
                },
        } => {
            if id <= 0 {
                return Err("source ID must be greater than zero".into());
            }
            let source_id = SourceId(id);
            let mut database = Database::open(&database)?;
            let source = database
                .get_source(source_id)?
                .ok_or("configured source does not exist")?;
            if !source.active {
                return Err("configured source is inactive".into());
            }
            let adapter = YtDlp::new(yt_dlp, Duration::from_secs(timeout_seconds.get()));
            let snapshot = adapter.enumerate(&source.url)?;
            let summary = database.reconcile_source_snapshot(source_id, &snapshot)?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!(
                    "Provider items inserted:  {}",
                    summary.provider_items_inserted
                );
                println!(
                    "Provider items existing:  {}",
                    summary.provider_items_existing
                );
                println!(
                    "Memberships activated:    {}",
                    summary.memberships_activated
                );
                println!(
                    "Memberships deactivated:  {}",
                    summary.memberships_deactivated
                );
                println!(
                    "Memberships unchanged:    {}",
                    summary.memberships_unchanged
                );
                println!("Acquisition jobs created: {}", summary.jobs_created);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Acquisition {
            command: AcquisitionCommand::History { config, limit },
        } => {
            if limit.get() > 1000 {
                return Err("acquisition history limit must not exceed 1000".into());
            }
            let config = AppConfig::from_file(&config)?;
            let entries =
                Database::acquisition_history_read_only(&config.database_path(), limit.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else if entries.is_empty() {
                println!("No acquisition history.");
            } else {
                for entry in entries {
                    println!(
                        "Job {}  {}  attempts={}  {}:{}",
                        entry.job_id,
                        entry.status,
                        entry.attempt_count,
                        entry.provider,
                        entry.provider_item_id
                    );
                    if let Some(message) = entry.latest_message {
                        println!("  {message}");
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Acquisition {
            command: AcquisitionCommand::Retry { job_id, config },
        } => {
            if job_id <= 0 {
                return Err("acquisition job ID must be greater than zero".into());
            }
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let released = database.retry_deferred_acquisition(job_id)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "job_id": job_id,
                        "released": released
                    }))?
                );
            } else {
                println!("Acquisition job: {job_id}");
                println!("Released:        {released}");
            }
            Ok(if released {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Acquisition {
            command: AcquisitionCommand::RecoverRunning { config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let recovered = database.recover_interrupted_acquisitions()?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "recovered": recovered
                    }))?
                );
            } else {
                println!("Running acquisitions recovered: {recovered}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Acquisition {
            command:
                AcquisitionCommand::RunOne {
                    config,
                    yt_dlp,
                    ffprobe,
                    download_timeout_seconds,
                    probe_timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let downloader =
                YtDlp::new(yt_dlp, Duration::from_secs(download_timeout_seconds.get()));
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let outcome = run_one_acquisition(
                &mut database,
                &config.state_directory,
                &config.library_directory,
                &downloader,
                &probe,
                &Sha256FileHasher,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                match outcome {
                    AcquisitionRunOutcome::Idle => println!("No acquisition work available."),
                    AcquisitionRunOutcome::Committed { work, commit } => {
                        println!("Acquisition job: {}", work.job_id);
                        println!("Provider item:   {}", work.provider_item_id);
                        println!("Artifact:        {}", commit.final_path.display());
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Acquisition {
            command:
                AcquisitionCommand::RunPending {
                    config,
                    max_jobs,
                    yt_dlp,
                    ffprobe,
                    download_timeout_seconds,
                    probe_timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let downloader =
                YtDlp::new(yt_dlp, Duration::from_secs(download_timeout_seconds.get()));
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let report = run_pending_acquisitions(
                &mut database,
                &config.state_directory,
                &config.library_directory,
                &downloader,
                &probe,
                &Sha256FileHasher,
                max_jobs.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Jobs selected:  {}", report.selected);
                println!("Jobs committed: {}", report.committed);
                println!("Jobs failed:    {}", report.failures.len());
                println!("Jobs skipped:   {}", report.skipped);
                for failure in report.failures {
                    println!("  Job {}: {}", failure.job_id, failure.message);
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Playlist {
            command: PlaylistCommand::Materialize { config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let report = materialize_playlists(
                &mut database,
                &config.library_directory,
                &config.playlist_directory,
            )?;
            render_playlist_materialization(&report, cli.json)?;
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Sync {
            command:
                SyncCommand::Run {
                    config,
                    max_jobs,
                    yt_dlp,
                    ffprobe,
                    source_timeout_seconds,
                    download_timeout_seconds,
                    probe_timeout_seconds,
                    max_fingerprints,
                    fingerprint_audio_seconds,
                    fpcalc,
                    fingerprint_timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let source_adapter = YtDlp::new(
                yt_dlp.clone(),
                Duration::from_secs(source_timeout_seconds.get()),
            );
            let acquisition_adapter =
                YtDlp::new(yt_dlp, Duration::from_secs(download_timeout_seconds.get()));
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let fingerprint_audio_seconds = u32::try_from(fingerprint_audio_seconds.get())
                .map_err(|_| "maximum fingerprint audio seconds exceeds u32")?;
            let fingerprinter = Fpcalc::new(
                fpcalc,
                Duration::from_secs(fingerprint_timeout_seconds.get()),
                fingerprint_audio_seconds,
            );
            let report = run_sync(
                &mut database,
                SyncDirectories {
                    state: &config.state_directory,
                    library: &config.library_directory,
                    playlists: &config.playlist_directory,
                },
                SyncBoundaries {
                    source_adapter: &source_adapter,
                    acquisition_adapter: &acquisition_adapter,
                    probe: &probe,
                    hasher: &Sha256FileHasher,
                    fingerprinter: &fingerprinter,
                },
                SyncLimits {
                    maximum_jobs: max_jobs.get(),
                    maximum_fingerprints: max_fingerprints.get(),
                    fingerprint_audio_seconds,
                },
            )?;
            render_sync_run(&report, cli.json)?;
            Ok(if report.is_successful() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
    }
}

fn render_sync_run(report: &SyncRunReport, json: bool) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    let reconciled = report
        .sources
        .iter()
        .filter(|result| matches!(result, SourceSyncResult::Reconciled { .. }))
        .count();
    println!("Sources selected:   {}", report.sources.len());
    println!("Sources reconciled: {reconciled}");
    println!("Sources failed:     {}", report.sources.len() - reconciled);
    for source in &report.sources {
        if let SourceSyncResult::Failed { source_id, message } = source {
            println!("  Source {}: {message}", source_id.0);
        }
    }
    println!("Jobs selected:      {}", report.acquisitions.selected);
    println!("Jobs committed:     {}", report.acquisitions.committed);
    println!("Jobs failed:        {}", report.acquisitions.failures.len());
    println!("Fingerprints saved: {}", report.fingerprints.recorded);
    println!("Fingerprint errors: {}", report.fingerprints.failures.len());
    println!("Playlists written:  {}", report.playlists.playlists.len());
    println!("Playlists failed:   {}", report.playlists.failures.len());
    Ok(())
}

fn render_playlist_materialization(
    report: &PlaylistMaterializationReport,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("Playlists materialized: {}", report.playlists.len());
    println!("Playlists failed:       {}", report.failures.len());
    for playlist in &report.playlists {
        println!(
            "  Collection {}: {} entries, {} omitted, {:?}: {}",
            playlist.collection_id,
            playlist.entries,
            playlist.omitted,
            playlist.effect,
            playlist.path.display()
        );
    }
    for failure in &report.failures {
        println!(
            "  Collection {} failed: {}",
            failure.collection_id, failure.message
        );
    }
    Ok(())
}

fn render_source_snapshot(snapshot: &SourceSnapshot, json: bool) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(snapshot)?);
        return Ok(());
    }
    println!("music-sync source enumerate");
    println!("Provider:   {}", snapshot.provider);
    println!(
        "Collection: {}",
        snapshot
            .provider_collection_id
            .as_deref()
            .unwrap_or("single item")
    );
    println!(
        "Title:      {}",
        snapshot.title.as_deref().unwrap_or("[unknown]")
    );
    println!("Items:      {}", snapshot.items.len());
    for item in &snapshot.items {
        println!(
            "  {}  {}",
            item.provider_item_id,
            item.title.as_deref().unwrap_or("[unknown]")
        );
    }
    Ok(())
}

fn render_adoption(
    report: &AdoptionReport,
    json: bool,
    heading: &str,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("{heading}");
    println!("Library:             {}", report.root);
    println!("Directories scanned: {}", report.directories_scanned);
    println!("Files scanned:       {}", report.files_scanned);
    println!("Media files:         {}", report.media_files);
    println!("Lyrics:              {}", report.lyric_files);
    println!("Artwork:             {}", report.artwork_files);
    println!("Playlists:           {}", report.playlist_files);
    println!("Unknown (preserved): {}", report.unknown_files);
    println!("Symbolic links:      {}", report.symbolic_links);
    println!("Corrupt media:       {}", report.corrupt_media);
    println!("Unreadable paths:    {}", report.unreadable_paths);
    println!("Relationships:");
    println!(
        "  Lyrics associated:  {}",
        report.relationships.lyrics_associated
    );
    println!(
        "  Lyrics orphaned:    {}",
        report.relationships.lyrics_orphaned
    );
    println!(
        "  Artwork beside media: {}",
        report.relationships.artwork_beside_media
    );
    println!(
        "  Artwork orphaned:   {}",
        report.relationships.artwork_orphaned
    );
    println!(
        "  Playlist entries:   {}",
        report.relationships.playlist_entries
    );
    println!(
        "  References resolved: {}",
        report.relationships.playlist_references_resolved
    );
    println!(
        "  References missing: {}",
        report.relationships.playlist_references_missing
    );
    println!(
        "  References external: {}",
        report.relationships.playlist_references_external
    );
    println!(
        "  Invalid playlists:  {}",
        report.relationships.playlist_files_invalid
    );
    if !report.issues.is_empty() {
        println!("Issues:");
        for issue in &report.issues {
            let kind = match issue.kind {
                music_sync::adoption::AdoptionIssueKind::EmptyMedia => "empty media",
                music_sync::adoption::AdoptionIssueKind::ProbeFailed => "probe failed",
                music_sync::adoption::AdoptionIssueKind::Unreadable => "unreadable",
                music_sync::adoption::AdoptionIssueKind::MissingPlaylistReference => {
                    "missing playlist reference"
                }
                music_sync::adoption::AdoptionIssueKind::ExternalPlaylistReference => {
                    "external playlist reference"
                }
                music_sync::adoption::AdoptionIssueKind::InvalidPlaylist => "invalid playlist",
                music_sync::adoption::AdoptionIssueKind::HashFailed => "hash failed",
            };
            println!("  {kind}: {} ({})", issue.path, issue.message);
        }
    }
    if let Some(probe) = &report.probe {
        println!("Media probe:");
        println!("  Attempted:          {}", probe.attempted);
        println!("  Succeeded:          {}", probe.succeeded);
        println!("  Failed:             {}", probe.failed);
        println!("  Skipped by limit:   {}", probe.skipped_due_to_limit);
        println!("  Duration known:     {}", probe.duration_known);
        println!("  Total duration ms:  {}", probe.total_duration_ms);
        println!("  Embedded artwork:   {}", probe.with_embedded_artwork);
        println!("  Basic tags:         {}", probe.with_basic_tags);
        println!("  Canonical identity: {}", probe.with_canonical_identity);
        println!(
            "  Valid recording MBID: {}",
            probe.with_valid_musicbrainz_recording_id
        );
        println!("  Valid ISRC:         {}", probe.with_valid_isrc);
        println!(
            "  Malformed canonical tags: {}",
            probe.with_malformed_canonical_tag
        );
        println!("  Codecs:");
        for (codec, count) in &probe.codecs {
            println!("    {codec}: {count}");
        }
    }
    if let Some(hash) = &report.hash {
        println!("Artifact hashes:");
        println!("  Attempted:          {}", hash.attempted);
        println!("  Succeeded:          {}", hash.succeeded);
        println!("  Failed:             {}", hash.failed);
        println!("  Skipped by limit:   {}", hash.skipped_due_to_limit);
        println!("  Bytes hashed:       {}", hash.bytes_hashed);
        println!("  Duplicate groups:   {}", hash.exact_duplicate_groups);
        println!("  Duplicate files:    {}", hash.exact_duplicate_files);
    }
    let effects = report.effects();
    println!();
    println!("Files modified:      {}", effects.files_modified);
    println!("Files deleted:       {}", effects.files_deleted);
    println!("Files downloaded:    {}", effects.files_downloaded);
    Ok(())
}

fn render_adoption_apply(
    report: &AdoptionApplyReport,
    json: bool,
) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    render_adoption(&report.scan, false, "music-sync library adopt (applied)")?;
    println!("Database:");
    println!(
        "  Recordings inserted: {}",
        report.database.recordings_inserted
    );
    println!(
        "  Artifacts inserted:  {}",
        report.database.artifacts_inserted
    );
    println!(
        "  Artifacts existing:  {}",
        report.database.artifacts_existing
    );
    Ok(())
}

fn initialize_logging(verbosity: u8) {
    let default_level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

fn render_doctor(report: &DoctorReport, json: bool) -> Result<(), serde_json::Error> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("music-sync doctor");
    for check in &report.checks {
        let marker = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warning => "WARN",
            CheckStatus::Failure => "FAIL",
        };
        println!("{marker:4} {:24} {}", check.name, check.message);
    }
    Ok(())
}
