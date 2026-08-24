# Validate staged media structurally and cryptographically

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Turn an untrusted staged download into explicit structural and exact-byte evidence
without modifying it or committing it to the managed library.

## Scope

- Compose the existing ffprobe and SHA-256 boundaries for one staged file.
- Require a regular non-empty file and a structurally valid audio stream.
- Detect size changes across probe/hash validation.
- Return durable-ready codec, duration, stream properties, byte count, and SHA-256.
- Add deterministic success, probe failure, and mutation/size mismatch tests.
- Do not fingerprint, move, or persist a final artifact in this slice.

## Safety boundary

- Validation is read-only and never deletes failed staging evidence.
- Hash evidence identifies exact bytes, not canonical musical identity.
- A failed validation can only defer the acquisition job.

## Work completed

- Added a read-only staged-media validator composing the existing probe and hash
  boundaries without coupling them to terminal presentation.
- Added non-empty regular-file checks and stable-size verification before and after
  probe/hash work.
- Added a durable-ready result containing exact SHA-256/bytes and structural audio
  properties while keeping canonical identity explicitly unresolved.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 67 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Tests cover known SHA-256 evidence, structural probe failure, byte-count mismatch,
  and preservation of staged bytes after rejected validation.
