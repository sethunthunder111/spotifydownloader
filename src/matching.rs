//! Decides whether a YouTube result is the same recording as a Spotify track.

use std::collections::HashSet;

/// What we know about the Spotify track.
pub struct Query<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub album: Option<&'a str>,
    pub duration_secs: Option<f64>,
}

/// What we know about a YouTube (Music) result.
pub struct Candidate<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub album: Option<&'a str>,
    pub duration_secs: Option<f64>,
}

const WEIGHT_TITLE: f64 = 2.0;
const WEIGHT_ARTIST: f64 = 1.5;
const WEIGHT_DURATION: f64 = 1.0;
const WEIGHT_ALBUM: f64 = 0.5;
/// Lowest total score accepted as the same recording (the maximum is 5.0).
pub const MIN_SCORE: f64 = 3.0;
/// Lowest title similarity worth considering at all.
const MIN_TITLE_SIMILARITY: f64 = 0.5;

/// Words that mark a different version of a song. A result that has one is rejected unless the
/// Spotify title has it too.
const UNWANTED: &[&str] = &[
    "live",
    "karaoke",
    "instrumental",
    "cover",
    "remix",
    "slowed",
    "sped",
    "reverb",
    "nightcore",
    "acoustic",
    "8d",
    "demo",
    "tribute",
];

/// Words that do not change which recording it is.
const IGNORED: &[&str] = &[
    "feat",
    "ft",
    "featuring",
    "with",
    "remastered",
    "remaster",
    "version",
    "deluxe",
    "edition",
    "mono",
    "stereo",
    "anniversary",
];

/// Lowercase words with punctuation removed. Accented letters are kept.
fn words(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Words that identify a title: no noise words, no four-digit years.
fn title_words(text: &str) -> HashSet<String> {
    words(text)
        .into_iter()
        .filter(|w| !IGNORED.contains(&w.as_str()))
        .filter(|w| !(w.len() == 4 && w.chars().all(|c| c.is_ascii_digit())))
        .collect()
}

/// How alike two titles are, from 0 (nothing shared) to 1 (same words).
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (title_words(a), title_words(b));
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// True when two artist names refer to the same act, for example `Daft Punk` and
/// `Daft Punk - Topic`.
fn same_artist(a: &str, b: &str) -> bool {
    let (a, b) = (words(a).join(" "), words(b).join(" "));
    !a.is_empty() && !b.is_empty() && (a == b || a.contains(&b) || b.contains(&a))
}

/// Scores `cand` against `query`, or returns `None` if it is clearly a different recording.
/// `tolerance_secs` is the largest accepted difference in length.
pub fn score(query: &Query, cand: &Candidate, tolerance_secs: f64) -> Option<f64> {
    let title = title_similarity(query.title, cand.title);
    if title < MIN_TITLE_SIMILARITY {
        return None;
    }

    let query_words = title_words(query.title);
    if title_words(cand.title)
        .iter()
        .any(|w| UNWANTED.contains(&w.as_str()) && !query_words.contains(w))
    {
        return None;
    }

    let artist = if cand.artists.is_empty() || query.artists.is_empty() {
        0.5
    } else if query
        .artists
        .iter()
        .any(|q| cand.artists.iter().any(|c| same_artist(q, c)))
    {
        1.0
    } else {
        return None;
    };

    let duration = match (query.duration_secs, cand.duration_secs) {
        (Some(a), Some(b)) => {
            let diff = (a - b).abs();
            if diff > tolerance_secs {
                return None;
            }
            1.0 - diff / tolerance_secs
        }
        _ => 0.5,
    };

    let album = match (query.album, cand.album) {
        (Some(a), Some(b)) if words(a) == words(b) => 1.0,
        _ => 0.0,
    };

    Some(
        WEIGHT_TITLE * title
            + WEIGHT_ARTIST * artist
            + WEIGHT_DURATION * duration
            + WEIGHT_ALBUM * album,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artists(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn spotify<'a>(a: &'a [String]) -> Query<'a> {
        Query {
            title: "One More Time",
            artists: a,
            album: Some("Discovery"),
            duration_secs: Some(320.0),
        }
    }

    fn cand<'a>(title: &'a str, a: &'a [String], album: &'a str, secs: f64) -> Candidate<'a> {
        Candidate {
            title,
            artists: a,
            album: Some(album),
            duration_secs: Some(secs),
        }
    }

    #[test]
    fn exact_match_scores_highest() {
        let a = artists(&["Daft Punk"]);
        let best = score(
            &spotify(&a),
            &cand("One More Time", &a, "Discovery", 320.0),
            7.0,
        )
        .unwrap();
        assert!(best > 4.9, "{best}");
    }

    #[test]
    fn prefers_the_album_version_over_an_edit() {
        let a = artists(&["Daft Punk"]);
        let q = spotify(&a);
        let album = score(&q, &cand("One More Time", &a, "Discovery", 320.0), 7.0).unwrap();
        let edit = score(
            &q,
            &cand("One More Time (Radio Edit)", &a, "One More Time", 320.0),
            7.0,
        )
        .unwrap();
        assert!(album > edit);
    }

    #[test]
    fn rejects_wrong_artist_length_and_versions() {
        let a = artists(&["Daft Punk"]);
        let q = spotify(&a);
        let other = artists(&["Matway"]);
        assert!(score(&q, &cand("One More Time", &other, "x", 320.0), 7.0).is_none());
        assert!(score(&q, &cand("One More Time", &a, "Discovery", 168.0), 7.0).is_none());
        assert!(score(&q, &cand("One More Time (Live)", &a, "Alive", 320.0), 7.0).is_none());
        assert!(score(&q, &cand("One More Time - Karaoke", &a, "x", 320.0), 7.0).is_none());
        assert!(score(&q, &cand("Aerodynamic", &a, "Discovery", 320.0), 7.0).is_none());
    }

    #[test]
    fn allows_an_unwanted_word_the_spotify_title_also_has() {
        let a = artists(&["Band"]);
        let q = Query {
            title: "Song (Live)",
            artists: &a,
            album: None,
            duration_secs: Some(200.0),
        };
        assert!(score(&q, &cand("Song - Live", &a, "x", 201.0), 7.0).is_some());
    }

    #[test]
    fn topic_channels_and_feature_credits_still_match() {
        let spot = artists(&["Daft Punk", "Romanthony"]);
        let yt = artists(&["Daft Punk - Topic"]);
        assert!(
            score(
                &spotify(&spot),
                &cand("One More Time", &yt, "Discovery", 321.0),
                7.0
            )
            .is_some()
        );
    }

    #[test]
    fn ignores_remaster_noise_and_punctuation() {
        assert!(title_similarity("Africa - 2018 Remaster", "Africa") > 0.99);
        assert!(title_similarity("D.A.N.C.E.", "D A N C E") > 0.99);
        assert!(title_similarity("Song (feat. Someone)", "Song") > 0.3);
        assert!(title_similarity("abc", "xyz") < 0.01);
    }

    #[test]
    fn unknown_duration_or_artist_is_neutral() {
        let a = artists(&["Daft Punk"]);
        let none: Vec<String> = Vec::new();
        let c = Candidate {
            title: "One More Time",
            artists: &none,
            album: None,
            duration_secs: None,
        };
        let s = score(&spotify(&a), &c, 7.0).unwrap();
        assert!(s >= MIN_SCORE, "{s}");
    }
}
