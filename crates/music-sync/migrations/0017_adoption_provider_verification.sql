CREATE TABLE adoption_provider_verifications (
    provider_item_id INTEGER PRIMARY KEY REFERENCES provider_items(id),
    acquisition_job_id INTEGER NOT NULL UNIQUE REFERENCES jobs(id),
    artifact_id INTEGER NOT NULL REFERENCES artifacts(id),
    status TEXT NOT NULL CHECK (status IN ('running','deferred','rejected','verified')),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    staged_path TEXT,
    staged_sha256 TEXT,
    staged_duration_ms INTEGER CHECK (staged_duration_ms IS NULL OR staged_duration_ms > 0),
    staged_fingerprint_json TEXT,
    fingerprint_decision TEXT CHECK (fingerprint_decision IS NULL OR fingerprint_decision IN ('match','mismatch','unavailable')),
    message TEXT,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE INDEX adoption_provider_verifications_status
ON adoption_provider_verifications(status, provider_item_id);
