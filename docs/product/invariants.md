# Product invariants

This document is the source of truth for non-negotiable safety properties. Code,
tests, configuration, and future designs must uphold them.

## Library preservation

1. Successfully acquired audio remains local until the user explicitly requests
   destructive cleanup.
2. Normal synchronization never deletes acquired audio.
3. Remote collection removal deactivates membership only.
4. Provider unavailability never modifies or deletes a healthy local artifact.
5. Unknown files found during adoption are preserved.
6. Existing media is never overwritten without verified intent.
7. Discovery-acquired audio follows the same preservation rules.

## Idempotency and resilience

- With no relevant external change, a repeated sync performs no download or file
  mutation: `sync(sync(state)) == sync(state)`.
- One failed or unresolved track never blocks unrelated synchronization,
  enrichment, playlist output, discovery, or acquisition.
- Partial downloads stay in job-specific temporary space and never appear as a
  completed library artifact.
- State transitions are transactional and work is safely resumable after a crash.
- Expensive resolution is performed only for changed or incomplete items.

## Identity

- Canonical recording, release, provider item, physical artifact, and collection
  membership are different identities.
- A YouTube video ID is not a recording ID.
- A recording may appear on several releases and in several collections while using
  one preferred artifact.
- Text, filenames, and titles may generate candidates; they never prove audio
  identity.
- Duration and version qualifiers (live, acoustic, remix, remaster, instrumental,
  karaoke, cover, sped/slowed, nightcore, extended, radio edit, demo, official
  audio, music video) are meaningful matching evidence.

## Replacement safety

Replacement search is forbidden for timeouts, temporary network or DNS failures,
rate limits, authentication/cookie/token failures, provider extraction changes,
disk-full or permission problems, and ffmpeg/post-processing failures.

Search begins only when both the artifact is missing/corrupt and the original source
is permanently unavailable. Candidates are never accepted by rank or text alone.
They are staged, filtered by canonical metadata, duration, and version, and—when
reference evidence exists—verified using perceptual audio identity before atomic
commit. A mismatch is rejected; insufficient evidence remains unresolved.

## Ownership boundaries

- music-sync owns its SQLite state, managed media, metadata, artwork, lyrics,
  playlists, acquisition state, and discovery state.
- Navidrome owns playback, streaming, users, favorites, ratings, play counts, and its
  private database. music-sync never manipulates that database.
- Development through Nix must not make Nix a runtime dependency.

