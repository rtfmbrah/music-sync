CREATE TABLE lyrics_observations (
    id INTEGER PRIMARY KEY,
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    provider TEXT NOT NULL,
    provider_entity_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('synchronized', 'plain', 'instrumental')),
    content TEXT,
    signature_json TEXT NOT NULL,
    raw_response_json TEXT NOT NULL,
    observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CHECK ((kind = 'instrumental') = (content IS NULL)),
    UNIQUE (recording_id, provider, provider_entity_id, kind)
) STRICT;

CREATE TABLE lyrics_selections (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    observation_id INTEGER NOT NULL REFERENCES lyrics_observations(id),
    selected_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE lyrics_resolutions (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    state TEXT NOT NULL CHECK (state IN ('resolved', 'instrumental', 'unavailable', 'deferred')),
    message TEXT NOT NULL,
    raw_response_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE lyrics_outputs (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    observation_id INTEGER NOT NULL REFERENCES lyrics_observations(id),
    artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
    path TEXT NOT NULL UNIQUE,
    sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
    byte_count INTEGER NOT NULL CHECK (byte_count > 0),
    state TEXT NOT NULL CHECK (state IN ('prepared', 'committed')),
    prepared_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    committed_at TEXT,
    CHECK ((state = 'committed') = (committed_at IS NOT NULL))
) STRICT;
