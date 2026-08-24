CREATE TABLE artwork_blobs (
    id INTEGER PRIMARY KEY,
    sha256 TEXT NOT NULL UNIQUE CHECK (length(sha256) = 64),
    relative_path TEXT NOT NULL UNIQUE,
    mime_type TEXT NOT NULL CHECK (mime_type IN ('image/jpeg', 'image/png', 'image/webp')),
    byte_count INTEGER NOT NULL CHECK (byte_count > 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE release_artwork (
    release_id INTEGER PRIMARY KEY REFERENCES releases(id),
    blob_id INTEGER NOT NULL REFERENCES artwork_blobs(id),
    source TEXT NOT NULL,
    source_image_id TEXT NOT NULL,
    source_url TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('front', 'back', 'other')),
    approved INTEGER NOT NULL CHECK (approved IN (0, 1)),
    selected_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (source, source_image_id)
) STRICT;

CREATE TABLE artwork_resolutions (
    release_id INTEGER PRIMARY KEY REFERENCES releases(id),
    state TEXT NOT NULL CHECK (state IN ('resolved', 'unavailable', 'deferred')),
    message TEXT NOT NULL,
    raw_response_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
