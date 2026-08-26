CREATE TABLE playlist_preserved_entries (
    collection_id INTEGER NOT NULL REFERENCES collections(id),
    position INTEGER NOT NULL CHECK (position >= 0),
    path TEXT NOT NULL,
    PRIMARY KEY (collection_id, position),
    UNIQUE (collection_id, path)
) STRICT;

-- Older databases stored the provider title as the collection name.  Restore the
-- operator-configured source name before the first schema-19 materialization so
-- an existing local playlist can be matched without another provider request.
UPDATE collections
SET name = (
    SELECT sources.name
    FROM source_collections
    JOIN sources ON sources.id = source_collections.source_id
    WHERE source_collections.collection_id = collections.id
      AND sources.name IS NOT NULL
)
WHERE EXISTS (
    SELECT 1
    FROM source_collections
    JOIN sources ON sources.id = source_collections.source_id
    WHERE source_collections.collection_id = collections.id
      AND sources.name IS NOT NULL
);
