CREATE TABLE discovery_seeds (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    origin TEXT NOT NULL CHECK (origin IN ('library', 'navidrome_favorite', 'manual')),
    weight_millionths INTEGER NOT NULL CHECK (weight_millionths BETWEEN 1 AND 1000000),
    active INTEGER NOT NULL CHECK (active IN (0,1)),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE discovery_runs (
    id INTEGER PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('running','succeeded','partial','failed')),
    maximum_tracks INTEGER NOT NULL CHECK (maximum_tracks >= 0),
    per_artist_maximum INTEGER NOT NULL CHECK (per_artist_maximum >= 0),
    exploration_maximum INTEGER NOT NULL CHECK (exploration_maximum >= 0),
    wildcard_maximum INTEGER NOT NULL CHECK (wildcard_maximum >= 0),
    free_bytes INTEGER NOT NULL CHECK (free_bytes >= 0),
    required_free_bytes INTEGER NOT NULL CHECK (required_free_bytes >= 0),
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT
) STRICT;

CREATE TABLE discovery_candidates (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES discovery_runs(id),
    musicbrainz_recording_id TEXT NOT NULL CHECK (length(musicbrainz_recording_id)=36),
    musicbrainz_artist_id TEXT CHECK (musicbrainz_artist_id IS NULL OR length(musicbrainz_artist_id)=36),
    lane TEXT NOT NULL CHECK (lane IN ('adjacent','exploration','wildcard')),
    provider TEXT NOT NULL,
    provider_score_millionths INTEGER NOT NULL CHECK (provider_score_millionths BETWEEN 0 AND 1000000),
    taste_score_millionths INTEGER NOT NULL CHECK (taste_score_millionths BETWEEN 0 AND 1000000),
    explanation_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('proposed','duplicate','budget_rejected','approved','queued','unresolved','acquired')),
    decision_reason TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (run_id,musicbrainz_recording_id)
) STRICT;

CREATE INDEX discovery_candidates_daily_state
ON discovery_candidates(created_at,state,musicbrainz_artist_id,lane);

CREATE TABLE discovery_acquisition_assertions (
    candidate_id INTEGER PRIMARY KEY REFERENCES discovery_candidates(id),
    provider_item_id INTEGER NOT NULL UNIQUE REFERENCES provider_items(id),
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    relationship_source TEXT NOT NULL,
    canonical_duration_ms INTEGER CHECK (canonical_duration_ms IS NULL OR canonical_duration_ms >= 0),
    canonical_isrc TEXT,
    assertion_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
