# Reconcile source snapshots transactionally

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P1 source synchronization

## Goal

Turn provider snapshots into durable items, memberships, and idempotent acquisition
jobs without deleting media or treating provider failure as an empty snapshot.

## Scope

- Add a forward schema migration linking configured sources to collections.
- Reconcile one successful ordered snapshot in one SQLite transaction.
- Upsert provider metadata separately from canonical recording identity.
- Activate present memberships and deactivate absent memberships only.
- Create at most one acquisition job per provider item across repeated snapshots.
- Record a successful sync run for committed reconciliation.
- Add fixture-backed CLI and persistence idempotency/removal tests.
- Do not download or materialize playlists in this slice.

## Work completed

- Added schema version 3 linking each configured source to one durable collection.
- Added atomic successful-snapshot reconciliation for provider metadata, ordered
  active/inactive memberships, succeeded sync runs, and idempotent acquisition jobs.
- Added `source reconcile` text/JSON CLI behavior using the configured source URL.
- Added migration, repeat-idempotency, membership-removal, provider-mismatch rollback,
  and subprocess provider-failure tests without live network access.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 56 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Provider failure is classified before persistence and leaves prior memberships
  active; successful removal only deactivates absent membership rows.
- Static-musl release build passed and the Debian 12 LXC offline lifecycle produced
  two initial items/memberships/jobs, zero repeat jobs, and unchanged memberships
  after a transient adapter failure. Production music remained read-only.
