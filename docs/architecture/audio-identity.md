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

## PLANNED

Use `fpcalc`/Chromaprint to derive perceptual fingerprints and a documented comparison
method, retaining reference fingerprints after acquisition. AcoustID and MusicBrainz
may corroborate embedded canonical evidence. Persist file hashes for integrity, not as perceptual
identity. Duration and qualifiers filter versions before a temporary candidate is
downloaded and fingerprinted.

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
