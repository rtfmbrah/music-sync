CREATE TABLE repair_cases (
    id INTEGER PRIMARY KEY,
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    original_provider_item_id INTEGER NOT NULL UNIQUE REFERENCES provider_items(id),
    reference_artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
    state TEXT NOT NULL DEFAULT 'eligible'
        CHECK (state IN ('eligible', 'unresolved', 'verified', 'cancelled')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE repair_attempts (
    id INTEGER PRIMARY KEY,
    repair_case_id INTEGER NOT NULL REFERENCES repair_cases(id),
    candidate_provider TEXT NOT NULL,
    candidate_provider_item_id TEXT NOT NULL,
    candidate_url TEXT NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN ('generated', 'rejected', 'unresolved', 'verified', 'committed')),
    reason TEXT NOT NULL,
    candidate_metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (repair_case_id, candidate_provider, candidate_provider_item_id)
) STRICT;
