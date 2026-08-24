# Architecture guide

The root [architecture overview](../../ARCHITECTURE.md) is the entry point. This
directory separates subsystem responsibilities:

- [domain model](domain-model.md): identities, ownership, and persisted concepts;
- [adoption](adoption.md): read-only inspection of arbitrary existing libraries;
- [synchronization](synchronization.md): deterministic source reconciliation;
- [acquisition](acquisition.md): crash-safe media production;
- [audio identity](audio-identity.md): replacement verification policy;
- [metadata](metadata.md): canonical enrichment and provenance;
- [discovery](discovery.md): autonomous probabilistic recommendations;
- [providers](providers.md): external adapters and failure classification;
- [Navidrome](navidrome.md): filesystem integration and listening signals;
- [observability](observability.md): logs, progress, runs, jobs, and recovery;
- [testing](testing.md): deterministic layered verification.

Each document labels implemented facts as **CURRENT**, future intent as **PLANNED**,
and immutable safety rules as **NON-NEGOTIABLE**. Accepted ADRs explain why major
choices were made.
