CREATE TABLE artists (
    id INTEGER PRIMARY KEY,
    musicbrainz_artist_id TEXT NOT NULL UNIQUE,
    canonical_name TEXT NOT NULL,
    sort_name TEXT,
    disambiguation TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE releases (
    id INTEGER PRIMARY KEY,
    musicbrainz_release_id TEXT NOT NULL UNIQUE,
    release_group_id TEXT,
    canonical_title TEXT NOT NULL,
    release_date TEXT,
    country TEXT,
    status TEXT,
    primary_type TEXT,
    secondary_types_json TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE recording_artist_credits (
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    position INTEGER NOT NULL CHECK (position >= 0),
    artist_id INTEGER NOT NULL REFERENCES artists(id),
    credited_name TEXT NOT NULL,
    join_phrase TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (recording_id, position)
) STRICT;

CREATE TABLE recording_releases (
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    release_id INTEGER NOT NULL REFERENCES releases(id),
    medium_position INTEGER CHECK (medium_position IS NULL OR medium_position > 0),
    track_position INTEGER CHECK (track_position IS NULL OR track_position > 0),
    track_title TEXT,
    PRIMARY KEY (recording_id, release_id)
) STRICT;

CREATE TABLE metadata_observations (
    id INTEGER PRIMARY KEY,
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    field TEXT NOT NULL CHECK (field IN (
        'title', 'artist_credit', 'release', 'release_date',
        'track_number', 'disc_number')),
    value TEXT NOT NULL,
    source TEXT NOT NULL,
    source_entity_id TEXT NOT NULL,
    confidence_millionths INTEGER NOT NULL
        CHECK (confidence_millionths BETWEEN 0 AND 1000000),
    resolution_context TEXT NOT NULL,
    observed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (recording_id, field, source, source_entity_id, value)
) STRICT;

CREATE TABLE metadata_selections (
    recording_id INTEGER NOT NULL REFERENCES recordings(id),
    field TEXT NOT NULL CHECK (field IN (
        'title', 'artist_credit', 'release', 'release_date',
        'track_number', 'disc_number')),
    observation_id INTEGER NOT NULL REFERENCES metadata_observations(id),
    selected_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (recording_id, field)
) STRICT;

CREATE TABLE metadata_resolutions (
    recording_id INTEGER PRIMARY KEY REFERENCES recordings(id),
    state TEXT NOT NULL CHECK (state IN ('pending', 'resolved', 'ambiguous', 'deferred')),
    method TEXT NOT NULL CHECK (method IN ('recording_mbid', 'isrc')),
    candidate_count INTEGER NOT NULL CHECK (candidate_count >= 0),
    message TEXT NOT NULL,
    raw_response_json TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;
