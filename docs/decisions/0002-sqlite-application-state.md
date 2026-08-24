# ADR 0002: SQLite is application state

- Status: Accepted
- Date: 2026-08-23

## Context

Synchronization, acquisition, recovery, and discovery require durable identity and
job history that cannot be reconstructed reliably from filenames.

## Decision

Use music-sync-owned SQLite as the persistent source of truth with ordered migrations
from the first release, enabled foreign keys, transactional state changes, and
deliberately separated domain tables. Reconcile SQLite and filesystem state rather
than treating either as implicitly authoritative for both.

## Consequences

Schema changes require forward migrations and integration tests. Operational runs,
jobs, attempts, and events can survive process crashes. music-sync never accesses or
modifies Navidrome's internal database.

