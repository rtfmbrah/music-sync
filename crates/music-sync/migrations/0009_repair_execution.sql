ALTER TABLE repair_attempts RENAME TO repair_attempts_v8;

CREATE TABLE repair_attempts (
    id INTEGER PRIMARY KEY,
    repair_case_id INTEGER NOT NULL REFERENCES repair_cases(id),
    candidate_provider TEXT NOT NULL,
    candidate_provider_item_id TEXT NOT NULL,
    candidate_url TEXT NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN ('generated', 'running', 'deferred', 'rejected',
                         'unresolved', 'verified', 'committed')),
    reason TEXT NOT NULL,
    candidate_metadata_json TEXT NOT NULL DEFAULT '{}',
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (repair_case_id, candidate_provider, candidate_provider_item_id)
) STRICT;

INSERT INTO repair_attempts(
    id, repair_case_id, candidate_provider, candidate_provider_item_id,
    candidate_url, state, reason, candidate_metadata_json, created_at, updated_at)
SELECT id, repair_case_id, candidate_provider, candidate_provider_item_id,
       candidate_url, state, reason, candidate_metadata_json, created_at, updated_at
FROM repair_attempts_v8;

DROP TABLE repair_attempts_v8;

CREATE TABLE repair_attempt_evidence (
    repair_attempt_id INTEGER PRIMARY KEY REFERENCES repair_attempts(id),
    staged_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    byte_count INTEGER NOT NULL CHECK (byte_count > 0),
    codec TEXT NOT NULL,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    sample_rate_hz INTEGER CHECK (sample_rate_hz IS NULL OR sample_rate_hz > 0),
    channels INTEGER CHECK (channels IS NULL OR channels > 0),
    musicbrainz_recording_id TEXT,
    isrc TEXT,
    fingerprint_max_seconds INTEGER NOT NULL CHECK (fingerprint_max_seconds > 0),
    fingerprint_duration_ms INTEGER NOT NULL CHECK (fingerprint_duration_ms >= 0),
    fingerprint_json TEXT NOT NULL,
    decision TEXT NOT NULL CHECK (decision IN ('rejected', 'unresolved', 'verified')),
    decision_reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE repair_commits (
    repair_attempt_id INTEGER PRIMARY KEY REFERENCES repair_attempts(id),
    final_path TEXT NOT NULL UNIQUE,
    artifact_id INTEGER UNIQUE REFERENCES artifacts(id),
    prepared_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    committed_at TEXT,
    CHECK ((artifact_id IS NULL) = (committed_at IS NULL))
) STRICT;
