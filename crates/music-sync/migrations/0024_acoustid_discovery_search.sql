CREATE TABLE discovery_search_candidates (
    id INTEGER PRIMARY KEY,
    discovery_candidate_id INTEGER NOT NULL REFERENCES discovery_candidates(id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    provider TEXT NOT NULL CHECK (provider = 'youtube'),
    provider_item_id TEXT NOT NULL,
    provider_url TEXT NOT NULL,
    provider_title TEXT,
    provider_duration_ms INTEGER CHECK (provider_duration_ms IS NULL OR provider_duration_ms >= 0),
    provider_metadata_json TEXT NOT NULL,
    search_query TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'generated','rejected','staged','verified','unresolved','deferred','queued','acquired'
    )),
    message TEXT NOT NULL,
    staged_path TEXT,
    staged_sha256 TEXT,
    staged_bytes INTEGER CHECK (staged_bytes IS NULL OR staged_bytes > 0),
    fingerprint_duration_seconds INTEGER
        CHECK (fingerprint_duration_seconds IS NULL OR fingerprint_duration_seconds > 0),
    acoustid_decision TEXT CHECK (acoustid_decision IS NULL OR acoustid_decision IN (
        'verified','contradicted','insufficient'
    )),
    acoustid_score_millionths INTEGER
        CHECK (acoustid_score_millionths IS NULL OR acoustid_score_millionths BETWEEN 0 AND 1000000),
    acoustid_response_json TEXT,
    acquisition_job_id INTEGER UNIQUE REFERENCES jobs(id),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (discovery_candidate_id, provider, provider_item_id)
) STRICT;

CREATE INDEX discovery_search_candidates_work
ON discovery_search_candidates(discovery_candidate_id,state,id);
