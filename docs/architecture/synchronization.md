# Source synchronization

## CURRENT

The `preservation` module implements a small pure reconciliation policy. Inactive
membership changes only membership; healthy artifacts are preserved regardless of
remote availability; transient failures defer; replacement search is eligible only
after permanent loss with no healthy artifact. Scenario tests lock these properties.

The provider boundary can enumerate one YouTube video or playlist into an ordered,
provider-neutral snapshot. `source reconcile` enumerates one active configured source
and commits a successful snapshot in one SQLite transaction. Provider items remain
separate from canonical recording identity; present memberships are activated in
provider order, absent memberships are only deactivated, and acquisition jobs use a
stable provider-item idempotency key. Provider failures occur before reconciliation
and therefore cannot masquerade as an empty snapshot.

## PLANNED

Synchronization proceeds in bounded, independently recoverable phases:

1. enumerate configured source identities cheaply through a provider adapter;
2. compare the snapshot with durable provider items and memberships (CURRENT);
3. transactionally activate/deactivate membership and create idempotent jobs
   (CURRENT);
4. run expensive resolution/acquisition/enrichment only for new or incomplete state;
5. atomically materialize M3U8 collections from active memberships (CURRENT).

A failed item records its outcome and does not stop other items. A second identical
snapshot creates no acquisition or filesystem work. Removed source items retain
provider history and artifacts.

Source discovery answers what belongs to a configured source. It does not generate
taste recommendations; that belongs to music discovery.

## NON-NEGOTIABLE

Normal sync never deletes audio, never overwrites unknown files, never redownloads
unchanged healthy artifacts, and never treats a transient provider failure as remote
deletion.
