CREATE TABLE genre_resolutions (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    state TEXT NOT NULL CHECK (state IN ('resolved', 'unavailable', 'ambiguous', 'deferred')),
    source_entity_id TEXT,
    genres_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(genres_json)),
    message TEXT NOT NULL,
    raw_response_json TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(raw_response_json)),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE recording_genres (
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    genre TEXT NOT NULL,
    source TEXT NOT NULL,
    source_entity_id TEXT NOT NULL,
    confidence_millionths INTEGER NOT NULL CHECK (
        confidence_millionths BETWEEN 0 AND 1000000),
    scope TEXT NOT NULL CHECK (scope IN ('recording', 'release-group', 'artist')),
    selected_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (recording_id, genre, source)
) STRICT;

CREATE INDEX genre_resolutions_state ON genre_resolutions(state, recording_id);
