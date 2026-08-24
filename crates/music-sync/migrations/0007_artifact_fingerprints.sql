CREATE TABLE artifact_fingerprints (
    artifact_id INTEGER PRIMARY KEY REFERENCES artifacts(id),
    algorithm INTEGER NOT NULL CHECK (algorithm = 2),
    max_seconds INTEGER NOT NULL CHECK (max_seconds > 0),
    duration_ms INTEGER NOT NULL CHECK (duration_ms >= 0),
    fingerprint_json TEXT NOT NULL,
    value_count INTEGER NOT NULL CHECK (value_count > 0),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
