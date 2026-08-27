# Simple operator CLI and duplicate evidence

- Status: Complete
- Started: 2026-08-26
- Roadmap: P5 operations and deployment

## Goal

Provide a small operator-facing CLI with global configuration, concise source
listing, a full source/member status tree, and a read-only library duplicate report.
Retain existing technical command paths as compatibility interfaces for service and
recovery automation.

## Safety boundary

- Listing and duplicate reporting never contact providers or mutate SQLite/media.
- Healthy preferred local audio always reports `success`, even after remote loss.
- Text similarity never establishes duplicate identity.
- Exact-byte, same-provider, and strict fingerprint evidence remain distinct.
- No command in this scope deletes, moves, retags, overwrites, or merges audio.
- Existing service unit command lines continue to work.

## Tasks

- [x] Add immutable source/member status queries with explicit precedence.
- [x] Classify copyright only from durable explicit provider diagnostics.
- [x] Add immutable duplicate groups from exact hashes, provider identity, and strict
      duration-aware raw fingerprint comparison.
- [x] Add production-default config resolution, `list [--full]`, `add`, `remove`,
      `sync`, `duplicates`, `backup`, and simplified status history/watch surfaces.
- [x] Retain and test compatible technical command paths.
- [x] Update architecture, operator documentation, roadmap, and CLI help tests.
- [x] Run canonical checks and production-shaped read-only acceptance.

## Acceptance evidence

- `just check` passed with 72 library unit tests, 33 CLI black-box tests, every
  adapter/scenario test, Clippy with warnings denied, and the architecture check.
- A consistent production-state copy retained the same SHA-256 and no SQLite
  sidecars after `list --full` and `duplicates`.
- Twelve sources and 1,571 active members were classified in 156 ms. Duplicate
  analysis considered 1,562 healthy artifacts and 1,473 fingerprints in 8.526 s,
  counted 89 insufficient fingerprints explicitly, and reported 125 non-destructive
  evidence groups.
- Production release `0.1.0-20260827.6` passed doctor and repeated the source/member
  and duplicate reports against live state while retaining the exact database
  SHA-256. The half-hour timer was restored after validation.
