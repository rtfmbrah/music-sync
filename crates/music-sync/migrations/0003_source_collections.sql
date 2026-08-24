CREATE TABLE source_collections (
    source_id INTEGER PRIMARY KEY REFERENCES sources(id),
    collection_id INTEGER NOT NULL REFERENCES collections(id)
) STRICT;
