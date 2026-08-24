# Source-preserving canonical tag materialization

- Status: Complete
- Started: 2026-08-25
- Completed: 2026-08-25
- Roadmap: P3 metadata enrichment

## Goal

Materialize selected canonical metadata into managed media without transcoding audio,
losing original acquired bytes, or exposing an unvalidated partial replacement.

## Result

- Schema version 14 stores durable hidden-staging ownership, explicit deferral,
  validated prepared/committed intent, source history, and derived artifact identity.
- The bounded ffmpeg adapter uses literal audio stream copy and canonical field maps.
  Canonical art becomes an attached stream; Opus/Ogg uses a file-backed standard
  picture block so image size is not constrained by process argument limits.
- Source bytes are hash-verified into immutable content-addressed history before a
  hidden result is structurally probed and compared for codec, rate, channels, and
  duration. Atomic replacement precedes a recoverable transactional preference change.
- Unknown staging and changed media are preserved/deferred; retry is explicit and
  committed repetitions perform no subprocess or filesystem work.
- Deterministic no-transcode/artwork/archive/interruption/CLI tests, canonical checks,
  and real ffmpeg/ffprobe schema-v14 LXC validation pass with production read-only.
