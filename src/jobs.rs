//! Download jobs shared by the command-line subcommands and the terminal UI.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::mpsc::UnboundedSender;

use crate::input::{Kind, parse_id};
use crate::spotify::{SpotifyClient, Track};
use crate::youtube::Step;
use crate::{tags, youtube};

/// A message from a running job.
pub enum Event {
    /// A permanent line for the log.
    Log(String),
    /// What the job is doing right now. Replaces the previous progress text; empty clears it.
    Progress(String),
}

/// Where job output goes: stdout for the CLI, a channel for the TUI.
#[derive(Clone)]
pub enum Sink {
    Stdout,
    Channel(UnboundedSender<Event>),
}

/// Width of the CLI progress line, so a shorter update fully covers a longer one.
const PROGRESS_WIDTH: usize = 79;

impl Sink {
    pub fn log(&self, message: impl Into<String>) {
        let message = message.into();
        match self {
            Sink::Stdout => {
                clear_progress_line();
                println!("{message}");
            }
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Log(message));
            }
        }
    }

    pub fn progress(&self, message: impl Into<String>) {
        let message = message.into();
        match self {
            Sink::Stdout => {
                let mut out = std::io::stdout();
                if out.is_terminal() {
                    let text: String = message.chars().take(PROGRESS_WIDTH).collect();
                    let _ = write!(out, "\r{text:<PROGRESS_WIDTH$}");
                    let _ = out.flush();
                }
            }
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Progress(message));
            }
        }
    }
}

fn clear_progress_line() {
    let mut out = std::io::stdout();
    if out.is_terminal() {
        let _ = write!(out, "\r{:PROGRESS_WIDTH$}\r", "");
    }
}

/// Everything a job needs that does not change while the program runs.
#[derive(Clone)]
pub struct Settings {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub download_dir: PathBuf,
}

#[derive(Clone, Debug)]
pub enum Job {
    Song(String),
    Album(String),
    Playlist(String),
    Search(String),
    File(PathBuf),
    Login,
}

pub async fn run_job(job: Job, settings: &Settings, sink: &Sink) -> Result<()> {
    let result = run_job_inner(job, settings, sink).await;
    sink.progress("");
    result
}

async fn run_job_inner(job: Job, settings: &Settings, sink: &Sink) -> Result<()> {
    let mut spotify = SpotifyClient::new(
        settings.client_id.clone(),
        settings.client_secret.clone(),
        settings.redirect_uri.clone(),
        sink.clone(),
    )?;
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()?;

    if let Job::Login = job {
        spotify.login().await?;
        sink.log("Spotify login saved. Playlist downloads will use it from now on.");
        return Ok(());
    }

    sink.progress("Checking yt-dlp and ffmpeg...");
    youtube::check_tools().await?;
    std::fs::create_dir_all(&settings.download_dir)
        .with_context(|| format!("cannot create {}", settings.download_dir.display()))?;
    let dir = settings.download_dir.as_path();

    match job {
        Job::Song(input) => {
            let id = parse_id(&input, Kind::Track)?;
            sink.progress("Looking up the track on Spotify...");
            let track = spotify.track(&id).await?;
            sink.log(format!(
                "Track: {} ({})",
                track.display_name(),
                minutes(track.duration_ms / 1000)
            ));
            download_one(&http, sink, dir, &track).await?;
        }
        Job::Album(input) => {
            let id = parse_id(&input, Kind::Album)?;
            sink.progress("Loading the album from Spotify...");
            let album = spotify.album(&id).await?;
            sink.log(format!(
                "Album: {} ({} tracks)",
                album.name,
                album.tracks.len()
            ));
            download_all(&http, sink, dir, &album.tracks).await;
        }
        Job::Playlist(input) => {
            let id = parse_id(&input, Kind::Playlist)?;
            sink.progress("Loading the playlist from Spotify...");
            let tracks = spotify.playlist(&id).await?;
            sink.log(format!("Playlist: {} playable tracks", tracks.len()));
            download_all(&http, sink, dir, &tracks).await;
        }
        Job::Search(query) => {
            sink.progress(format!("Searching Spotify for \"{query}\"..."));
            let track = spotify
                .search_track(&query)
                .await?
                .ok_or_else(|| anyhow!("no Spotify track matched \"{query}\""))?;
            sink.log(format!(
                "Best match: {} ({})",
                track.display_name(),
                minutes(track.duration_ms / 1000)
            ));
            download_one(&http, sink, dir, &track).await?;
        }
        Job::File(path) => {
            let text = tokio::fs::read_to_string(&path)
                .await
                .with_context(|| format!("cannot read {}", path.display()))?;
            let entries: Vec<&str> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .collect();
            let mut tracks = Vec::new();
            for (i, line) in entries.iter().enumerate() {
                sink.progress(format!(
                    "Looking up {} of {} on Spotify...",
                    i + 1,
                    entries.len()
                ));
                match parse_id(line, Kind::Track) {
                    Ok(id) => match spotify.track(&id).await {
                        Ok(track) => tracks.push(track),
                        Err(e) => sink.log(format!("[skip] {line}: {e:#}")),
                    },
                    Err(e) => sink.log(format!("[skip] {e:#}")),
                }
            }
            if tracks.is_empty() {
                bail!("no valid track entries in {}", path.display());
            }
            sink.log(format!("File: {} tracks", tracks.len()));
            download_all(&http, sink, dir, &tracks).await;
        }
        Job::Login => unreachable!("handled above"),
    }
    Ok(())
}

async fn download_all(http: &reqwest::Client, sink: &Sink, dir: &Path, tracks: &[Track]) {
    let total = tracks.len();
    let mut ok = 0;
    for (i, track) in tracks.iter().enumerate() {
        sink.log(format!("[{}/{}] {}", i + 1, total, track.display_name()));
        match download_one(http, sink, dir, track).await {
            Ok(_) => ok += 1,
            Err(e) => sink.log(format!("  [fail] {e:#}")),
        }
    }
    sink.log(format!(
        "Finished: {ok}/{total} tracks saved to {}",
        dir.display()
    ));
}

/// Finds the track on YouTube, downloads it as MP3, moves it into `dir` and tags it.
pub async fn download_one(
    http: &reqwest::Client,
    sink: &Sink,
    dir: &Path,
    track: &Track,
) -> Result<PathBuf> {
    let name = track.display_name();
    let query = name.replace(['\'', '"', ',', '.', '/'], "");
    sink.progress(format!("Searching YouTube for {name}..."));
    let video = youtube::find_match(&query, track.duration_ms)
        .await?
        .ok_or_else(|| anyhow!("no YouTube match for \"{name}\" with a similar duration"))?;
    match video.duration_secs {
        Some(secs) => sink.log(format!(
            "  found: {} ({})",
            video.title,
            minutes(secs.round() as u64)
        )),
        None => sink.log(format!("  found: {}", video.title)),
    }

    let temp_dir = std::env::temp_dir().join("spotify-dl").join(&track.id);
    let result = async {
        sink.progress(format!("Starting download of {name}..."));
        let mp3 = youtube::download_mp3(&video, &temp_dir, |step| match step {
            Step::Downloading { percent, eta } => sink.progress(match eta {
                Some(eta) => format!("Downloading {name}: {percent}, {eta} left"),
                None => format!("Downloading {name}: {percent}"),
            }),
            Step::Converting => sink.progress(format!("Converting {name} to MP3...")),
        })
        .await?;
        let target = dir.join(sanitize_filename(&format!("{name}.mp3")));
        move_file(&mp3, &target).await?;
        sink.progress(format!("Writing tags and cover art for {name}..."));
        if let Err(e) = tags::write(&target, track, http, sink).await {
            sink.log(format!("  warning: saved but could not write tags: {e:#}"));
        }
        Ok::<PathBuf, anyhow::Error>(target)
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;

    let target = result?;
    sink.log(format!("  saved: {}", target.display()));
    Ok(target)
}

/// Formats seconds as `m:ss`.
fn minutes(secs: u64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

async fn move_file(from: &Path, to: &Path) -> Result<()> {
    if tokio::fs::rename(from, to).await.is_ok() {
        return Ok(());
    }
    // Rename fails across drives (temp dir vs. download dir); copy instead.
    tokio::fs::copy(from, to)
        .await
        .with_context(|| format!("cannot write {}", to.display()))?;
    tokio::fs::remove_file(from).await?;
    Ok(())
}

/// Makes a string safe to use as a Windows file name.
pub fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim_end_matches(['.', ' ']).trim();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{minutes, sanitize_filename};

    #[test]
    fn replaces_reserved_characters() {
        assert_eq!(sanitize_filename("AC/DC: Back?.mp3"), "AC_DC_ Back_.mp3");
        assert_eq!(sanitize_filename("name. "), "name");
        assert_eq!(sanitize_filename("..."), "untitled");
    }

    #[test]
    fn formats_minutes() {
        assert_eq!(minutes(0), "0:00");
        assert_eq!(minutes(320), "5:20");
    }
}
