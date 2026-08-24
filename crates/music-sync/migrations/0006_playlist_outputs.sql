ALTER TABLE recordings
ADD COLUMN preferred_artifact_id INTEGER REFERENCES artifacts(id);

UPDATE recordings
SET preferred_artifact_id = (
    SELECT MIN(artifacts.id)
    FROM artifacts
    WHERE artifacts.recording_id = recordings.id
      AND artifacts.health = 'healthy'
)
WHERE 1 = (
    SELECT COUNT(*)
    FROM artifacts
    WHERE artifacts.recording_id = recordings.id
      AND artifacts.health = 'healthy'
);

CREATE TABLE playlist_outputs (
    collection_id INTEGER PRIMARY KEY REFERENCES collections(id),
    path TEXT NOT NULL UNIQUE,
    sha256 TEXT,
    updated_at TEXT
) STRICT;
