//! YouTube search and audio extraction through the `yt-dlp` command (which needs `ffmpeg`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::process::Command;

/// Allowed difference between the Spotify and YouTube durations, in seconds.
const DURATION_TOLERANCE_SECS: f64 = 30.0;
const SEARCH_RESULTS: u32 = 5;

#[derive(Debug, Clone)]
pub struct VideoMatch {
    pub id: String,
    pub title: String,
}

impl VideoMatch {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

#[derive(Deserialize)]
struct SearchOutput {
    #[serde(default)]
    entries: Vec<SearchEntry>,
}

#[derive(Deserialize)]
struct SearchEntry {
    id: String,
    title: Option<String>,
    duration: Option<f64>,
}

/// Locates a bundled tool next to the executable first, then falls back to PATH.
pub fn tool_path(name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let candidate = dir.join(&file);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(file)
}

/// Searches YouTube for `query` and returns the first result whose duration is close to
/// `expected_ms`. If the expected duration is unknown, the top result is used.
pub async fn find_match(query: &str, expected_ms: u64) -> Result<Option<VideoMatch>> {
    let output = run(
        "yt-dlp",
        &[
            "--flat-playlist",
            "--dump-single-json",
            "--no-warnings",
            "--skip-download",
            &format!("ytsearch{SEARCH_RESULTS}:{query}"),
        ],
    )
    .await?;
    let parsed: SearchOutput =
        serde_json::from_slice(&output).context("could not read yt-dlp search results")?;

    let expected_secs = expected_ms as f64 / 1000.0;
    let found = parsed.entries.into_iter().find(|e| {
        expected_ms == 0
            || e.duration
                .is_some_and(|d| (d - expected_secs).abs() <= DURATION_TOLERANCE_SECS)
    });
    Ok(found.map(|e| VideoMatch {
        title: e.title.unwrap_or_default(),
        id: e.id,
    }))
}

/// Downloads a video's audio as `<dir>/<id>.mp3` and returns that path.
pub async fn download_mp3(video: &VideoMatch, dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let template = dir.join("%(id)s.%(ext)s").to_string_lossy().into_owned();
    let url = video.url();
    let mut args = vec![
        "--no-playlist",
        "--no-warnings",
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--output",
        template.as_str(),
    ];
    // yt-dlp only looks in its own folder and PATH. If ffmpeg sits next to spotify-dl.exe,
    // tell yt-dlp where it is.
    let ffmpeg = tool_path("ffmpeg");
    let ffmpeg = ffmpeg.to_string_lossy().into_owned();
    if Path::new(&ffmpeg).is_absolute() {
        args.extend(["--ffmpeg-location", ffmpeg.as_str()]);
    }
    args.push(url.as_str());
    run("yt-dlp", &args).await?;

    let mp3 = dir.join(format!("{}.mp3", video.id));
    if !mp3.exists() {
        bail!("yt-dlp finished but {} was not created", mp3.display());
    }
    Ok(mp3)
}

async fn run(tool: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new(tool_path(tool))
        .args(args)
        // If the job is cancelled (for example the user quits the TUI), stop the child too.
        .kill_on_drop(true)
        .output()
        .await
        .with_context(|| {
            format!(
                "could not run {tool}. Install yt-dlp and ffmpeg and make sure they are on PATH \
                 (or place yt-dlp.exe and ffmpeg.exe next to spotify-dl.exe)"
            )
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        bail!("{tool} failed: {last}");
    }
    Ok(output.stdout)
}

/// Checks that yt-dlp and ffmpeg can be started, so the user gets one clear message up front.
pub async fn check_tools() -> Result<()> {
    // yt-dlp wants `--version`; ffmpeg wants `-version`.
    for (tool, flag) in [("yt-dlp", "--version"), ("ffmpeg", "-version")] {
        Command::new(tool_path(tool))
            .arg(flag)
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("{tool} was not found. Install it and add it to PATH."))?;
    }
    Ok(())
}
