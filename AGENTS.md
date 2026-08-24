# Agent guide

## Mission

music-sync is a resilient, autonomous, provider-agnostic music library manager for
Navidrome. It continuously imports configured sources, preserves acquired audio,
enriches canonical metadata/artwork/lyrics, repairs lost media only with conservative
identity verification, and will discover music autonomously within safety budgets.

## Non-negotiable rules

- This is a clean-room v2 rewrite. Never inspect or derive from legacy source code.
- Preserve successfully acquired audio indefinitely unless the user explicitly asks
  for destructive cleanup.
- Normal sync never deletes audio; source removal only deactivates membership.
- Unknown adopted files are preserved. Never overwrite existing media implicitly.
- A provider object is not a recording identity. Text similarity is not identity.
- Transient network, auth, rate-limit, provider, disk, permission, or processing
  failures never trigger replacement search.
- Healthy local audio is untouched when its remote provider item disappears.
- Replacement is conservative and identity-verified; unresolved beats incorrect.
- Partial downloads never appear as complete artifacts. Work must be resumable.
- Ordinary tests are deterministic and never contact live services.
- All repository content and all program output are English.

## Repository map

- `crates/music-sync/`: reusable domain and boundary library.
- `crates/music-sync-cli/`: thin CLI; may depend on the library only.
- `docs/product/`: vision, requirements, invariants, and authoritative roadmap.
- `docs/architecture/`: current and planned subsystem designs.
- `docs/decisions/`: binding Architecture Decision Records (ADRs).
- `docs/exec-plans/`: active and completed task state.
- `scripts/`: lightweight deterministic repository checks.
- `config.example.toml`: non-secret configuration shape.
- `flake.nix`: reproducible NixOS development shell.
- `justfile`: discoverable local/CI commands.

## Read before changing

- Any behavior: `docs/product/invariants.md` and relevant tests.
- Domain or persistence: `docs/architecture/domain-model.md`.
- Library adoption: `docs/architecture/adoption.md`.
- Source sync: `docs/architecture/synchronization.md` and `providers.md`.
- Downloads or repair: `docs/architecture/acquisition.md` and `audio-identity.md`.
- Metadata, artwork, lyrics: `docs/architecture/metadata.md`.
- Recommendations: `docs/architecture/discovery.md`.
- Navidrome integration: `docs/architecture/navidrome.md`.
- Logs/jobs/progress: `docs/architecture/observability.md`.
- LXC deployment: `docs/deployment/lxc.md`.
- Test changes: `docs/architecture/testing.md`.
- Product sequencing: `docs/product/roadmap.md` and the active execution plan.

## Engineering standards

- Use stable Rust 1.95.0, Edition 2024, safe Rust, rustfmt, and Clippy.
- Keep the library independent of terminal UI and the CLI crate.
- Abstract real external boundaries, not every function.
- Prefer strong types, explicit state transitions, immutable data, composition,
  localized errors, transactional persistence, and idempotent effects.
- Avoid global mutable state, hidden effects, stringly typed state, opaque booleans,
  casual production `unwrap`/`expect`, giant modules, and speculative abstractions.
- Use Tokio only when concurrent I/O provides concrete value; bound concurrency.
- Keep provider-specific behavior in provider adapters. `yt-dlp` is an adapter.
- Reuse specialized subprocess tools (`yt-dlp`, `ffmpeg`, `ffprobe`, `fpcalc`).
- Preserve source audio when sensible; never force lossy-to-lossy conversion merely
  for uniformity.
- Keep secrets outside tracked TOML and source files.
- Public APIs need useful Rustdoc. Comments explain intent and invariants.

## Required commands

Enter the reproducible environment with `nix develop`.

- `just fmt`: format Rust code.
- `just lint`: Clippy all workspace targets with warnings denied.
- `just test`: all deterministic tests.
- `just test-unit`: library unit tests.
- `just test-integration`: integration and scenario tests.
- `just test-architecture`: dependency-direction enforcement.
- `just check`: canonical local and CI verification.
- `just security`: opt-in, network-dependent advisory audit.

Do not add live Internet behavior to `just check`. Provider tests use stored fixtures;
live tests must be explicit and opt-in.

## ADRs and documentation

Accepted ADRs are binding. Never silently contradict one. Supersede a decision with
a new ADR that names the old record and explains migration consequences.

Repository docs must change with behavior. Keep CURRENT and PLANNED claims distinct.
Do not duplicate TODO lists: roadmap owns product sequencing; execution plans own
task-level work. Move completed plans rather than erasing their history.

## Definition of done

Before declaring code work complete:

1. Implement the requested scope without fake commands or stubs.
2. Format code and run Clippy.
3. Add meaningful tests at the appropriate layer and run existing tests.
4. Run the architecture check and `just check`.
5. Document public APIs and update relevant product/architecture docs.
6. Add or supersede an ADR when an architectural decision changes.
7. Update the roadmap and execution-plan status.
8. Inspect status/diff; explain unrelated changes and exclude generated artifacts.
9. Report only checks actually executed, including anything blocked and why.
