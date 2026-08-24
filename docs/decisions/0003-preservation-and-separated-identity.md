# ADR 0003: Preserve media and separate identities

- Status: Accepted
- Date: 2026-08-23

## Context

Remote objects disappear and titles collide. Incorrect automatic replacement is more
damaging than leaving a recording unresolved.

## Decision

Acquired audio is preserved until explicit destructive user intent. Model canonical
recordings, releases, provider items, artifacts, and memberships separately. Removing
remote membership changes membership only. Never replace healthy media. Search for a
replacement only after permanent source loss plus missing/corrupt local media, and
accept it only through conservative canonical and perceptual-audio evidence. Text
similarity cannot prove identity.

## Consequences

Storage is expected to grow and cleanup must be an explicit future workflow.
Transient errors defer. Candidate false negatives are accepted. Tests must cover
preservation and same-title/wrong-audio regressions.

