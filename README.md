# music-sync v2

music-sync is a resilient, autonomous, provider-agnostic music library manager for
Navidrome. It imports explicitly configured sources, preserves acquired audio,
enriches canonical metadata, safely repairs unavailable media through verified
identity, and will autonomously discover new music within configured budgets.

This repository is a clean-room rewrite. Previous implementations are not design
inputs and must not be inspected. See [product vision](docs/product/vision.md),
[invariants](docs/product/invariants.md), and [architecture](ARCHITECTURE.md).

## Current foundation

The Rust workspace contains:

- `music-sync`, a reusable library with configuration, SQLite migrations,
  read-only diagnostics, preservation policy, and conservative replacement policy;
- `music-sync-cli`, a thin binary exposing a real local `doctor` command;
- a read-only adoption scanner plus explicit transactional artifact registration;
- persistent YouTube source management, inspect-only enumeration, and transactional
  membership reconciliation with idempotent acquisition jobs;
- a one-job acquisition command with durable staging, yt-dlp download, ffprobe/hash
  validation, atomic no-clobber artifact commit, and deferred retry;
- bounded batch acquisition and atomic Navidrome-compatible M3U8 materialization
  from active memberships and preferred healthy artifacts;
- deterministic unit, integration/scenario, and dependency-direction checks;
- a pinned Rust/Nix development environment and repository-native documentation.

Metadata enrichment and discovery remain documented plans rather than implemented
commands.

## Development

```bash
nix develop
just check
cargo run -p music-sync-cli -- --help
cargo run -p music-sync-cli -- doctor --config config.example.toml
cargo run -p music-sync-cli -- library adopt /path/to/music
cargo run -p music-sync-cli -- library adopt /path/to/music --probe --max-probes 25
cargo run -p music-sync-cli -- source add 'https://www.youtube.com/playlist?list=…' --database state.sqlite3
cargo run -p music-sync-cli -- source reconcile 1 --database state.sqlite3
cargo run -p music-sync-cli -- acquisition run-one --config music-sync.toml
cargo run -p music-sync-cli -- acquisition run-pending --max-jobs 100 --config music-sync.toml
cargo run -p music-sync-cli -- playlist materialize --config music-sync.toml
cargo run -p music-sync-cli -- sync run --max-jobs 100 --config music-sync.toml
cargo run -p music-sync-cli -- status --config music-sync.toml
```

The example configuration uses production-style placeholder paths and will report
missing directories unless they exist. `doctor` is read-only and never contacts a
provider. Use `--json` for machine-readable results and `-v` through `-vvv` for
increasing log detail.

The canonical pre-commit and CI command is `just check`. The separate
`just security` command needs network access and an up-to-date advisory database.
Use `just build-portable` inside the Nix shell for a static x86_64 Linux deployment
artifact. Confirmed LXC details and safety boundaries are documented in
[`docs/deployment/lxc.md`](docs/deployment/lxc.md).
Example timer-driven operation is provided under `deploy/systemd/`; paths and the
dedicated service account must be adapted to the target host before installation.

## Configuration and secrets

Copy `config.example.toml` to a private deployment location and adjust paths.
Ordinary behavior belongs in TOML. Future provider credentials belong in a secret
store or environment variables and must never be committed. No secrets are needed
by the current foundation.

## Project status

The authoritative sequence of future work is in the
[roadmap](docs/product/roadmap.md). Active work must have an execution plan under
`docs/exec-plans/active/`; architectural changes require an ADR.
