# Provider boundaries

## CURRENT

`music-sync source enumerate <url>` invokes a narrow YouTube `yt-dlp` boundary with
an explicit executable, non-zero deadline, 16 MiB stdout/stderr limits, and no media
download. It parses single-video and flat-playlist JSON into provider-neutral source
snapshots while retaining raw item metadata. Provider item IDs remain separate from
recording identity.

Failures expose conservative domain classes: explicit private/deleted/unavailable
items are permanent; rate limits, authentication requirements, and timeouts are
separate; malformed output is extraction failure; unknown adapter/infrastructure
failures remain transient. Stored video, playlist, private, rate-limit, malformed,
and timeout fixtures cover the boundary without live network access.

`source add`, `source list`, and `source remove` persist configuration in
music-sync-owned SQLite. The first provider policy accepts only HTTPS YouTube and
youtu.be URLs. Remove means deactivate, never delete; `list --all` includes inactive
rows, and adding the same URL reactivates its stable source ID. `source reconcile`
loads the configured URL, enumerates it, and only then passes the successful snapshot
to transactional persistence. Adapter errors leave durable membership unchanged.

The same bounded yt-dlp boundary supports single-item acquisition into an explicit
job staging directory. Download output paths are treated as untrusted adapter output:
relative, multiple, missing, empty, non-regular, and staging-escaping results are
rejected before any artifact can be committed.

Timer-driven enumeration and acquisition apply configured yt-dlp pacing inside the
adapter. Extraction requests receive a fixed inter-request delay, while media
downloads receive bounded randomized sleep. The service defaults to one second
between requests and five to fifteen seconds before downloads. Explicit operator
commands remain immediate; pacing policy belongs to autonomous operation.

Authenticated YouTube access explicitly selects the `default,web_embedded` client
set to avoid the demonstrated logged-in `tv_downgraded` reload failure. yt-dlp's
cache is rooted below application state because the dedicated service account has no
home directory. Both choices remain adapter details and are fixture-tested as exact
subprocess arguments.

## PLANNED

Expand classification with demonstrated geo/cookie/PO-token fixtures.
YouTube-specific flags/parsing must remain inside the adapter.

Ordinary tests execute adapters against controlled subprocess fixtures for videos,
playlists, deleted/private/geo-blocked output, rate limits, auth/token requirements,
and malformed output. Live providers are explicit opt-in checks. Future providers
evolve the boundary from demonstrated capabilities rather than speculative traits.
