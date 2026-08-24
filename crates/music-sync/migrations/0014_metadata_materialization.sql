CREATE TABLE metadata_materializations (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    source_artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
    result_artifact_id INTEGER UNIQUE REFERENCES artifacts(id),
    source_path TEXT NOT NULL,
    source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
    history_path TEXT NOT NULL UNIQUE,
    staged_path TEXT NOT NULL UNIQUE,
    final_path TEXT NOT NULL,
    result_sha256 TEXT NOT NULL CHECK (length(result_sha256) = 64),
    result_bytes INTEGER NOT NULL CHECK (result_bytes > 0),
    codec TEXT NOT NULL,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    sample_rate_hz INTEGER CHECK (sample_rate_hz IS NULL OR sample_rate_hz > 0),
    channels INTEGER CHECK (channels IS NULL OR channels > 0),
    canonical_snapshot_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('prepared', 'committed')),
    message TEXT NOT NULL,
    prepared_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    committed_at TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK ((state = 'committed') = (committed_at IS NOT NULL)),
    CHECK ((state = 'committed') = (result_artifact_id IS NOT NULL))
) STRICT;

CREATE TABLE metadata_materialization_states (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    state TEXT NOT NULL CHECK (state IN ('pending', 'deferred')),
    message TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE metadata_materialization_staging (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    path TEXT NOT NULL UNIQUE,
    reserved_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
