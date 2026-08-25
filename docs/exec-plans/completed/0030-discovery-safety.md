# Autonomous discovery signals, scoring, and safety budgets

- Status: Complete
- Started: 2026-08-25
- Roadmap: P4 autonomous discovery

## Goal

Generate explainable canonical recording recommendations from read-only taste signals,
deduplicate them, and approve growth only inside hard daily, artist-diversity, lane,
and free-storage budgets.

## Safety boundary

- Discovery is disabled by default and does no work without explicit configuration.
- Recommendations require valid recording MBIDs; text never establishes identity.
- Navidrome access is read-only through its public API, never its private database.
- Scores use deterministic fixed-point arithmetic and persist their explanation.
- Existing recordings and prior queued/acquired candidates are exact-MBID duplicates.
- Daily total, per-artist, exploration, wildcard, and minimum-free-space limits are
  hard gates checked transactionally before approval.
- Provider and infrastructure failures cannot create acquisition jobs.
- Acquisition routing requires a canonical provider-URL relationship and independent
  staged canonical/duration compatibility; otherwise unresolved wins.
- Ordinary tests use stored fixtures and loopback HTTP only.

## Tasks

- [x] Add discovery runs, seeds, candidates, decisions, and daily budget accounting.
- [x] Add deterministic explainable scoring and exact canonical deduplication.
- [x] Add read-only Navidrome signal adapter and ListenBrainz recommendations.
- [x] Add bounded storage guard and lane/artist/daily enforcement.
- [x] Add strong MusicBrainz provider-relationship acquisition routing.
- [x] Strengthen P2 staging checks for discovery assertions.
- [x] Add CLI/status/orchestration, deterministic scenarios, and documentation.
- [x] Run canonical checks and isolated LXC validation.

## Validation

- `nix develop --command env CARGO_TARGET_DIR=/tmp/music-sync-v2-p4-target just check`
  passed formatting, Clippy with warnings denied, 116 deterministic tests,
  architecture enforcement, and fixture shell validation.
- A static musl release ran as `music-sync-dev` on Debian 12 in
  `/srv/music-sync-v2-test/discovery-20260825`.
- Loopback ListenBrainz and MusicBrainz fixtures produced one approved candidate and
  one exact recording-level YouTube acquisition assertion in schema 15.
- Fixture acquisition independently exposed the asserted recording MBID and ISRC at
  the canonical 180-second duration, committed one owned artifact, and moved the
  candidate from `queued` to `acquired`.
- `/srv/music` remained readable and non-writable throughout validation. No
  production service, database, configuration, or media was changed.
