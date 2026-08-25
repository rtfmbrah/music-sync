# Verified adopted/provider links

- Status: Active
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
- [ ] Probe, hash, and fingerprint adopted production artifacts read-only.
- [ ] Enumerate the twelve operator-provided sources and run verified migration.
- [ ] Review unresolved/rejected cases before enabling ordinary acquisition.
