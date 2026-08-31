//! Provenance-aware provider display metadata extraction.

use serde_json::Value;

/// Display metadata retained separately from canonical recording identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDisplayMetadata {
    /// Provider title.
    pub title: String,
    /// Explicit artist or provenance-labelled uploader fallback.
    pub artist: Option<String>,
    /// Field that supplied `artist`.
    pub artist_provenance: Option<&'static str>,
    /// Explicit provider album, never a source playlist name.
    pub album: Option<String>,
    /// Explicit release/upload date when supplied.
    pub release_date: Option<String>,
    /// Explicit non-generic provider categories.
    pub genres: Vec<String>,
    /// Best HTTPS thumbnail URL.
    pub thumbnail_url: Option<String>,
}

impl ProviderDisplayMetadata {
    /// Extracts conservative display fields from one complete or flat yt-dlp object.
    #[must_use]
    pub fn from_ytdlp(value: &Value) -> Option<Self> {
        let title = text(value, "track").or_else(|| text(value, "title"))?;
        let (artist, artist_provenance) = [
            ("artist", "artist"),
            ("creator", "creator"),
            ("channel", "channel"),
            ("uploader", "uploader"),
        ]
        .into_iter()
        .find_map(|(field, provenance)| text(value, field).map(|v| (v, provenance)))
        .map_or((None, None), |(artist, provenance)| {
            (Some(artist), Some(provenance))
        });
        let mut genres = Vec::new();
        if let Some(genre) = text(value, "genre") {
            push_genre(&mut genres, genre);
        }
        if let Some(categories) = value.get("categories").and_then(Value::as_array) {
            for category in categories.iter().filter_map(Value::as_str) {
                push_genre(&mut genres, category.to_owned());
            }
        }
        let thumbnail_url = value
            .get("thumbnails")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|thumbnail| thumbnail.get("url").and_then(Value::as_str))
            .rfind(|url| url.starts_with("https://"))
            .map(str::to_owned)
            .or_else(|| text(value, "thumbnail").filter(|url| url.starts_with("https://")));
        Some(Self {
            title,
            artist,
            artist_provenance,
            album: text(value, "album"),
            release_date: text(value, "release_date")
                .or_else(|| text(value, "release_year"))
                .or_else(|| text(value, "upload_date")),
            genres,
            thumbnail_url,
        })
    }

    /// Whether the artist is strong enough for an exact lyrics lookup.
    #[must_use]
    pub fn lyrics_artist_is_explicit(&self) -> bool {
        matches!(self.artist_provenance, Some("artist" | "creator"))
    }
}

fn text(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(|value| match value {
            Value::String(value) => Some(value.trim().to_owned()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        })
        .filter(|value| !value.is_empty() && value != "NA")
}

fn push_genre(genres: &mut Vec<String>, genre: String) {
    let genre = genre.trim();
    if genre.is_empty()
        || matches!(
            genre.to_ascii_lowercase().as_str(),
            "music" | "entertainment" | "people & blogs"
        )
        || genres
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(genre))
    {
        return;
    }
    genres.push(genre.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_explicit_music_fields_and_rejects_generic_categories()
    -> Result<(), Box<dyn std::error::Error>> {
        let value = serde_json::json!({
            "title": "Video title", "track": "Track title", "artist": "Artist",
            "channel": "Channel", "album": "Album", "release_year": 2026,
            "genre": "Frenchcore", "categories": ["Music", "Electronic"],
            "thumbnails": [{"url":"http://unsafe"},{"url":"https://img/large.jpg"}]
        });
        let metadata = ProviderDisplayMetadata::from_ytdlp(&value)
            .ok_or("expected complete provider display metadata")?;
        assert_eq!(metadata.title, "Track title");
        assert_eq!(metadata.artist.as_deref(), Some("Artist"));
        assert_eq!(metadata.artist_provenance, Some("artist"));
        assert_eq!(metadata.genres, ["Frenchcore", "Electronic"]);
        assert_eq!(
            metadata.thumbnail_url.as_deref(),
            Some("https://img/large.jpg")
        );
        assert!(metadata.lyrics_artist_is_explicit());
        Ok(())
    }

    #[test]
    fn labels_channel_as_display_only_fallback() -> Result<(), Box<dyn std::error::Error>> {
        let metadata = ProviderDisplayMetadata::from_ytdlp(&serde_json::json!({
            "title":"Track", "channel":"Uploader", "categories":["Music"]
        }))
        .ok_or("expected provider display metadata for channel fallback")?;
        assert_eq!(metadata.artist_provenance, Some("channel"));
        assert!(metadata.genres.is_empty());
        assert!(!metadata.lyrics_artist_is_explicit());
        Ok(())
    }
}
