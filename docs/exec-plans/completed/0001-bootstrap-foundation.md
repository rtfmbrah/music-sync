# Bootstrap repository foundation

- Status: Completed
- Started: 2026-08-23
- Completed: 2026-08-23
- Roadmap: P0

## Goal

Create a compiling, tested, documented, agent-native Rust foundation from the v2
specification without inspecting any legacy implementation.

## Scope and safety

- Create the smallest library-first workspace and truthful `doctor` vertical slice.
- Establish configuration, tracing, SQLite migrations, safety policy, tests, Nix,
  task automation, roadmap, architecture docs, and ADRs.
- Do not contact providers, modify media, or implement pretend future commands.
- Preserve English-only content and forbid unsafe project code.

## Work completed

- Inventoried the empty v2 workspace only; no legacy code was sought or read.
- Created a pinned Rust workspace, library, CLI, and lint/format policy.
- Implemented config, read-only diagnostics, SQLite migration, preservation, and
  replacement-evidence policy.
- Added deterministic unit, black-box CLI, safety scenario, and dependency-direction
  tests.
- Added Nix shell/lock, Cargo lock, `justfile`, configuration example, ignore rules,
  and a provider-neutral CI contract.
- Wrote focused product/architecture documentation and five accepted ADRs.

## Decisions

Rust 1.95.0 is pinned because it is the stable toolchain present in the development
environment and supports Edition 2024. SQLite is bundled in Rust builds for Linux
portability; the Nix shell also provides the SQLite CLI. Missing future media tools
are warnings in read-only `doctor` until their dependent workflows exist.

## Validation evidence

- `cargo check --workspace --all-targets --all-features`: passed.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: passed.
- `cargo test --workspace --all-features`: passed (16 tests).
- `python3 scripts/check_architecture.py`: passed.
- `nix develop path:. -c just check`: passed with Rust 1.95.0 and pinned inputs.
- `nix flake check path:.`: passed for the local x86_64-linux system; Nix reported
  aarch64-linux as unevaluated on this host.
- CLI `--version`, `--help`, and offline JSON `doctor`: passed in the Nix shell;
  doctor found `yt-dlp`, `ffmpeg`, `ffprobe`, and `fpcalc` and made no directories.
- `nix develop path:. -c cargo audit`: passed, scanning 83 dependencies against
  1,225 loaded RustSec advisories.

The explicit `path:.` flake reference was used because this newly initialized Git
repository has no first commit and therefore all files are untracked. After the first
commit, the documented ordinary `nix develop` command resolves the same locked flake.
