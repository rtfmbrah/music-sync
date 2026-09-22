//! Service file-log initialization and startup rotation.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use time::macros::format_description;
use time::{Duration, OffsetDateTime};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const ARCHIVE_FORMAT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]-[hour]-[minute]-[second]");

/// Initializes terminal logging and optional persistent service file logging.
pub fn initialize(
    verbosity: u8,
    progress_disabled: bool,
    log_directory: Option<&Path>,
) -> Result<LoggingGuard, Box<dyn std::error::Error>> {
    let terminal_level = match verbosity {
        0 if progress_disabled => "warn",
        0 | 1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let file_level = match verbosity {
        0 | 1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let terminal = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_filter(environment_filter(terminal_level));
    let (file, rotation, lock) = log_directory.map(prepare_current_log).transpose()?.map_or(
        (None, None, None),
        |(writer, rotation, lock)| {
            let make_writer = move || writer.clone();
            let layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(make_writer)
                .with_filter(EnvFilter::new(file_level));
            (Some(layer), Some(rotation), Some(lock))
        },
    );
    tracing_subscriber::registry()
        .with(terminal)
        .with(file)
        .try_init()?;
    if let Some(rotation) = rotation {
        tracing::info!(
            current_log = %rotation.current.display(),
            archived_log = rotation
                .archived
                .as_ref()
                .map_or("", |path| path.to_str().unwrap_or("[non-Unicode path]")),
            "persistent service logging initialized"
        );
    }
    Ok(LoggingGuard { _lock: lock })
}

fn environment_filter(default_level: &str) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level))
}

#[derive(Debug)]
struct LogRotation {
    current: PathBuf,
    archived: Option<PathBuf>,
}

/// Keeps the service-log ownership lock alive for the complete command.
pub struct LoggingGuard {
    _lock: Option<File>,
}

fn prepare_current_log(directory: &Path) -> io::Result<(SharedFile, LogRotation, File)> {
    fs::create_dir_all(directory)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".service-log.lock"))?;
    lock.try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => io::Error::new(
            io::ErrorKind::WouldBlock,
            "another music-sync service invocation owns current.log",
        ),
        TryLockError::Error(error) => error,
    })?;
    let current = directory.join("current.log");
    let archived = rotate_nonempty_log(&current, directory, OffsetDateTime::now_utc())?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&current)?;
    Ok((
        SharedFile(Arc::new(Mutex::new(file))),
        LogRotation { current, archived },
        lock,
    ))
}

fn rotate_nonempty_log(
    current: &Path,
    directory: &Path,
    timestamp: OffsetDateTime,
) -> io::Result<Option<PathBuf>> {
    match fs::metadata(current) {
        Ok(metadata) if metadata.len() == 0 => return Ok(None),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    for offset in 0..=60_i64 {
        let stamp = (timestamp + Duration::seconds(offset))
            .format(ARCHIVE_FORMAT)
            .map_err(io::Error::other)?;
        let archived = directory.join(format!("{stamp}-music-sync.log"));
        match fs::hard_link(current, &archived) {
            Ok(()) => {
                fs::remove_file(current)?;
                return Ok(Some(archived));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not reserve a unique timestamped music-sync log name",
    ))
}

#[derive(Clone)]
struct SharedFile(Arc<Mutex<File>>);

impl Write for SharedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("service log lock was poisoned"))?
            .write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("service log lock was poisoned"))?
            .flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_nonempty_current_log_without_overwrite() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let current = root.path().join("current.log");
        fs::write(&current, b"first run\n")?;
        let instant = OffsetDateTime::from_unix_timestamp(1_788_883_323)?;

        let first = rotate_nonempty_log(&current, root.path(), instant)?
            .ok_or("non-empty current log was not rotated")?;
        assert_eq!(
            first.file_name().and_then(|name| name.to_str()),
            Some("2026-09-08-16-02-03-music-sync.log")
        );
        assert_eq!(fs::read(&first)?, b"first run\n");

        fs::write(&current, b"second run\n")?;
        let second = rotate_nonempty_log(&current, root.path(), instant)?
            .ok_or("second current log was not rotated")?;
        assert_eq!(
            second.file_name().and_then(|name| name.to_str()),
            Some("2026-09-08-16-02-04-music-sync.log")
        );
        assert_eq!(fs::read(&first)?, b"first run\n");
        assert_eq!(fs::read(&second)?, b"second run\n");
        Ok(())
    }

    #[test]
    fn leaves_empty_current_log_unarchived() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let current = root.path().join("current.log");
        File::create(&current)?;

        assert!(rotate_nonempty_log(&current, root.path(), OffsetDateTime::UNIX_EPOCH)?.is_none());
        assert!(current.exists());
        Ok(())
    }

    #[test]
    fn concurrent_owner_cannot_rotate_active_log() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let (mut writer, _, _guard) = prepare_current_log(root.path())?;
        writer.write_all(b"active run\n")?;
        writer.flush()?;

        let error = prepare_current_log(root.path())
            .err()
            .ok_or("concurrent log owner unexpectedly succeeded")?;

        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(fs::read(root.path().join("current.log"))?, b"active run\n");
        assert_eq!(
            fs::read_dir(root.path())?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with("-music-sync.log"))
                })
                .count(),
            0
        );
        Ok(())
    }
}
