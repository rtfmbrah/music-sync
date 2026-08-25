CREATE TABLE artifact_fingerprint_deferrals (
    artifact_id INTEGER PRIMARY KEY REFERENCES artifacts(id),
    max_seconds INTEGER NOT NULL CHECK (max_seconds > 0),
    status TEXT NOT NULL DEFAULT 'deferred' CHECK (status IN ('pending','deferred')),
    attempt_count INTEGER NOT NULL DEFAULT 1 CHECK (attempt_count > 0),
    message TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
