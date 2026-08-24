# ADR 0001: Rust library-first workspace

- Status: Accepted
- Date: 2026-08-23

## Context

Core behavior must be reusable by other Rust programs, while terminal concerns and
future automation interfaces must not contaminate domain behavior.

## Decision

Use stable Rust 1.95.0, Edition 2024, safe Rust by default, and a Cargo workspace with
the reusable `music-sync` library plus the thin `music-sync-cli` binary. The CLI may
depend on the library; the reverse is forbidden and mechanically checked. Add crates
only when a demonstrated boundary requires them. Use Tokio only for concrete async
I/O value.

## Consequences

Public library APIs need Rustdoc and must return presentation-neutral results. The CLI
owns arguments, terminal/JSON rendering, exit codes, and logging initialization.
Rustfmt, Clippy, and workspace tests are release gates.

