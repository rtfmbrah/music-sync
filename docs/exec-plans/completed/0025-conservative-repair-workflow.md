# Conservative repair workflow

- Status: Complete
- Started: 2026-08-24
- Completed: 2026-08-25
- Roadmap: P2 acquisition and identity

## Goal

Turn permanent original loss plus missing/corrupt local media into an auditable,
bounded repair workflow that can only commit independently identity-verified audio.

## Scope delivered

- Persist provider availability observations, eligibility, cases, attempts, staged
  evidence, decisions, and recoverable commit intent.
- Require active membership, unhealthy artifact, definitive permanent original loss,
  and retained reference evidence before search or commit.
- Generate bounded untrusted provider candidates and claim attempts exclusively.
- Validate staged structure/bytes, embedded canonical identity, duration, and raw
  fingerprints before accepting a candidate.
- Expose assess/generate/run-one/commit/retry/recover and operational status.

## Safety boundary

- Transient, authentication, rate-limit, extraction, disk, and permission failures
  never generate replacement eligibility.
- Healthy media and inactive membership never enter repair.
- Search text generates candidates but never verifies them.
- Rejected and insufficient candidates remain staged/audited and never replace media.
- A verified commit rehashes staging, rechecks live prerequisites, preserves prior
  artifact rows, and never overwrites a path.

## Result

- Schema versions 8–10 persist conservative eligibility, untrusted candidates,
  exclusive attempts, complete identity evidence, recoverable commit intent, and
  acquisition canonical evidence.
- Verified commit creates a new no-clobber artifact, preserves historical media
  state, and updates preference only transactionally.
- Deterministic repository checks and a full offline LXC acceptance path pass while
  production music remains read-only.
