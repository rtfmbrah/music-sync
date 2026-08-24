# Architecture

This is the short entry point. Detailed responsibilities live in
[`docs/architecture/`](docs/architecture/index.md), while accepted decisions live
in [`docs/decisions/`](docs/decisions/README.md).

## Current

```text
music-sync-cli
    |
    v
music-sync library
    +-- configuration
    +-- read-only diagnostics
    +-- preservation and identity policy
    +-- SQLite migration boundary
```

The library contains no terminal presentation. The CLI parses arguments, configures
logging, calls library APIs, and renders results. An automated architecture check
enforces `music-sync-cli -> music-sync` and forbids the reverse dependency.

SQLite owns durable application state. The initial schema deliberately separates
canonical recordings, provider items, physical artifacts, collections and
memberships, and operational runs/jobs/events. The schema is a foundation, not a
claim that synchronization has been implemented.

## Planned flow

```text
configured sources -> provider adapters -> reconciliation -> jobs
                                                    |
                 canonical metadata <--------------+
                                                    |
temporary acquisition -> media validation -> identity verification
          -> atomic artifact commit -> playlist materialization -> Navidrome scan

listening signals -> music discovery -> budget -> same verified acquisition path
```

Source discovery is deterministic reconciliation. Music discovery is probabilistic
recommendation. They share safe acquisition but never share meaning or confidence.
Navidrome remains the playback owner; music-sync never writes its internal database.

## Binding constraints

The [invariants](docs/product/invariants.md) and accepted ADRs are binding. In
particular, synchronization does not delete audio, provider identity is not musical
identity, transient errors never trigger replacement, and incomplete artifacts are
never exposed as complete files.

