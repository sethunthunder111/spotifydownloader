//! Parsing of user-supplied Spotify links, URIs and bare IDs.

use anyhow::{Result, bail};
use reqwest::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Track,
    Album,
    Playlist,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Track => "track",
            Kind::Album => "album",
            Kind::Playlist => "playlist",
        }
    }
}

fn is_spotify_id(s: &str) -> bool {
    s.len() == 22 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Extracts a Spotify ID of the expected kind from a URL, `spotify:` URI or bare ID.
pub fn parse_id(input: &str, kind: Kind) -> Result<String> {
    let input = input.trim();
    if input.is_empty() {
        bail!("empty input");
    }

    if let Some(rest) = input.strip_prefix("spotify:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() == 2 && parts[0] == kind.as_str() && is_spotify_id(parts[1]) {
            return Ok(parts[1].to_string());
        }
        bail!("expected a Spotify {} URI, got \"{input}\"", kind.as_str());
    }

    if input.contains("spotify.com/") {
        let with_scheme = if input.contains("://") {
            input.to_string()
        } else {
            format!("https://{input}")
        };
        let url = Url::parse(&with_scheme)
            .map_err(|e| anyhow::anyhow!("invalid URL \"{input}\": {e}"))?;
        let segments: Vec<&str> = url
            .path_segments()
            .map(|s| s.filter(|p| !p.is_empty()).collect())
            .unwrap_or_default();
        // Handles locale prefixes such as /intl-de/track/<id>.
        for window in segments.windows(2) {
            if window[0] == kind.as_str() && is_spotify_id(window[1]) {
                return Ok(window[1].to_string());
            }
        }
        bail!("expected a Spotify {} link, got \"{input}\"", kind.as_str());
    }

    if is_spotify_id(input) {
        return Ok(input.to_string());
    }

    bail!(
        "could not parse \"{input}\" as a Spotify {} URL, URI or ID",
        kind.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "4uLU6hMCjMI75M1A2tKUQC";

    #[test]
    fn parses_bare_id() {
        assert_eq!(parse_id(ID, Kind::Track).unwrap(), ID);
    }

    #[test]
    fn parses_track_url_with_query() {
        let url = format!("https://open.spotify.com/track/{ID}?si=abc123");
        assert_eq!(parse_id(&url, Kind::Track).unwrap(), ID);
    }

    #[test]
    fn parses_url_without_scheme_and_with_locale() {
        let url = format!("open.spotify.com/intl-de/album/{ID}");
        assert_eq!(parse_id(&url, Kind::Album).unwrap(), ID);
    }

    #[test]
    fn parses_uri() {
        let uri = format!("spotify:playlist:{ID}");
        assert_eq!(parse_id(&uri, Kind::Playlist).unwrap(), ID);
    }

    #[test]
    fn rejects_wrong_kind() {
        let url = format!("https://open.spotify.com/album/{ID}");
        assert!(parse_id(&url, Kind::Track).is_err());
        assert!(parse_id("spotify:album:", Kind::Album).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_id("hello world", Kind::Track).is_err());
        assert!(parse_id("", Kind::Track).is_err());
    }
}
