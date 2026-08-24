# Product vision

music-sync exists so a user can configure music sources and Navidrome paths, run a
deterministic command from external automation, open a Navidrome-compatible player,
press shuffle, and receive a library that continuously grows and improves without
routine review.

It is not a download-wrapper product. Acquisition is only one stage in an autonomous
library manager that preserves media, resolves canonical musical identity, enriches
metadata/artwork/lyrics, explains its work, and safely recovers from provider loss.

Human input is reserved for configuration and exceptional repair. Recommendations
may experiment with taste, but downloaded audio must still have extremely high
identity confidence. False-negative identity decisions are preferable to silently
adding the wrong song.

The project is a clean-room rewrite based on repository requirements only. Migration
may inspect generic media assets, tags, playlists, filesystem layout, and relevant
Navidrome paths, but never legacy application source or internal behavior.

