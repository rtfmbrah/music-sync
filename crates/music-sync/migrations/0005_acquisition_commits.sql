CREATE TABLE acquisition_commits (
    job_id INTEGER PRIMARY KEY REFERENCES acquisition_jobs(job_id),
    staged_path TEXT NOT NULL,
    final_path TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL,
    bytes INTEGER NOT NULL CHECK (bytes > 0),
    codec TEXT NOT NULL,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    sample_rate_hz INTEGER CHECK (sample_rate_hz IS NULL OR sample_rate_hz > 0),
    channels INTEGER CHECK (channels IS NULL OR channels > 0),
    status TEXT NOT NULL CHECK (status IN ('prepared', 'committed')),
    prepared_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    committed_at TEXT
) STRICT;
