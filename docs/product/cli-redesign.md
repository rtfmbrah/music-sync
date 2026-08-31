# CLI redesign worksheet

Status: accepted design; implementation tracked by execution plan 0034.

Use this file to decide the public command structure before implementation. Rename,
move, add, or delete lines directly. Commands under `admin` are intentionally still
available for diagnostics and recovery, but removed from the normal operator path.

## Current CLI

```text
music-sync
├── doctor
├── status
├── source
│   ├── add <url>
│   ├── list
│   ├── remove <id>
│   ├── reconcile <id>
│   └── enumerate <url>
├── sync
│   └── run
├── service
│   └── run
├── library
│   ├── adopt <path>
│   ├── health
│   ├── fingerprint
│   ├── retry-fingerprint <artifact-id>
│   ├── verify-provider-links
│   └── quarantine-unverified-provider-links
├── acquisition
│   ├── history
│   ├── retry <job-id>
│   ├── recover-running
│   ├── run-one
│   └── run-pending
├── playlist
│   └── materialize
├── metadata
│   ├── resolve
│   ├── materialize
│   └── retry-materialize <recording-id>
├── artwork
│   └── fetch
├── lyrics
│   └── fetch
├── discovery
│   ├── run
│   └── route
├── repair
│   ├── assess
│   ├── generate
│   ├── run-one
│   ├── commit <attempt-id>
│   ├── retry <attempt-id>
│   └── recover-running
├── runs
│   ├── history
│   ├── show <run-id>
│   └── recover-interrupted
├── events
└── maintenance
    └── backup
```

Current global options:

```text
-v, --verbose
--json
--no-progress
```

Most commands currently repeat `--config`, while low-level source commands repeat
`--database`.

## Editable simplified target

The intended normal workflow is: configure sources, run or observe synchronization,
inspect actionable problems, and request a safe report. The service uses the same
orchestrator as `run`; subsystem phase commands remain under `admin`.

```text
music-sync [--json] [-v]
├── status
│   ├── [default]                  # concise current state and latest run
│   ├── watch                      # follow durable progress/events
│   └── history                    # recent synchronization runs
├── sync                           # one complete foreground synchronization
├── list [--full] [STATUS] [--source <SOURCE>]
│                                  # sources; optionally filter member tracks
├── add <url> [--name <name>]
├── remove <id>                    # disable source; never delete audio
├── duplicates                     # read-only evidence report
├── doctor
└── backup [--directory <path>]
```

Commands that use application state accept `--config <path>` and default to
`/etc/music-sync/music-sync.toml`, so the installed LXC CLI needs no repeated config
argument. Compatibility commands retain their prior config/database arguments.

`list` prints configured playlist and single-track sources. `list --full` prints a
tree in provider order and includes every playlist member:

```text
1. playlist Daily  youtube-id=PL...
   ├── success    abc123       Artist - Title
   ├── pending    def456       Artist - Title
   ├── failed     ghi789       Artist - Title
   ├── missing    jkl012       Artist - Title
   └── copyright  mno345       Artist - Title
2. track Standalone title  youtube-id=pqr678
   └── success    pqr678       Artist - Title
```

Status flags `--success`, `--pending`, `--failed`, `--missing`, and `--copyright`
imply the detailed tree and may be combined with OR semantics. `--source <SOURCE>`
restricts the tree to an exact source ID, case-insensitive source name, provider
playlist ID, or single-video ID. It also implies the detailed tree. For example:

```text
music-sync list --failed --missing
music-sync list --success --source winstreak
```

Status precedence and meaning are strict:

1. `success`: a healthy preferred local artifact exists. This wins even if the
   provider object was later removed, made private, or copyright-blocked.
2. `pending`: no healthy artifact exists and acquisition has not reached a terminal
   or deferred failure. Running work is displayed as `pending` with an additional
   progress annotation.
3. `copyright`: no healthy artifact exists and the provider returned a persistently
   recorded, specifically classified copyright restriction.
4. `missing`: no healthy artifact exists and the object was removed from its source
   or is definitively unavailable for a non-copyright reason.
5. `failed`: no healthy artifact exists and acquisition is deferred or failed for a
   transient, authentication, processing, permission, disk, or unclassified provider
   error. The stored diagnostic is shown on the following indented line.

The display uses canonical artist and title only when they are supported by stored
metadata evidence. It otherwise shows the provider title unchanged; it never guesses
artist identity by splitting a YouTube title string.

## Duplicate-report safety contract

The first implementation is report-only:

```text
exact bytes
  SHA-256 equal -> definitely byte-identical, but retain every file

same provider object
  provider plus provider-item ID equal -> reuse one preferred artifact

audio candidate
  compatible duration plus Chromaprint similarity -> review candidate only

text candidate
  similar artist/title -> weak hint only, never identity
```

`duplicates` does not delete, move, retag, overwrite, or merge audio. It reads
persisted evidence and clearly labels groups with insufficient evidence. A later
cleanup workflow requires a separate design and explicit operator confirmation.

## Decisions to edit

- Should `music-sync` with no subcommand display status, help, or run a sync?
- Should `status watch` read the journal, durable database events, or both?
- Should `source remove` be renamed to `source disable` to emphasize preservation?
- Should advanced commands remain public under `admin`, or be hidden from help?
- Should `duplicates` calculate missing fingerprints automatically, or only compare
  evidence already stored in the database?
- Which old command paths must remain as deprecated compatibility aliases?
