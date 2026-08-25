//! Terminal entry point for music-sync.

#![forbid(unsafe_code)]

use std::error::Error;
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{ArgAction, Parser, Subcommand, ValueEnum};
use music_sync::acquisition::{
    AcquisitionRunOutcome, run_one_acquisition, run_pending_acquisitions,
};
use music_sync::adoption::{
    AdoptionApplyReport, AdoptionReport, HashLimit, ProbeLimit, apply_library, scan_library,
    scan_library_with_hash, scan_library_with_probe, scan_library_with_probe_and_hash,
};
use music_sync::adoption_link::{
    quarantine_unverified_adopted_provider_links, verify_adopted_provider_links,
};
use music_sync::artwork::{CoverArtArchive, resolve_release_artwork};
use music_sync::config::AppConfig;
use music_sync::content_hash::Sha256FileHasher;
use music_sync::diagnostics::{CheckStatus, DoctorReport, run_doctor};
use music_sync::discovery::{DfFreeSpace, run_discovery};
use music_sync::discovery_routing::route_approved_discovery;
use music_sync::fingerprint::{Fpcalc, reconcile_artifact_fingerprints};
use music_sync::health::reconcile_artifact_health;
use music_sync::listenbrainz::ListenBrainz;
use music_sync::lyrics::{Lrclib, resolve_lyrics};
use music_sync::media_probe::Ffprobe;
use music_sync::metadata::resolve_canonical_metadata;
use music_sync::musicbrainz::MusicBrainz;
use music_sync::navidrome::NavidromeFavorites;
use music_sync::persistence::{
    Database, ServiceRunHistoryStatus, ServiceRunTerminalStatus, ServiceRunTrigger,
};
use music_sync::playlist::{PlaylistMaterializationReport, materialize_playlists};
use music_sync::provider::{SourceId, SourceSnapshot, is_supported_youtube_url};
use music_sync::repair::{
    assess_repair_eligibility, commit_verified_repair, generate_repair_candidates,
    run_one_repair_verification,
};
use music_sync::service::{
    ServiceBoundaries, ServiceDirectories, ServiceLimits, run_complete_service,
};
use music_sync::sync::{
    SourceSyncResult, SyncBoundaries, SyncDirectories, SyncLimits, SyncRunReport, run_sync,
};
use music_sync::tag_materialization::{FfmpegMetadataRemuxer, materialize_canonical_tags};
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

    /// Disable real-time phase progress on stderr.
    #[arg(long, global = true)]
    no_progress: bool,

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
    /// Resolve canonical recording, artist, and release metadata.
    Metadata {
        #[command(subcommand)]
        command: MetadataCommand,
    },
    /// Resolve and immutably cache canonical release artwork.
    Artwork {
        #[command(subcommand)]
        command: ArtworkCommand,
    },
    /// Resolve and materialize adjacent synchronized or plain lyrics.
    Lyrics {
        #[command(subcommand)]
        command: LyricsCommand,
    },
    /// Generate explainable recommendations within hard safety budgets.
    Discovery {
        #[command(subcommand)]
        command: DiscoveryCommand,
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
    /// Run the complete bounded autonomous service cycle.
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
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
    /// Inspect or explicitly recover durable service-cycle history.
    Runs {
        #[command(subcommand)]
        command: RunsCommand,
    },
    /// Query bounded persisted operational events without mutation.
    Events {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum newest events (1 through 1000).
        #[arg(long, default_value = "100")]
        limit: NonZeroUsize,
        /// Exact severity filter.
        #[arg(long, value_enum)]
        level: Option<EventLevelFilter>,
        /// Exact component filter.
        #[arg(long)]
        component: Option<String>,
        /// Exact durable run filter.
        #[arg(long)]
        run_id: Option<i64>,
        /// Exact durable job filter.
        #[arg(long)]
        job_id: Option<i64>,
    },
    /// Perform explicit offline-safe operational maintenance.
    Maintenance {
        #[command(subcommand)]
        command: MaintenanceCommand,
    },
}

#[derive(Debug, Subcommand)]
enum MaintenanceCommand {
    /// Create a consistent, no-clobber SQLite snapshot in a provisioned directory.
    Backup {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Existing directory that receives a uniquely named snapshot.
        #[arg(long)]
        directory: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum EventLevelFilter {
    Info,
    Warning,
    Error,
}
impl EventLevelFilter {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Subcommand)]
enum ServiceCommand {
    /// Run every enabled autonomous phase once.
    Run {
        /// TOML configuration containing service policy and directories.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Durable invocation origin.
        #[arg(long, value_enum, default_value = "manual")]
        trigger: SyncTrigger,
    },
}

#[derive(Debug, Subcommand)]
enum RunsCommand {
    /// Show bounded newest-first service-cycle history without mutation.
    History {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum history rows (1 through 1000).
        #[arg(long, default_value = "100")]
        limit: NonZeroUsize,
        /// Optional exact durable status filter.
        #[arg(long)]
        status: Option<RunStatusFilter>,
    },
    /// Mark one confirmed abandoned service cycle interrupted.
    RecoverInterrupted {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Show one service cycle with ordered phase summaries.
    Show {
        /// Durable service run ID.
        run_id: i64,
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum RunStatusFilter {
    Running,
    Succeeded,
    Partial,
    Failed,
    Interrupted,
}

impl From<RunStatusFilter> for ServiceRunHistoryStatus {
    fn from(value: RunStatusFilter) -> Self {
        match value {
            RunStatusFilter::Running => Self::Running,
            RunStatusFilter::Succeeded => Self::Succeeded,
            RunStatusFilter::Partial => Self::Partial,
            RunStatusFilter::Failed => Self::Failed,
            RunStatusFilter::Interrupted => Self::Interrupted,
        }
    }
}

#[derive(Debug, Subcommand)]
enum DiscoveryCommand {
    /// Import optional Navidrome favorites, then run one bounded ListenBrainz pass.
    Run {
        /// TOML configuration identifying state, library, and discovery policy.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Meaningful HTTP User-Agent including operator contact information.
        #[arg(long)]
        user_agent: String,
        /// ListenBrainz endpoint; override only for controlled fixtures/mirrors.
        #[arg(long, default_value = "https://api.listenbrainz.org")]
        listenbrainz_endpoint: String,
        /// Maximum recommendations fetched and considered once.
        #[arg(long, default_value = "100")]
        max_candidates: NonZeroUsize,
        /// HTTP deadline in seconds.
        #[arg(long, default_value = "30")]
        timeout_seconds: NonZeroU64,
        /// `df` executable used for the free-storage guard.
        #[arg(long, default_value = "df")]
        df: PathBuf,
    },
    /// Route approved candidates only through canonical recording URL relationships.
    Route {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Meaningful MusicBrainz User-Agent including operator contact information.
        #[arg(long)]
        user_agent: String,
        /// MusicBrainz endpoint; override only for controlled fixtures/mirrors.
        #[arg(long, default_value = "https://musicbrainz.org/ws/2")]
        endpoint: String,
        /// Maximum approved candidates considered once.
        #[arg(long, default_value = "20")]
        max_candidates: NonZeroUsize,
        /// Per-request HTTP deadline in seconds.
        #[arg(long, default_value = "30")]
        timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum MetadataCommand {
    /// Resolve a bounded set of strong recording identities through MusicBrainz.
    Resolve {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Meaningful MusicBrainz User-Agent including operator contact information.
        #[arg(long)]
        user_agent: String,
        /// MusicBrainz ws/2 endpoint; override only for controlled fixtures/mirrors.
        #[arg(long, default_value = "https://musicbrainz.org/ws/2")]
        endpoint: String,
        /// Maximum recordings attempted once in stable order.
        #[arg(long, default_value = "100")]
        max_recordings: NonZeroUsize,
        /// Per-request HTTP deadline in seconds.
        #[arg(long, default_value = "30")]
        timeout_seconds: NonZeroU64,
    },
    /// Stream-copy selected canonical tags while retaining exact original bytes.
    Materialize {
        /// TOML configuration identifying state and the managed library.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum recordings attempted once in stable order.
        #[arg(long, default_value = "20")]
        max_recordings: NonZeroUsize,
        /// ffmpeg executable path.
        #[arg(long, default_value = "ffmpeg")]
        ffmpeg: PathBuf,
        /// ffprobe executable path.
        #[arg(long, default_value = "ffprobe")]
        ffprobe: PathBuf,
        /// Per-recording remux deadline in seconds.
        #[arg(long, default_value = "120")]
        timeout_seconds: NonZeroU64,
        /// Per-output validation deadline in seconds.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
    },
    /// Explicitly release one deferred materialization for retry.
    RetryMaterialize {
        /// Durable recording ID.
        recording_id: i64,
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ArtworkCommand {
    /// Fetch a bounded set of selected releases through Cover Art Archive.
    Fetch {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Meaningful HTTP User-Agent including operator contact information.
        #[arg(long)]
        user_agent: String,
        /// Cover Art Archive endpoint; override only for controlled fixtures/mirrors.
        #[arg(long, default_value = "https://coverartarchive.org")]
        endpoint: String,
        /// Maximum releases attempted once in stable order.
        #[arg(long, default_value = "100")]
        max_releases: NonZeroUsize,
        /// Per-request HTTP deadline in seconds.
        #[arg(long, default_value = "30")]
        timeout_seconds: NonZeroU64,
    },
}

#[derive(Debug, Subcommand)]
enum LyricsCommand {
    /// Resolve a bounded set of canonical managed recordings through LRCLIB.
    Fetch {
        /// TOML configuration identifying application state and managed library.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Meaningful HTTP User-Agent including operator contact information.
        #[arg(long)]
        user_agent: String,
        /// LRCLIB API endpoint; override only for controlled fixtures/mirrors.
        #[arg(long, default_value = "https://lrclib.net/api")]
        endpoint: String,
        /// Maximum recordings attempted once in stable order.
        #[arg(long, default_value = "100")]
        max_recordings: NonZeroUsize,
        /// Per-request HTTP deadline in seconds.
        #[arg(long, default_value = "30")]
        timeout_seconds: NonZeroU64,
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
    /// Claim, stage, and independently verify at most one generated candidate.
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
        /// fpcalc executable path.
        #[arg(long, default_value = "fpcalc")]
        fpcalc: PathBuf,
        /// Candidate download deadline in seconds.
        #[arg(long, default_value = "600")]
        download_timeout_seconds: NonZeroU64,
        /// Candidate structural probe deadline in seconds.
        #[arg(long, default_value = "30")]
        probe_timeout_seconds: NonZeroU64,
        /// Candidate fingerprint deadline in seconds.
        #[arg(long, default_value = "60")]
        fingerprint_timeout_seconds: NonZeroU64,
        /// Maximum candidate audio seconds fingerprinted.
        #[arg(long, default_value = "120")]
        fingerprint_audio_seconds: NonZeroU64,
    },
    /// Atomically commit one independently verified candidate without overwriting media.
    Commit {
        /// Durable verified repair attempt ID.
        attempt_id: i64,
        /// TOML configuration containing the managed library directory.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Explicitly release one deferred repair attempt for retry.
    Retry {
        /// Durable deferred repair attempt ID.
        attempt_id: i64,
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
    },
    /// Recover abandoned running repair attempts after confirming no process is active.
    RecoverRunning {
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
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
        /// Durable invocation origin for service history.
        #[arg(long, value_enum, default_value = "manual")]
        trigger: SyncTrigger,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SyncTrigger {
    Manual,
    Timer,
}
impl From<SyncTrigger> for ServiceRunTrigger {
    fn from(value: SyncTrigger) -> Self {
        match value {
            SyncTrigger::Manual => Self::Manual,
            SyncTrigger::Timer => Self::Timer,
        }
    }
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
    /// Verify adopted artifacts against queued YouTube objects without replacing audio.
    VerifyProviderLinks {
        /// TOML configuration containing state, tools, and deadlines.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum unique provider objects attempted once.
        #[arg(long, default_value = "100")]
        max_items: NonZeroUsize,
    },
    /// Defer downloads that could duplicate unresolved adopted filename candidates.
    QuarantineUnverifiedProviderLinks {
        /// TOML configuration identifying migration state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
        /// Maximum unique provider objects inspected once.
        #[arg(long, default_value = "100")]
        max_items: NonZeroUsize,
    },
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
    /// Explicitly release one deferred artifact fingerprint for retry.
    RetryFingerprint {
        /// Durable artifact ID shown in the fingerprint failure report.
        artifact_id: i64,
        /// TOML configuration identifying application state.
        #[arg(short, long, default_value = "music-sync.toml")]
        config: PathBuf,
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
    initialize_logging(cli.verbose, cli.no_progress || cli.json);
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
        Command::Maintenance {
            command: MaintenanceCommand::Backup { config, directory },
        } => {
            let config = AppConfig::from_file(&config)?;
            let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let destination = directory.join(format!("music-sync-{timestamp}.sqlite3"));
            let database = Database::open(&config.database_path())?;
            database.backup_to(&destination)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"backup": destination, "created": true})
                );
            } else {
                println!("Database backup created: {}", destination.display());
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Events {
            config,
            limit,
            level,
            component,
            run_id,
            job_id,
        } => {
            if limit.get() > 1000 {
                return Err("event limit must not exceed 1000".into());
            }
            if component
                .as_deref()
                .is_some_and(|value| value.is_empty() || value.len() > 64)
            {
                return Err("event component filter must contain 1 through 64 bytes".into());
            }
            let config = AppConfig::from_file(&config)?;
            let events = Database::operational_events_read_only(
                &config.database_path(),
                limit.get(),
                level.map(EventLevelFilter::as_str),
                component.as_deref(),
                run_id,
                job_id,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&events)?)
            } else {
                for event in events {
                    println!(
                        "{} {} {} {}: {}",
                        event.created_at, event.level, event.component, event.event, event.message
                    )
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Service {
            command: ServiceCommand::Run { config, trigger },
        } => {
            let config = AppConfig::from_file(&config)?;
            if !config.service.enabled {
                return Err("complete service requires service.enabled=true".into());
            }
            let user_agent = config
                .service
                .user_agent
                .as_deref()
                .ok_or("complete service requires a user agent")?;
            let timeout = Duration::from_secs(config.service.http_timeout_seconds);
            let loopback = |endpoint: &str| {
                endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://[::1]:")
            };
            let source = YtDlp::new(
                config.service.yt_dlp.clone(),
                Duration::from_secs(config.service.source_timeout_seconds),
            )
            .with_cookie_file(config.service.yt_dlp_cookie_file.clone())
            .with_cache_directory(config.state_directory.join("yt-dlp-cache"))
            .with_pacing(
                config.service.yt_dlp_sleep_requests_seconds,
                config.service.yt_dlp_min_sleep_seconds,
                config.service.yt_dlp_max_sleep_seconds,
            );
            let acquisition = YtDlp::new(
                config.service.yt_dlp.clone(),
                Duration::from_secs(config.service.download_timeout_seconds),
            )
            .with_cookie_file(config.service.yt_dlp_cookie_file.clone())
            .with_cache_directory(config.state_directory.join("yt-dlp-cache"))
            .with_pacing(
                config.service.yt_dlp_sleep_requests_seconds,
                config.service.yt_dlp_min_sleep_seconds,
                config.service.yt_dlp_max_sleep_seconds,
            );
            let probe = Ffprobe::new(
                config.service.ffprobe.clone(),
                Duration::from_secs(config.service.probe_timeout_seconds),
            );
            let fingerprinter = Fpcalc::new(
                config.service.fpcalc.clone(),
                Duration::from_secs(config.service.fingerprint_timeout_seconds),
                config.service.fingerprint_audio_seconds,
            );
            let musicbrainz = MusicBrainz::with_endpoint(
                &config.service.musicbrainz_endpoint,
                user_agent,
                timeout,
                if loopback(&config.service.musicbrainz_endpoint) {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                },
                !loopback(&config.service.musicbrainz_endpoint),
            )?;
            let artwork = CoverArtArchive::with_endpoint(
                &config.service.cover_art_endpoint,
                user_agent,
                timeout,
                !loopback(&config.service.cover_art_endpoint),
            )?;
            let lyrics = Lrclib::with_endpoint(
                &config.service.lyrics_endpoint,
                user_agent,
                timeout,
                if loopback(&config.service.lyrics_endpoint) {
                    Duration::ZERO
                } else {
                    Duration::from_millis(300)
                },
                !loopback(&config.service.lyrics_endpoint),
            )?;
            let remuxer = FfmpegMetadataRemuxer::new(
                config.service.ffmpeg.clone(),
                Duration::from_secs(config.service.remux_timeout_seconds),
            );
            let recommendations = if config.discovery.enabled {
                Some(ListenBrainz::with_endpoint(
                    &config.service.listenbrainz_endpoint,
                    config
                        .discovery
                        .listenbrainz_user
                        .as_deref()
                        .ok_or("enabled discovery requires ListenBrainz user")?,
                    user_agent,
                    timeout,
                    config.discovery.exploration_ratio,
                    config.discovery.wildcard_ratio,
                    !loopback(&config.service.listenbrainz_endpoint),
                )?)
            } else {
                None
            };
            let taste_signals = if let (Some(url), Some(user)) = (
                config.discovery.navidrome_url.as_deref(),
                config.discovery.navidrome_user.as_deref(),
            ) {
                Some(NavidromeFavorites::new(
                    url,
                    user,
                    &std::env::var("NAVIDROME_TOKEN")?,
                    &std::env::var("NAVIDROME_SALT")?,
                    timeout,
                )?)
            } else {
                None
            };
            let mut database = Database::open(&config.database_path())?;
            let run_id = database.start_service_run(trigger.into(), env!("CARGO_PKG_VERSION"))?;
            let result = run_complete_service(
                &mut database,
                run_id,
                ServiceDirectories {
                    state: &config.state_directory,
                    library: &config.library_directory,
                    playlists: &config.playlist_directory,
                },
                ServiceBoundaries {
                    source_adapter: &source,
                    acquisition_adapter: &acquisition,
                    probe: &probe,
                    hasher: &Sha256FileHasher,
                    fingerprinter: &fingerprinter,
                    canonical_metadata: &musicbrainz,
                    artwork: &artwork,
                    lyrics: &lyrics,
                    remuxer: &remuxer,
                    recommendations: recommendations
                        .as_ref()
                        .map(|value| value as &dyn music_sync::discovery::RecommendationProvider),
                    taste_signals: taste_signals
                        .as_ref()
                        .map(|value| value as &dyn music_sync::navidrome::TasteSignalProvider),
                    free_space: &DfFreeSpace::new("df".into()),
                },
                ServiceLimits {
                    items: config.service.phase_item_limit,
                    fingerprint_audio_seconds: config.service.fingerprint_audio_seconds,
                },
                &config.discovery,
            );
            let report = match result {
                Ok(report) => report,
                Err(error) => {
                    database.finish_service_run(
                        run_id,
                        ServiceRunTerminalStatus::Failed,
                        &serde_json::json!({"error":error.to_string()}),
                    )?;
                    return Err(Box::new(error));
                }
            };
            let successful = report.is_successful();
            database.finish_service_run(
                run_id,
                if successful {
                    ServiceRunTerminalStatus::Succeeded
                } else {
                    ServiceRunTerminalStatus::Partial
                },
                &serde_json::to_value(&report)?,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?)
            } else {
                println!(
                    "Service run {run_id}: {}",
                    if successful { "succeeded" } else { "partial" }
                );
            }
            Ok(if successful {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Runs {
            command:
                RunsCommand::History {
                    config,
                    limit,
                    status,
                },
        } => {
            if limit.get() > 1000 {
                return Err("service run history limit must not exceed 1000".into());
            }
            let config = AppConfig::from_file(&config)?;
            let history = Database::service_run_history_read_only(
                &config.database_path(),
                limit.get(),
                status.map(Into::into),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&history)?);
            } else {
                for entry in history {
                    println!(
                        "{} {} {} {} phases={} failed={} started={} finished={}",
                        entry.id,
                        entry.status,
                        entry.trigger,
                        entry.binary_version,
                        entry.phase_count,
                        entry.failed_phase_count,
                        entry.started_at,
                        entry.finished_at.as_deref().unwrap_or("-")
                    );
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Runs {
            command: RunsCommand::Show { run_id, config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let detail = Database::service_run_detail_read_only(&config.database_path(), run_id)?;
            let Some(detail) = detail else {
                return Err(format!("service run {run_id} does not exist").into());
            };
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&detail)?);
            } else {
                println!(
                    "Run {}: {} ({})",
                    detail.run.id, detail.run.status, detail.run.trigger
                );
                for phase in detail.phases {
                    println!("  {} {}: {}", phase.ordinal, phase.phase, phase.status);
                    if let Some(message) = phase.message {
                        println!("    {message}");
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Runs {
            command: RunsCommand::RecoverInterrupted { config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let recovered = database.recover_interrupted_service_run()?;
            if cli.json {
                println!("{}", serde_json::json!({"recovered_run_id":recovered}));
            } else {
                println!(
                    "Recovered interrupted service run: {}",
                    recovered.map_or_else(|| "none".into(), |id| id.to_string())
                );
            }
            Ok(if recovered.is_some() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Discovery {
            command:
                DiscoveryCommand::Route {
                    config,
                    user_agent,
                    endpoint,
                    max_candidates,
                    timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            if !config.discovery.enabled {
                return Err("discovery routing requires discovery.enabled=true".into());
            }
            let mut database = Database::open(&config.database_path())?;
            let provider = MusicBrainz::with_endpoint(
                &endpoint,
                &user_agent,
                Duration::from_secs(timeout_seconds.get()),
                if endpoint.starts_with("http://127.0.0.1:")
                    || endpoint.starts_with("http://[::1]:")
                {
                    Duration::ZERO
                } else {
                    Duration::from_secs(1)
                },
                !endpoint.starts_with("http://127.0.0.1:")
                    && !endpoint.starts_with("http://[::1]:"),
            )?;
            let report = route_approved_discovery(&mut database, &provider, max_candidates.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Candidates selected: {}", report.selected);
                println!("Acquisitions queued: {}", report.queued);
                println!("Unresolved:          {}", report.unresolved);
                println!("Provider deferred:   {}", report.deferred);
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Discovery {
            command:
                DiscoveryCommand::Run {
                    config,
                    user_agent,
                    listenbrainz_endpoint,
                    max_candidates,
                    timeout_seconds,
                    df,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            if !config.discovery.enabled {
                if cli.json {
                    println!("{}", serde_json::json!({"disabled": true}));
                } else {
                    println!("Discovery is disabled; no provider or database was contacted.");
                }
                return Ok(ExitCode::SUCCESS);
            }
            let mut database = Database::open(&config.database_path())?;
            let mut seed_report = None;
            if let (Some(url), Some(user)) = (
                config.discovery.navidrome_url.as_deref(),
                config.discovery.navidrome_user.as_deref(),
            ) {
                let token = std::env::var("NAVIDROME_TOKEN")?;
                let salt = std::env::var("NAVIDROME_SALT")?;
                let adapter = NavidromeFavorites::new(
                    url,
                    user,
                    &token,
                    &salt,
                    Duration::from_secs(timeout_seconds.get()),
                )?;
                seed_report = Some(database.replace_navidrome_seeds(&adapter.recording_mbids()?)?);
            }
            let listenbrainz_user = config
                .discovery
                .listenbrainz_user
                .as_deref()
                .ok_or("enabled discovery requires a ListenBrainz user")?;
            let provider = ListenBrainz::with_endpoint(
                &listenbrainz_endpoint,
                listenbrainz_user,
                &user_agent,
                Duration::from_secs(timeout_seconds.get()),
                config.discovery.exploration_ratio,
                config.discovery.wildcard_ratio,
                !listenbrainz_endpoint.starts_with("http://127.0.0.1:")
                    && !listenbrainz_endpoint.starts_with("http://[::1]:"),
            )?;
            let report = run_discovery(
                &mut database,
                &provider,
                &DfFreeSpace::new(df),
                &config.library_directory,
                &config.discovery,
                max_candidates.get(),
            )?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"seeds": seed_report, "discovery": report})
                    )?
                );
            } else {
                if let Some(seeds) = seed_report {
                    println!("Navidrome seeds matched:   {}", seeds.matched);
                    println!("Navidrome seeds unmatched: {}", seeds.unmatched);
                }
                println!("Recommendations received: {}", report.received);
                println!("Candidates approved:      {}", report.persistence.approved);
                println!(
                    "Canonical duplicates:     {}",
                    report.persistence.duplicates
                );
                println!(
                    "Budget rejected:          {}",
                    report.persistence.budget_rejected
                );
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Metadata {
            command:
                MetadataCommand::RetryMaterialize {
                    recording_id,
                    config,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let released = database.retry_metadata_materialization(recording_id)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"recording_id": recording_id, "released": released})
                );
            } else {
                println!("Materialization retry released: {released}");
            }
            Ok(if released {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Metadata {
            command:
                MetadataCommand::Materialize {
                    config,
                    max_recordings,
                    ffmpeg,
                    ffprobe,
                    timeout_seconds,
                    probe_timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let remuxer =
                FfmpegMetadataRemuxer::new(ffmpeg, Duration::from_secs(timeout_seconds.get()));
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let report = materialize_canonical_tags(
                &mut database,
                &remuxer,
                &probe,
                &Sha256FileHasher,
                &config.state_directory,
                max_recordings.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Recordings selected: {}", report.selected);
                println!("Tags committed:      {}", report.committed);
                println!("Commits recovered:   {}", report.recovered);
                println!("Tags deferred:       {}", report.deferred);
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Lyrics {
            command:
                LyricsCommand::Fetch {
                    config,
                    user_agent,
                    endpoint,
                    max_recordings,
                    timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let https_only = endpoint.starts_with("https://");
            if !https_only
                && !endpoint.starts_with("http://127.0.0.1:")
                && !endpoint.starts_with("http://[::1]:")
            {
                return Err("lyrics endpoint must use HTTPS or loopback HTTP".into());
            }
            let provider = Lrclib::with_endpoint(
                &endpoint,
                &user_agent,
                Duration::from_secs(timeout_seconds.get()),
                if https_only {
                    Duration::from_millis(300)
                } else {
                    Duration::ZERO
                },
                https_only,
            )?;
            let report = resolve_lyrics(&mut database, &provider, max_recordings.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Recordings selected: {}", report.selected);
                println!("Lyrics resolved:     {}", report.resolved);
                println!("Instrumental:        {}", report.instrumental);
                println!("Lyrics unavailable:  {}", report.unavailable);
                println!("Lyrics deferred:     {}", report.deferred);
                println!("Sidecars committed:  {}", report.sidecars_committed);
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Artwork {
            command:
                ArtworkCommand::Fetch {
                    config,
                    user_agent,
                    endpoint,
                    max_releases,
                    timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let https_only = endpoint.starts_with("https://");
            if !https_only
                && !endpoint.starts_with("http://127.0.0.1:")
                && !endpoint.starts_with("http://[::1]:")
            {
                return Err("artwork endpoint must use HTTPS or loopback HTTP".into());
            }
            let provider = CoverArtArchive::with_endpoint(
                &endpoint,
                &user_agent,
                Duration::from_secs(timeout_seconds.get()),
                https_only,
            )?;
            let report = resolve_release_artwork(
                &mut database,
                &provider,
                &config.state_directory,
                max_releases.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Releases selected:   {}", report.selected);
                println!("Artwork resolved:    {}", report.resolved);
                println!("Artwork unavailable: {}", report.unavailable);
                println!("Artwork deferred:    {}", report.deferred);
                println!("Provider failures:   {}", report.failures.len());
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Metadata {
            command:
                MetadataCommand::Resolve {
                    config,
                    user_agent,
                    endpoint,
                    max_recordings,
                    timeout_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let https_only = endpoint.starts_with("https://");
            if !https_only
                && !endpoint.starts_with("http://127.0.0.1:")
                && !endpoint.starts_with("http://[::1]:")
            {
                return Err("metadata endpoint must use HTTPS or loopback HTTP".into());
            }
            let provider = MusicBrainz::with_endpoint(
                &endpoint,
                &user_agent,
                Duration::from_secs(timeout_seconds.get()),
                if https_only {
                    Duration::from_secs(1)
                } else {
                    Duration::ZERO
                },
                https_only,
            )?;
            let report =
                resolve_canonical_metadata(&mut database, &provider, max_recordings.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Recordings selected:  {}", report.selected);
                println!("Recordings resolved:  {}", report.resolved);
                println!("Recordings ambiguous: {}", report.ambiguous);
                println!("Recordings deferred:  {}", report.deferred);
                println!("Provider failures:    {}", report.failures.len());
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Repair {
            command: RepairCommand::Retry { attempt_id, config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            if !database.retry_deferred_repair_attempt(attempt_id)? {
                return Err(format!("repair attempt {attempt_id} is not deferred").into());
            }
            println!("Repair attempt {attempt_id} released for retry.");
            Ok(ExitCode::SUCCESS)
        }
        Command::Repair {
            command: RepairCommand::RecoverRunning { config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let recovered = database.recover_running_repair_attempts()?;
            if cli.json {
                println!("{}", serde_json::json!({ "recovered": recovered }));
            } else {
                println!("Repair attempts recovered: {recovered}");
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Repair {
            command: RepairCommand::Commit { attempt_id, config },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let report = commit_verified_repair(
                &mut database,
                attempt_id,
                &config.library_directory,
                &Sha256FileHasher,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Repair attempt committed: {}", report.attempt_id);
                println!("Artifact path: {}", report.final_path.display());
                println!("Artifact ID: {}", report.persistence.artifact_id);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Repair {
            command:
                RepairCommand::RunOne {
                    config,
                    yt_dlp,
                    ffprobe,
                    fpcalc,
                    download_timeout_seconds,
                    probe_timeout_seconds,
                    fingerprint_timeout_seconds,
                    fingerprint_audio_seconds,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let fingerprint_audio_seconds = u32::try_from(fingerprint_audio_seconds.get())
                .map_err(|_| "maximum fingerprint audio seconds exceeds u32")?;
            let downloader =
                YtDlp::new(yt_dlp, Duration::from_secs(download_timeout_seconds.get()));
            let probe = Ffprobe::new(ffprobe, Duration::from_secs(probe_timeout_seconds.get()));
            let fingerprinter = Fpcalc::new(
                fpcalc,
                Duration::from_secs(fingerprint_timeout_seconds.get()),
                fingerprint_audio_seconds,
            );
            let outcome = run_one_repair_verification(
                &mut database,
                &config.state_directory,
                &downloader,
                &probe,
                &Sha256FileHasher,
                &fingerprinter,
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else {
                println!("Repair verification: {outcome:?}");
            }
            Ok(match outcome {
                music_sync::repair::RepairRunOutcome::Deferred { .. } => ExitCode::from(1),
                _ => ExitCode::SUCCESS,
            })
        }
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
                println!("Fingerprints deferred:   {}", report.fingerprints_deferred);
                println!("Repairs eligible:        {}", report.repairs.eligible);
                println!("Repairs unresolved:      {}", report.repairs.unresolved);
                println!("Repairs verified:        {}", report.repairs.verified);
                println!("Repairs cancelled:       {}", report.repairs.cancelled);
                println!(
                    "Repair attempts queued:  {}",
                    report.repair_attempts.generated
                );
                println!(
                    "Repair attempts running: {}",
                    report.repair_attempts.running
                );
                println!(
                    "Repair attempts deferred:{}",
                    report.repair_attempts.deferred
                );
                println!(
                    "Repair attempts verified:{}",
                    report.repair_attempts.verified
                );
                println!(
                    "Repair attempts committed:{}",
                    report.repair_attempts.committed
                );
                println!("Metadata resolved:       {}", report.metadata.resolved);
                println!("Metadata ambiguous:      {}", report.metadata.ambiguous);
                println!("Metadata deferred:       {}", report.metadata.deferred);
                println!(
                    "Metadata fields selected:{}",
                    report.metadata.selected_fields
                );
                println!("Artwork resolved:        {}", report.artwork.resolved);
                println!("Artwork unavailable:     {}", report.artwork.unavailable);
                println!("Artwork deferred:        {}", report.artwork.deferred);
                println!("Artwork cached blobs:    {}", report.artwork.cached_blobs);
                println!("Lyrics resolved:         {}", report.lyrics.resolved);
                println!("Lyrics instrumental:     {}", report.lyrics.instrumental);
                println!("Lyrics unavailable:      {}", report.lyrics.unavailable);
                println!("Lyrics deferred:         {}", report.lyrics.deferred);
                println!(
                    "Lyrics sidecars:         {}",
                    report.lyrics.committed_outputs
                );
                println!(
                    "Tag outputs prepared:    {}",
                    report.metadata_materializations.prepared
                );
                println!(
                    "Tag outputs committed:   {}",
                    report.metadata_materializations.committed
                );
                println!(
                    "Tag outputs deferred:    {}",
                    report.metadata_materializations.deferred
                );
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
            command: LibraryCommand::VerifyProviderLinks { config, max_items },
        } => {
            let config = AppConfig::from_file(&config)?;
            if !config.service.enabled {
                return Err("provider-link verification requires service.enabled=true".into());
            }
            let mut database = Database::open(&config.database_path())?;
            let downloader = YtDlp::new(
                config.service.yt_dlp.clone(),
                Duration::from_secs(config.service.download_timeout_seconds),
            )
            .with_cookie_file(config.service.yt_dlp_cookie_file.clone())
            .with_cache_directory(config.state_directory.join("yt-dlp-cache"))
            .with_pacing(
                config.service.yt_dlp_sleep_requests_seconds,
                config.service.yt_dlp_min_sleep_seconds,
                config.service.yt_dlp_max_sleep_seconds,
            );
            let probe = Ffprobe::new(
                config.service.ffprobe.clone(),
                Duration::from_secs(config.service.probe_timeout_seconds),
            );
            let fingerprinter = Fpcalc::new(
                config.service.fpcalc.clone(),
                Duration::from_secs(config.service.fingerprint_timeout_seconds),
                config.service.fingerprint_audio_seconds,
            );
            let report = verify_adopted_provider_links(
                &mut database,
                &config.state_directory,
                &downloader,
                &probe,
                &Sha256FileHasher,
                &fingerprinter,
                max_items.get(),
            )?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Provider objects selected: {}", report.selected);
                println!("Verified links:           {}", report.verified);
                println!("Rejected candidates:      {}", report.rejected);
                println!("Deferred candidates:      {}", report.deferred);
                println!("Ambiguous filenames:      {}", report.ambiguous);
                println!("Boundary failures:        {}", report.failures.len());
            }
            Ok(if report.failures.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Library {
            command: LibraryCommand::QuarantineUnverifiedProviderLinks { config, max_items },
        } => {
            let config = AppConfig::from_file(&config)?;
            if !config.service.enabled {
                return Err("provider-link quarantine requires service.enabled=true".into());
            }
            let mut database = Database::open(&config.database_path())?;
            let report =
                quarantine_unverified_adopted_provider_links(&mut database, max_items.get())?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Provider objects selected: {}", report.selected);
                println!("Newly quarantined:        {}", report.quarantined);
                println!("Already quarantined:      {}", report.already_quarantined);
                println!("Verifications running:    {}", report.running);
                println!("Rejected candidates:      {}", report.rejected);
                println!("Changed during run:       {}", report.changed_during_run);
            }
            Ok(if report.running == 0 && report.changed_during_run == 0 {
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
                LibraryCommand::RetryFingerprint {
                    artifact_id,
                    config,
                },
        } => {
            let config = AppConfig::from_file(&config)?;
            let mut database = Database::open(&config.database_path())?;
            let released = database.retry_deferred_artifact_fingerprint(artifact_id)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({"artifact_id": artifact_id, "released": released})
                );
            } else if released {
                println!("Released artifact fingerprint {artifact_id} for retry");
            } else {
                println!("Artifact fingerprint {artifact_id} was not deferred");
            }
            Ok(if released {
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
                    trigger,
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
            let service_run_id =
                database.start_service_run(trigger.into(), env!("CARGO_PKG_VERSION"))?;
            let report = match run_sync(
                &mut database,
                service_run_id,
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
            ) {
                Ok(report) => report,
                Err(error) => {
                    database.finish_service_run(
                        service_run_id,
                        ServiceRunTerminalStatus::Failed,
                        &serde_json::json!({"error":error.to_string()}),
                    )?;
                    return Err(Box::new(error));
                }
            };
            let successful = report.is_successful();
            let report_json = serde_json::to_value(&report)?;
            database.finish_service_run(
                service_run_id,
                if successful {
                    ServiceRunTerminalStatus::Succeeded
                } else {
                    ServiceRunTerminalStatus::Partial
                },
                &serde_json::json!({"core_sync":report_json}),
            )?;
            render_sync_run(&report, cli.json)?;
            Ok(if successful {
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

fn initialize_logging(verbosity: u8, progress_disabled: bool) {
    let default_level = match verbosity {
        0 if progress_disabled => "warn",
        0 => "info",
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
