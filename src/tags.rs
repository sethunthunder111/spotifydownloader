//! Writes ID3v2.4 metadata and cover art into the downloaded MP3.

use std::path::Path;

use anyhow::{Context, Result};
use id3::frame::{Picture, PictureType};
use id3::{Tag, TagLike, Timestamp, Version};

use crate::jobs::Sink;
use crate::spotify::Track;

pub async fn write(path: &Path, track: &Track, http: &reqwest::Client, sink: &Sink) -> Result<()> {
    let mut tag = Tag::new();
    tag.set_title(track.name.as_str());
    if !track.artists.is_empty() {
        tag.set_artist(track.artists.join("/"));
    }
    if let Some(album) = &track.album {
        tag.set_album(album.as_str());
    }
    // ID3v2.4 dropped the `TYER` frame that `set_year` writes; the year lives in `TDRC`.
    if let Some(year) = track.year.as_deref().and_then(|y| y.parse::<i32>().ok()) {
        tag.set_date_recorded(Timestamp {
            year,
            month: None,
            day: None,
            hour: None,
            minute: None,
            second: None,
        });
    }

    if let Some(url) = &track.image_url {
        match fetch_image(http, url).await {
            Ok(data) => {
                tag.add_frame(Picture {
                    mime_type: if url.ends_with(".png") {
                        "image/png"
                    } else {
                        "image/jpeg"
                    }
                    .to_string(),
                    picture_type: PictureType::CoverFront,
                    description: "Cover".to_string(),
                    data,
                });
            }
            Err(e) => sink.log(format!("  warning: could not download album art: {e:#}")),
        }
    }

    tag.write_to_path(path, Version::Id3v24)
        .with_context(|| format!("cannot write tags to {}", path.display()))
}

async fn fetch_image(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let bytes = http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    Ok(bytes.to_vec())
}
