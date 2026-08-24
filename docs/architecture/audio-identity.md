# Audio identity and replacement

## CURRENT

The library's replacement policy accepts only independently compatible canonical
metadata plus a positive result from an audio fingerprint boundary. A fingerprint
mismatch rejects a candidate; unavailable evidence remains unresolved. Candidate
titles are deliberately absent from verification input. Safety scenarios cover the
historical class of same-title/wrong-recording failures without consulting legacy
implementation.

Adoption can observe structurally valid embedded recording MBIDs and ISRCs. These
identifiers may support future canonical resolution, but an internally well-formed
tag can still be wrong and therefore does not independently verify audio identity.

Acquisition staging can now retain a SHA-256 digest after structural audio
validation. This proves exact artifact bytes only; it is deliberately not used as
perceptual or canonical recording evidence.

`library health` checks registered artifacts in stable bounded order without media
mutation. Absent paths become missing; non-regular paths and exact-hash mismatch
become corrupt. Probe, permission, and hashing failures leave prior health unchanged
because insufficient infrastructure evidence is not corruption. Successful checks
refresh structural properties and exact SHA-256. No health result starts replacement
search in this phase.

Schema version 7 stores bounded raw Chromaprint algorithm-2 evidence separately from
exact artifact hashes. `library fingerprint` and the automatic post-acquisition sync
phase select healthy artifacts in stable bounded order, reject paths outside the
configured library and symbolic links, invoke `fpcalc` with explicit output, time,
and audio-length bounds, and retain prior evidence when one extraction fails.
Repeating the same extraction bound is idempotent.

Raw comparison requires at least 120 overlapping values, searches at most 120 values
of alignment, permits at most two average differing bits per value, and requires
durations within three seconds. These deliberately strict near-identity thresholds
produce match, mismatch, or insufficient-evidence results; they do not identify
covers or infer identity from names.

Schema version 8 separates repair cases and candidate attempts from acquisition jobs.
The explicit `repair assess` workflow checks only active provider items whose
preferred artifact is already missing or corrupt. A durable eligible case requires a
direct, explicitly permanent provider failure and retained raw reference fingerprint;
transient/authentication/rate-limit/extraction failures create no case. Recovered
health, membership removal, or restored provider availability cancels an unresolved
case. `repair generate` performs bounded title-based search but stores every result
as generated-only evidence; search position and text confer no identity trust.

## PLANNED

AcoustID and MusicBrainz may corroborate embedded canonical evidence. Persist file
hashes for integrity, not as perceptual identity. Duration and qualifiers filter
versions before a temporary candidate is downloaded and fingerprinted.

Pipeline:

```text
permanent original loss + missing/corrupt artifact
-> candidate generation (text allowed)
-> canonical metadata, duration, and version filtering
-> temporary candidate acquisition
-> audio fingerprint verification
-> mismatch: reject | insufficient: unresolved | verified: atomic commit
```

Thresholds and qualifier behavior require fixture-based tests before production use.
The first search result has no special trust.

## NON-NEGOTIABLE

Healthy local audio means no replacement. Infrastructure/transient errors mean no
replacement search. Text similarity never proves recording identity, and false
negatives are preferable to false positives.
