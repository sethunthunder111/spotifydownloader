//! Writes ID3v2.4 metadata and cover art into the downloaded MP3.

use std::path::Path;

use anyhow::{Context, Result};
use id3::frame::{Picture, PictureType};
use id3::{Tag, TagLike, Timestamp, Version};

use crate::spotify::Track;

/// Writes the title, artists, album, year and (if given) front cover into the MP3.
pub fn write(path: &Path, track: &Track, cover: Option<&[u8]>) -> Result<()> {
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
    if let Some(data) = cover {
        tag.add_frame(Picture {
            mime_type: image_mime(data).to_string(),
            picture_type: PictureType::CoverFront,
            description: "Cover".to_string(),
            data: data.to_vec(),
        });
    }

    tag.write_to_path(path, Version::Id3v24)
        .with_context(|| format!("cannot write tags to {}", path.display()))
}

/// The MIME type of an image, read from its first bytes. Spotify serves JPEG.
pub fn image_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// File extension matching [`image_mime`].
pub fn image_extension(data: &[u8]) -> &'static str {
    if image_mime(data) == "image/png" {
        "png"
    } else {
        "jpg"
    }
}

#[cfg(test)]
mod tests {
    use super::{image_extension, image_mime};

    #[test]
    fn detects_image_type_from_magic_bytes() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n...."), "image/png");
        assert_eq!(image_extension(b"\x89PNG\r\n\x1a\n...."), "png");
        assert_eq!(image_mime(b"\xff\xd8\xff\xe0...."), "image/jpeg");
        assert_eq!(image_extension(b""), "jpg");
    }
}
