CREATE TABLE service_runs (
    id INTEGER PRIMARY KEY,
    trigger TEXT NOT NULL CHECK (trigger IN ('manual','timer','recovery_test')),
    status TEXT NOT NULL CHECK (status IN ('running','succeeded','partial','failed','interrupted')),
    binary_version TEXT NOT NULL,
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT,
    summary_json TEXT NOT NULL DEFAULT '{}',
    CHECK ((status = 'running' AND finished_at IS NULL) OR
           (status != 'running' AND finished_at IS NOT NULL))
) STRICT;

CREATE UNIQUE INDEX service_runs_single_running
ON service_runs((1)) WHERE status = 'running';

CREATE TABLE service_run_phases (
    service_run_id INTEGER NOT NULL REFERENCES service_runs(id),
    phase TEXT NOT NULL,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    status TEXT NOT NULL CHECK (status IN ('running','succeeded','partial','failed','skipped')),
    started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    finished_at TEXT,
    summary_json TEXT NOT NULL DEFAULT '{}',
    message TEXT,
    PRIMARY KEY (service_run_id, phase),
    UNIQUE (service_run_id, ordinal),
    CHECK ((status = 'running' AND finished_at IS NULL) OR
           (status != 'running' AND finished_at IS NOT NULL))
) STRICT;

CREATE INDEX service_runs_newest ON service_runs(id DESC);
CREATE INDEX service_run_phases_by_run ON service_run_phases(service_run_id, ordinal);
