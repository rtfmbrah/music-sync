# Download one acquisition into bounded staging

- Status: Completed
- Started: 2026-08-24
- Completed: 2026-08-24
- Roadmap: P2 acquisition and identity

## Goal

Execute one claimed YouTube acquisition through the pinned yt-dlp boundary while
keeping every result inside durable job-specific staging.

## Scope

- Extend the yt-dlp adapter with single-item media download behavior.
- Enforce a non-zero deadline and bounded stdout/stderr capture.
- Use a fixed staging output template and disable playlist expansion/overwrites.
- Accept only one non-empty regular media file contained directly in staging.
- Return typed provider, process, output, and filesystem failures.
- Add deterministic executable fixtures for success, provider failure, malformed
  output, path escape, empty media, and timeout.
- Do not probe, hash, or commit media to the managed library in this slice.

## Safety boundary

- Existing media is never passed as an output target and cannot be overwritten.
- Partial downloads never become library artifacts.
- Provider or infrastructure failure leaves the durable job retryable and cannot
  trigger replacement search.

## Work completed

- Reused the bounded yt-dlp process runner for enumeration and single-item download.
- Added fixed-template, no-playlist, no-overwrite acquisition into canonical staging.
- Added strict validation for exactly one absolute, directly contained, non-empty
  regular file plus a typed staged-media result.
- Kept every accepted download outside the managed library.

## Validation evidence

- `just check`: passed with warnings-denied Clippy, 65 deterministic tests,
  architecture enforcement, and deployment-script syntax validation.
- Executable fixtures cover success, rate limit, missing/relative output, path
  escape, empty output file, and timeout without network access.
