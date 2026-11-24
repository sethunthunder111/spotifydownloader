//! ISRC lookup results and their CSV and JSON exports.

use std::path::Path;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::spotify::Track;

/// What was looked up: one song or a whole playlist.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Track,
    Playlist,
}

/// The result of an ISRC lookup.
pub struct Listing {
    pub kind: Kind,
    pub name: String,
    pub url: Option<String>,
    pub tracks: Vec<Track>,
    /// Playlist items left out because they are local files or unavailable.
    pub skipped: usize,
}

impl Listing {
    pub fn with_isrc(&self) -> usize {
        self.tracks.iter().filter(|t| t.isrc.is_some()).count()
    }
}

/// Columns of the CSV file, in order.
const CSV_HEADER: [&str; 9] = [
    "position",
    "isrc",
    "title",
    "artists",
    "album",
    "release_date",
    "duration_ms",
    "spotify_id",
    "spotify_url",
];

/// Spreadsheet programs run cells that start with these characters as formulas.
const FORMULA_STARTS: [char; 3] = ['=', '+', '@'];

/// The listing as CSV text. It starts with a UTF-8 byte order mark so Excel reads accented
/// names correctly.
pub fn csv(listing: &Listing) -> String {
    let mut out = String::from('\u{feff}');
    push_row(&mut out, CSV_HEADER.iter().map(|s| s.to_string()));
    for (i, t) in listing.tracks.iter().enumerate() {
        push_row(
            &mut out,
            [
                (i + 1).to_string(),
                t.isrc.clone().unwrap_or_default(),
                t.name.clone(),
                t.artists.join("; "),
                t.album.clone().unwrap_or_default(),
                t.release_date.clone().unwrap_or_default(),
                t.duration_ms.to_string(),
                t.id.clone(),
                t.url(),
            ]
            .into_iter(),
        );
    }
    out
}

fn push_row(out: &mut String, fields: impl Iterator<Item = String>) {
    let line: Vec<String> = fields.map(|f| csv_field(&f)).collect();
    out.push_str(&line.join(","));
    out.push_str("\r\n");
}

/// Quotes a field when needed (RFC 4180) and defuses spreadsheet formulas.
fn csv_field(value: &str) -> String {
    let safe = if value.starts_with(FORMULA_STARTS) {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if safe.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", safe.replace('"', "\"\""))
    } else {
        safe
    }
}

#[derive(Serialize)]
struct JsonFile<'a> {
    source: JsonSource<'a>,
    total: usize,
    with_isrc: usize,
    skipped: usize,
    tracks: Vec<JsonTrack<'a>>,
}

#[derive(Serialize)]
struct JsonSource<'a> {
    #[serde(rename = "type")]
    kind: Kind,
    name: &'a str,
    url: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonTrack<'a> {
    position: usize,
    isrc: Option<&'a str>,
    title: &'a str,
    artists: &'a [String],
    album: Option<&'a str>,
    release_date: Option<&'a str>,
    duration_ms: u64,
    spotify_id: &'a str,
    spotify_url: String,
}

/// The listing as pretty-printed JSON.
pub fn json(listing: &Listing) -> Result<String> {
    let file = JsonFile {
        source: JsonSource {
            kind: listing.kind,
            name: &listing.name,
            url: listing.url.as_deref(),
        },
        total: listing.tracks.len(),
        with_isrc: listing.with_isrc(),
        skipped: listing.skipped,
        tracks: listing
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| JsonTrack {
                position: i + 1,
                isrc: t.isrc.as_deref(),
                title: &t.name,
                artists: &t.artists,
                album: t.album.as_deref(),
                release_date: t.release_date.as_deref(),
                duration_ms: t.duration_ms,
                spotify_id: &t.id,
                spotify_url: t.url(),
            })
            .collect(),
    };
    serde_json::to_string_pretty(&file).context("cannot build the JSON export")
}

/// Writes `contents` to `path`, creating missing folders.
pub fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: &str, name: &str, isrc: Option<&str>) -> Track {
        Track {
            id: id.into(),
            name: name.into(),
            artists: vec!["Daft Punk".into(), "Romanthony".into()],
            album: Some("Discovery".into()),
            image_url: None,
            duration_ms: 320_000,
            year: Some("2001".into()),
            release_date: Some("2001-03-12".into()),
            isrc: isrc.map(str::to_string),
        }
    }

    fn listing(tracks: Vec<Track>) -> Listing {
        Listing {
            kind: Kind::Playlist,
            name: "My list".into(),
            url: Some("https://open.spotify.com/playlist/abc".into()),
            tracks,
            skipped: 2,
        }
    }

    #[test]
    fn csv_has_bom_header_and_rows() {
        let text = csv(&listing(vec![track(
            "t1",
            "One More Time",
            Some("GBDUW0000053"),
        )]));
        assert!(text.starts_with('\u{feff}'));
        let lines: Vec<&str> = text.trim_start_matches('\u{feff}').lines().collect();
        assert_eq!(
            lines[0],
            "position,isrc,title,artists,album,release_date,duration_ms,spotify_id,spotify_url"
        );
        assert_eq!(
            lines[1],
            "1,GBDUW0000053,One More Time,Daft Punk; Romanthony,Discovery,2001-03-12,320000,t1,\
             https://open.spotify.com/track/t1"
        );
    }

    #[test]
    fn csv_quotes_commas_quotes_and_newlines() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a, b"), "\"a, b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
    }

    #[test]
    fn csv_defuses_formulas_but_keeps_normal_text() {
        assert_eq!(csv_field("=1+1"), "'=1+1");
        assert_eq!(csv_field("@home"), "'@home");
        assert_eq!(csv_field("Track - Remix"), "Track - Remix");
    }

    #[test]
    fn missing_isrc_is_an_empty_csv_cell_and_null_in_json() {
        let l = listing(vec![track("t1", "x", None)]);
        assert!(csv(&l).contains("1,,x,"));
        let v: serde_json::Value = serde_json::from_str(&json(&l).unwrap()).unwrap();
        assert!(v["tracks"][0]["isrc"].is_null());
    }

    #[test]
    fn json_has_source_counts_and_tracks() {
        let l = listing(vec![
            track("t1", "A", Some("AAA111111111")),
            track("t2", "B", None),
        ]);
        let v: serde_json::Value = serde_json::from_str(&json(&l).unwrap()).unwrap();
        assert_eq!(v["source"]["type"], "playlist");
        assert_eq!(v["source"]["name"], "My list");
        assert_eq!(
            (v["total"].as_u64(), v["with_isrc"].as_u64()),
            (Some(2), Some(1))
        );
        assert_eq!(v["skipped"], 2);
        assert_eq!(v["tracks"][0]["position"], 1);
        assert_eq!(v["tracks"][0]["isrc"], "AAA111111111");
        assert_eq!(v["tracks"][0]["artists"][1], "Romanthony");
        assert_eq!(
            v["tracks"][1]["spotify_url"],
            "https://open.spotify.com/track/t2"
        );
    }

    #[test]
    fn writes_files_and_creates_folders() {
        let dir = std::env::temp_dir().join(format!("spotify-dl-isrc-{}", std::process::id()));
        let file = dir.join("nested").join("out.csv");
        write_file(&file, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello");
        let _ = std::fs::remove_dir_all(dir);
    }
}
