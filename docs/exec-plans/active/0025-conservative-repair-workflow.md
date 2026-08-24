# Conservative repair workflow

- Status: Active
- Started: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Turn permanent original loss plus missing/corrupt local media into an auditable,
bounded repair workflow that can only commit independently identity-verified audio.

## Scope

- Persist provider availability observations and repair cases/attempts.
- Create eligibility only for active membership, unhealthy artifact, definitive
  permanent original loss, and retained reference evidence.
- Add bounded provider candidate search and temporary staged validation.
- Filter canonical identity, duration, qualifiers, and raw fingerprints before any
  atomic no-clobber commit.
- Expose inspect/run/retry behavior with deterministic fixtures and operational
  status.

## Safety boundary

- Transient, authentication, rate-limit, extraction, disk, and permission failures
  never generate replacement candidates.
- Healthy media and inactive membership never enter repair.
- Search text generates candidates but never verifies them.
- Rejected and insufficient candidates remain staged/audited and never replace media.
- A verified commit preserves prior artifact rows and never overwrites a path.

## Progress

- [x] Persist eligibility and provider observations.
- [x] Generate bounded candidates through a provider adapter as untrusted evidence.
- [ ] Validate and independently verify staged candidates.
- [ ] Commit verified replacement atomically and expose operations.
- [ ] Run deterministic, architecture, and isolated LXC validation.
