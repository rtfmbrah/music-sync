# Existing library adoption

## CURRENT

`music-sync library adopt <path>` recursively scans an arbitrary directory in dry-run
mode. The reusable library API does not follow symbolic links and classifies regular
files by conservative, case-insensitive extensions:

- common audio containers/codecs as media;
- `.lrc` as lyrics;
- JPEG, PNG, and WebP as artwork candidates;
- M3U/M3U8 as playlists;
- everything else as unknown and preserved.

The scanner opens files to verify readability and marks zero-byte recognized audio as
obviously corrupt. It records traversal/open failures and at most 100 issue details
while retaining complete aggregate counts. Non-Unicode paths are rendered lossily so
reporting cannot break JSON output. Symbolic links are counted but never followed.

The same pass conservatively reports relationships among already observed files.
Same-directory, same-stem LRC files are associated with recognized non-empty media;
artwork is reported as beside media when its directory contains recognized media.
Neither observation establishes recording or release identity.

M3U/M3U8 validation reads at most 8 MiB per playlist, requires UTF-8, ignores blank
and comment lines, and lexically resolves relative paths from the playlist directory.
References are counted as resolved only when they match recognized media from the
same scan. Missing in-root targets and URI/out-of-root targets are bounded issues.
Validation does not canonicalize paths, follow links, fetch URIs, or rewrite lists.

Both text and JSON reports explicitly show zero files modified, deleted, or
downloaded. The default scan remains extension/readability-only. Explicit `--probe`
mode sequentially invokes a bounded ffprobe adapter for each recognized media file;
`--max-probes` limits deterministic path-order sampling and each process has a
configurable non-zero timeout. Captured stdout/stderr is capped at 1 MiB.

Probe summaries count codec, known duration, attached artwork, basic tag presence,
and canonical tag evidence. Recording MBIDs and ISRCs are independently classified
as absent, structurally valid, or malformed; aggregate reports count files in each
useful class without retaining values. A well-formed embedded identifier remains an
unverified claim, not proof that the audio has that identity. Failures and timeouts
become bounded issues and do not mark media for deletion or replacement.

Explicit `--hash` mode streams recognized media through SHA-256 in deterministic
path order. `--max-hashes` limits work independently from media probing, and both
modes may run in one scan. Reports contain attempts, successful bytes, failures,
skips, and exact-duplicate group/file counts but omit digest values. Equal hashes
prove exact artifact bytes only; they do not prove that distinct encodings contain
the same recording, and duplicate observations never trigger cleanup.

Explicit `--apply --database <path>` registers every recognized non-empty media path
in music-sync-owned SQLite using one transaction. Apply requires an absolute library
root, does not permit probe/hash options in this first persistence slice, and leaves
existing artifact rows unchanged. Each new artifact receives a separate unresolved
recording row with no invented canonical or provider identity and `unknown` health.
Reports distinguish inserted/existing database rows from the guaranteed zero media
file effects. A repeated apply is idempotent for the same absolute paths. Non-Unicode
paths cannot be represented losslessly in SQLite text and roll back the entire batch.

A successful probe establishes container/audio structure, not canonical recording
identity or a full decode of every frame.

## PLANNED

Add deeper decode validation, provider-ID observation, external corroboration,
perceptual fingerprints, and canonical identification states. Expensive operations
must be observable and safely resumable.

Extend apply with persisted probe/hash evidence, root-scoped reconciliation, and
explicit canonical identification states. Missing paths must be reconciled
conservatively and never cause automatic file deletion or replacement.

## NON-NEGOTIABLE

Unknown means preserve. Scanning is read-only, does not follow links, and never
requires redownload. A filename or extension cannot establish recording identity.
Corruption or duplicate suspicion is reported, never cleaned automatically.
