# Verified adopted/provider links

- Status: Complete
- Started: 2026-08-25
- Roadmap: production migration prerequisite

## Goal

Prevent duplicate first-run downloads by independently proving which enumerated
YouTube objects are already represented by preserved adopted artifacts.

## Safety boundary

- Filenames and titles generate candidates only.
- Existing adopted audio remains byte-for-byte untouched.
- Provider staging is retained and never appears as a complete library artifact.
- Only duration plus raw Chromaprint agreement may associate identities.
- Mismatch, ambiguity, and insufficient evidence remain unresolved.
- Production service activation waits for an explicit migration report.

## Tasks

- [x] Add durable schema and transactional verified association.
- [x] Add bounded resumable verification workflow and CLI.
- [x] Add deterministic matching and no-duplicate scenario coverage.
- [x] Run canonical checks and isolated LXC fixture acceptance.
- [x] Add an audited first-activation quarantine for unresolved filename candidates.
- [x] Persist and explicitly retry isolated production fingerprint failures.
- [x] Probe, hash, and fingerprint adopted production artifacts read-only.
- [x] Enumerate the twelve operator-provided sources and run verified migration.
- [x] Review unresolved/rejected cases before enabling ordinary acquisition.

## Production result

The 2026-08-25 migration preserved and registered 1,560 healthy production audio
artifacts. It proved 1,127 adopted/provider links, quarantined 205 unresolved exact
filename candidates, deferred 117 genuinely unavailable provider items, and
persisted 88 isolated fingerprint failures for explicit retry. No production audio
was rewritten or deleted. Schema 18, release `0.1.0-20260825.3`, the paced service,
the half-hour sync timer, and the daily backup timer are active on the LXC.
