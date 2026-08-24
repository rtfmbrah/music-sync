set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# Format all Rust sources.
fmt:
    cargo fmt --all

# Verify formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Run Clippy for every target and fail on warnings.
lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Run all deterministic tests.
test:
    cargo test --workspace --all-features

# Run library unit tests only.
test-unit:
    cargo test --workspace --all-features --lib

# Run integration and scenario test targets.
test-integration:
    cargo test --workspace --all-features --tests

# Enforce workspace dependency direction.
test-architecture:
    python3 scripts/check_architecture.py

# Validate deployment shell scripts without network or side effects.
test-scripts:
    bash -n scripts/install-ytdlp crates/music-sync/tests/fixtures/acquisition/*.sh crates/music-sync/tests/fixtures/repair/*.sh

# Apply safe formatter and Clippy suggestions.
fix:
    cargo fmt --all
    cargo clippy --workspace --all-targets --all-features --fix --allow-dirty --allow-staged

# Canonical deterministic local and CI verification.
check: fmt-check lint test test-architecture test-scripts

# Build a portable static x86_64 Linux release binary for Debian-like targets.
build-portable:
    cargo build --locked --release -p music-sync-cli --target x86_64-unknown-linux-musl

# Network-dependent Rust dependency advisory check.
security:
    cargo audit
