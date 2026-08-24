# ADR 0004: External media adapters and source-preserving audio

- Status: Accepted
- Date: 2026-08-23

## Context

Media extraction, probing/transcoding, and perceptual fingerprints are specialized,
rapidly evolving capabilities. Format uniformity can destroy quality through
unnecessary lossy transcoding.

## Decision

Wrap `yt-dlp`, `ffmpeg`, `ffprobe`, and Chromaprint/`fpcalc` behind narrow subprocess
boundaries. Isolate YouTube behavior and classify adapter failures into domain
meaning. Preserve the best sensible source codec; normalize organization, metadata,
artwork, and lyrics instead. Remux without re-encoding when justified.

## Consequences

Runtime diagnostics and deployment must check external tools. Deterministic tests use
fixtures/fake executables, while live tests stay opt-in. Navidrome handles playback
transcoding where necessary.

