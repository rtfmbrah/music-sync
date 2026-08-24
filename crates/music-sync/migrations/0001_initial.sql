CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE recordings (
    id INTEGER PRIMARY KEY,
    musicbrainz_recording_id TEXT UNIQUE,
    isrc TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE provider_items (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    provider_item_id TEXT NOT NULL,
    original_url TEXT NOT NULL,
    source_title TEXT,
    source_metadata_json TEXT NOT NULL DEFAULT '{}',
    availability TEXT NOT NULL DEFAULT 'unknown'
        CHECK (availability IN ('unknown', 'available', 'transient_failure', 'permanently_unavailable')),
    recording_id INTEGER REFERENCES recordings(id),
    UNIQUE (provider, provider_item_id)
) STRICT;

CREATE TABLE artifacts (
    id INTEGER PRIMARY KEY,
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    path TEXT NOT NULL UNIQUE,
    sha256 TEXT,
    chromaprint TEXT,
    duration_ms INTEGER CHECK (duration_ms IS NULL OR duration_ms >= 0),
    codec TEXT,
    sample_rate_hz INTEGER,
    channels INTEGER,
    health TEXT NOT NULL DEFAULT 'unknown'
        CHECK (health IN ('unknown', 'healthy', 'missing', 'corrupt')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE collections (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    provider_collection_id TEXT NOT NULL,
    name TEXT NOT NULL,
    UNIQUE (provider, provider_collection_id)
) STRICT;

CREATE TABLE collection_memberships (
    collection_id INTEGER NOT NULL REFERENCES collections(id),
    provider_item_id INTEGER NOT NULL REFERENCES provider_items(id),
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    position INTEGER,
    first_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_seen_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (collection_id, provider_item_id)
) STRICT;

CREATE TABLE sync_runs (
    id INTEGER PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed', 'interrupted')),
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
) STRICT;

CREATE TABLE jobs (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES sync_runs(id),
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'deferred')),
    idempotency_key TEXT NOT NULL UNIQUE,
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    run_id INTEGER REFERENCES sync_runs(id),
    job_id INTEGER REFERENCES jobs(id),
    level TEXT NOT NULL,
    component TEXT NOT NULL,
    event TEXT NOT NULL,
    message TEXT NOT NULL,
    context_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

