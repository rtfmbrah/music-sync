CREATE TABLE sources (
    id INTEGER PRIMARY KEY,
    provider TEXT NOT NULL,
    url TEXT NOT NULL,
    name TEXT,
    active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (provider, url)
) STRICT;
