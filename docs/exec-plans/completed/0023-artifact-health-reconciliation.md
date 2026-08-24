# Artifact health reconciliation

- Status: Complete
- Started: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Boundedly reconcile registered artifact health from conservative local evidence
without modifying media or initiating replacement search.

## Scope

- Load registered artifacts in stable ID order with an explicit limit.
- Reject paths outside the configured library and never follow symbolic links.
- Mark absent files missing; mark exact-hash mismatches and non-regular paths corrupt.
- Treat permission, hashing, and probe failures as isolated unresolved checks rather
  than corrupt evidence.
- Refresh successful hash and structural media properties transactionally.
- Add text/JSON CLI behavior, deterministic tests, status integration, and isolated
  LXC validation.

## Safety boundary

- Health reconciliation never writes, renames, tags, or deletes media.
- A check failure never triggers replacement search or changes prior health.
- Healthy preferred artifacts remain untouched; missing/corrupt preference merely
  becomes unavailable to playlist materialization.

## Result

- Added bounded stable artifact selection and conservative local reconciliation.
- Missing paths, non-regular paths, and trusted SHA-256 mismatch have explicit
  states; infrastructure/probe failures preserve prior health.
- Successful checks refresh structural/hash evidence without changing media.
- Unit and black-box tests cover healthy, missing, persistent mismatch, repeat, and
  insufficient-evidence behavior; `just check` passes with 81 tests.
- Static-musl LXC validation preserved exact media bytes and production read-only
  access.
