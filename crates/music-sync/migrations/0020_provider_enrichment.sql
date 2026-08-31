CREATE TABLE provider_item_enrichments (
    provider_item_id INTEGER PRIMARY KEY REFERENCES provider_items(id),
    display_title TEXT NOT NULL,
    display_artist TEXT,
    artist_provenance TEXT CHECK (artist_provenance IS NULL OR artist_provenance IN (
        'artist', 'creator', 'channel', 'uploader')),
    album TEXT,
    release_date TEXT,
    genres_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(genres_json)),
    thumbnail_url TEXT,
    completeness TEXT NOT NULL CHECK (completeness IN ('snapshot', 'full')),
    raw_metadata_json TEXT NOT NULL CHECK (json_valid(raw_metadata_json)),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE recording_provider_artwork (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    provider_item_id INTEGER NOT NULL REFERENCES provider_items(id),
    blob_id INTEGER NOT NULL REFERENCES artwork_blobs(id),
    source_url TEXT NOT NULL,
    selected_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE provider_artwork_resolutions (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    provider_item_id INTEGER NOT NULL REFERENCES provider_items(id),
    state TEXT NOT NULL CHECK (state IN ('resolved', 'unavailable', 'deferred')),
    message TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE INDEX provider_item_enrichments_completeness
ON provider_item_enrichments(completeness, provider_item_id);
