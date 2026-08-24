# Raw fingerprint evidence

- Status: Complete
- Started: 2026-08-24
- Roadmap: P1 adoption evidence and P2 acquisition identity

## Goal

Derive, persist, and compare bounded perceptual evidence for healthy artifacts while
keeping extraction failures isolated and never treating a fingerprint as canonical
recording identity.

## Scope

- Add a bounded raw algorithm-2 `fpcalc` subprocess adapter.
- Store versioned artifact evidence through a forward SQLite migration.
- Reconcile fingerprints in stable bounded order through CLI and ordinary sync.
- Define strict near-identical comparison with explicit insufficient evidence.
- Cover subprocess, persistence, repeat, sync, and comparison behavior with offline
  fixtures.

## Safety boundary

- Fingerprinting reads healthy regular media only and never changes it.
- Paths outside the configured library and symbolic links are rejected.
- Extraction failure retains prior state and cannot initiate replacement.
- Text, provider identity, and a raw fingerprint alone never establish canonical
  recording identity.

## Result

- Schema version 7 retains bounded raw Chromaprint evidence per artifact.
- Manual reconciliation and the post-acquisition sync phase are bounded and
  idempotent, with per-artifact failures reported separately.
- Comparison uses strict overlap, alignment, bit-error, and duration thresholds and
  returns match, mismatch, or unavailable.
- The canonical deterministic repository check passes with 87 tests.
