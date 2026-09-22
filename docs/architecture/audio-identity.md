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

For autonomous discovery, a curated MusicBrainz recording-to-URL relationship is
the provider-object assertion, not the audio identity proof. The downloaded file
must independently expose the asserted recording MBID or unique canonical ISRC and
match canonical duration within two seconds before the ordinary crash-safe commit
can be prepared. Text and recommendation rank never enter this comparison.

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
Repeating the same extraction bound is idempotent. Some `fpcalc` releases emit a
complete JSON fingerprint and then exit 3 with the exact diagnostic `Error decoding
audio frame (End of file)` for otherwise decodable Opus files. The adapter accepts
only that exact exit/diagnostic combination with non-empty output, then applies the
same strict JSON, duration, count, and size validation as a successful invocation.
Every other non-zero exit remains a failure.

Schema version 18 durably defers an isolated artifact fingerprint failure at its
requested extraction bound. Ordinary service cycles exclude that artifact instead
of repeatedly invoking fpcalc and blocking later unfingerprinted artifacts. Missing
or outdated evidence is prioritized ahead of already-current evidence. An operator
may explicitly release one artifact with `library retry-fingerprint`; changing the
configured extraction bound also makes it eligible without discarding the retained
diagnostic. Successful extraction clears stale deferral evidence.

Raw comparison requires at least 120 overlapping values, searches at most 120 values
of alignment, permits at most two average differing bits per value, and requires
durations within three seconds. These deliberately strict near-identity thresholds
produce match, mismatch, or insufficient-evidence results; they do not identify
covers or infer identity from names.

The read-only `duplicates` report reuses this exact strict comparator without
promoting a match to canonical identity. It reports exact SHA-256 groups, multiple
healthy artifacts associated with one provider object, and duration-compatible raw
Chromaprint match candidates as separate evidence classes. Exact-byte equality does
not authorize deletion, provider identity is not recording identity, and a
fingerprint match remains an operator-review candidate. Artifacts lacking usable
fingerprints are counted explicitly. The report neither contacts providers nor
changes SQLite or media.

Schema version 8 separates repair cases and candidate attempts from acquisition jobs.
The explicit `repair assess` workflow checks only active provider items whose
preferred artifact is already missing or corrupt. A durable eligible case requires a
direct, explicitly permanent provider failure and retained raw reference fingerprint;
transient/authentication/rate-limit/extraction failures create no case. Recovered
health, membership removal, or restored provider availability cancels an unresolved
case. `repair generate` performs bounded title-based search but stores every result
as generated-only evidence; search position and text confer no identity trust.

Schema version 9 adds exclusive candidate claims, stable per-attempt staging,
deferred/retryable failures, complete verification evidence, and recoverable commit
intent. `repair run-one` downloads at most one generated candidate, validates stable
bytes and audio structure, retains actual embedded canonical identifier values,
compares duration and raw Chromaprint evidence, and records rejected, unresolved, or
verified. Missing canonical or duration evidence is unresolved rather than mismatch.
Only a match with no contradiction becomes verified.

`repair commit` rehashes verified staging immediately before creating a new managed
`repair/recording-<id>-attempt-<id>.<ext>` hard link. It never overwrites or removes
the missing/corrupt reference path. Prepared intent makes a crash after linking
recoverable; finalization transactionally creates a healthy artifact, stores its
fingerprint, associates the independently verified candidate provider object, and
makes the new artifact preferred. Live membership, permanent-loss, and unhealthy
reference prerequisites are checked again before and during commit.

The optional discovery search uses AcoustID to corroborate canonical evidence when
provider URL and local tags are absent. It sends only a compressed Chromaprint and
duration and requires the exact expected MusicBrainz recording MBID in a strict,
non-contradictory response. No result is insufficient evidence rather than a
mismatch. Meaningful version qualifiers come from MusicBrainz rather than title
inference.

## PLANNED

Pipeline:

```text
permanent original loss + missing/corrupt artifact
-> candidate generation (text allowed)
-> canonical metadata, duration, and version filtering
-> temporary candidate acquisition
-> audio fingerprint verification
-> mismatch: reject | insufficient: unresolved | verified: atomic commit
```

Qualifier behavior requires fixture-based tests alongside canonical metadata
resolution. The first search result has no special trust.

## NON-NEGOTIABLE

Healthy local audio means no replacement. Infrastructure/transient errors mean no
replacement search. Text similarity never proves recording identity, and false
negatives are preferable to false positives.
