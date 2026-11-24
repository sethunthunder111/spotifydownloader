//! Finds a track on YouTube Music (falling back to YouTube) and downloads it as MP3 through the
//! `yt-dlp` command, which needs `ffmpeg`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::matching::{self, Candidate, MIN_SCORE, Query};

/// Allowed difference between the Spotify and YouTube Music lengths, in seconds. Song uploads
/// are the album audio, so they match to within a second or two.
const MUSIC_TOLERANCE_SECS: f64 = 7.0;
/// Allowed difference for the plain YouTube fallback, where uploads often have intros.
const YOUTUBE_TOLERANCE_SECS: f64 = 30.0;
/// Search results whose metadata is fetched and scored.
const MUSIC_CANDIDATES: usize = 4;
const YOUTUBE_RESULTS: u32 = 5;
/// Prefix for the progress lines we ask yt-dlp to print, so they are easy to pick out.
const PROGRESS_TAG: &str = "spotify-dl-progress";
/// Network options for every yt-dlp call. On some networks YouTube over IPv6 accepts the
/// connection and then never answers, so yt-dlp waits forever. IPv4 avoids that.
const NETWORK_ARGS: [&str; 3] = ["--force-ipv4", "--socket-timeout", "20"];
/// Longest time a search may take before it counts as stuck.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(90);
/// Longest time a download may go without printing progress before it counts as stuck.
/// Generous, because converting a long track prints nothing until it finishes.
const STALL_TIMEOUT: Duration = Duration::from_secs(300);
/// Fewest milliseconds between progress reports for one download.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(200);

/// Caps how many yt-dlp metadata lookups run at once across all parallel downloads.
static LOOKUPS: Semaphore = Semaphore::const_new(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    YouTubeMusic,
    YouTube,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::YouTubeMusic => "YouTube Music",
            Source::YouTube => "YouTube",
        }
    }
}

/// What to look for, taken from the Spotify track.
pub struct TrackQuery<'a> {
    pub title: &'a str,
    pub artists: &'a [String],
    pub album: Option<&'a str>,
    pub duration_ms: u64,
}

impl TrackQuery<'_> {
    fn duration_secs(&self) -> Option<f64> {
        (self.duration_ms > 0).then(|| self.duration_ms as f64 / 1000.0)
    }

    fn as_query(&self) -> Query<'_> {
        Query {
            title: self.title,
            artists: self.artists,
            album: self.album,
            duration_secs: self.duration_secs(),
        }
    }
}

/// The video chosen for a track.
#[derive(Debug, Clone)]
pub struct Match {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
    pub source: Source,
    /// 0 to 5 for YouTube Music matches; 0 for the plain YouTube fallback.
    pub score: f64,
    /// Full yt-dlp metadata, kept so the download does not have to look the video up again.
    info_json: Option<Arc<String>>,
}

impl Match {
    pub fn url(&self) -> String {
        match self.source {
            Source::YouTubeMusic => format!("https://music.youtube.com/watch?v={}", self.id),
            Source::YouTube => format!("https://www.youtube.com/watch?v={}", self.id),
        }
    }
}

/// Download progress of the audio stream.
#[derive(Debug, Clone, Copy, Default)]
pub struct Transfer {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Bytes per second.
    pub speed: Option<f64>,
    pub eta_secs: Option<u64>,
}

/// The audio stream yt-dlp picked, before conversion.
#[derive(Debug, Clone, Default)]
pub struct SourceFormat {
    pub container: String,
    pub codec: String,
    pub bitrate_kbps: Option<f64>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
}

/// What yt-dlp is doing while it fetches a track.
pub enum Step {
    Downloading(Transfer, SourceFormat),
    Converting,
}

/// Facts about the finished MP3, read with `ffprobe`.
#[derive(Debug, Clone, Default)]
pub struct Mp3Info {
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub duration_secs: Option<f64>,
    pub size_bytes: u64,
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

/// The parts of a video's metadata we use.
#[derive(Deserialize)]
struct Info {
    id: String,
    title: Option<String>,
    track: Option<String>,
    artists: Option<Vec<String>>,
    artist: Option<String>,
    uploader: Option<String>,
    album: Option<String>,
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

/// Builds a command for a helper tool with safe defaults.
fn command(tool: &str) -> Command {
    let mut cmd = Command::new(tool_path(tool));
    cmd
        // The TUI owns the console. Children must not read key presses from it.
        .stdin(Stdio::null())
        // If the job is cancelled (for example the user quits the TUI), stop the child too.
        .kill_on_drop(true)
        // yt-dlp is a Python program. Without this, Windows pipes use the legacy code page
        // and titles with non-ASCII characters arrive garbled.
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    cmd
}

fn not_found(tool: &str) -> String {
    format!(
        "could not run {tool}. Install yt-dlp and ffmpeg and make sure they are on PATH \
         (or place yt-dlp.exe and ffmpeg.exe next to spotify-dl.exe)"
    )
}

fn search_timeout() -> anyhow::Error {
    anyhow::anyhow!(
        "YouTube did not answer within {} seconds. Check your connection and run \
         `yt-dlp -U` to update yt-dlp.",
        SEARCH_TIMEOUT.as_secs()
    )
}

// ---------- Searching ----------

/// Finds the best recording of the track: YouTube Music first, plain YouTube as a fallback.
pub async fn find_match(track: &TrackQuery<'_>) -> Result<Option<Match>> {
    if let Some(found) = find_on_music(track).await? {
        return Ok(Some(found));
    }
    find_on_youtube(track).await
}

/// Searches the YouTube Music "Songs" results and checks each candidate's real metadata
/// (title, artist, album, length) against the Spotify track.
async fn find_on_music(track: &TrackQuery<'_>) -> Result<Option<Match>> {
    let artist = track.artists.first().map_or("", String::as_str);
    let mut url = Url::parse_with_params(
        "https://music.youtube.com/search",
        [("q", format!("{} {artist}", track.title).trim())],
    )?;
    url.set_fragment(Some("songs"));

    let entries = search(url.as_str()).await?;
    // Look up the results whose titles are closest to ours.
    let mut ranked: Vec<(f64, SearchEntry)> = entries
        .into_iter()
        .map(|e| {
            let sim = matching::title_similarity(track.title, e.title.as_deref().unwrap_or(""));
            (sim, e)
        })
        .filter(|(sim, _)| *sim >= 0.3)
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.truncate(MUSIC_CANDIDATES);

    let mut lookups = JoinSet::new();
    for (_, entry) in ranked {
        lookups.spawn(async move { fetch_info(&entry.id).await });
    }

    let query = track.as_query();
    let mut best: Option<Match> = None;
    while let Some(result) = lookups.join_next().await {
        // A failed lookup of one candidate must not hide the others.
        let Ok(Ok((info, raw))) = result else {
            continue;
        };
        let title = info
            .track
            .clone()
            .or(info.title.clone())
            .unwrap_or_default();
        let artists = artists_of(&info);
        let candidate = Candidate {
            title: &title,
            artists: &artists,
            album: info.album.as_deref(),
            duration_secs: info.duration,
        };
        let Some(score) = matching::score(&query, &candidate, MUSIC_TOLERANCE_SECS) else {
            continue;
        };
        if score >= MIN_SCORE && best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Match {
                id: info.id,
                title,
                artists,
                album: info.album,
                duration_secs: info.duration,
                source: Source::YouTubeMusic,
                score,
                info_json: Some(Arc::new(raw)),
            });
        }
    }
    Ok(best)
}

/// Plain YouTube search: the first result whose length is close enough.
async fn find_on_youtube(track: &TrackQuery<'_>) -> Result<Option<Match>> {
    let artist = track.artists.first().map_or("", String::as_str);
    let query = format!("{artist} {}", track.title).replace(['\'', '"', ',', '.', '/'], "");
    let entries = search(&format!("ytsearch{YOUTUBE_RESULTS}:{query}")).await?;
    let expected = track.duration_secs();
    let found = entries.into_iter().find(|e| {
        expected.is_none_or(|x| {
            e.duration
                .is_some_and(|d| (d - x).abs() <= YOUTUBE_TOLERANCE_SECS)
        })
    });
    Ok(found.map(|e| Match {
        title: e.title.unwrap_or_default(),
        id: e.id,
        artists: Vec::new(),
        album: None,
        duration_secs: e.duration,
        source: Source::YouTube,
        score: 0.0,
        info_json: None,
    }))
}

/// Runs a flat yt-dlp search and returns the result entries.
async fn search(target: &str) -> Result<Vec<SearchEntry>> {
    let _slot = LOOKUPS.acquire().await?;
    let mut args = NETWORK_ARGS.to_vec();
    args.extend([
        "--flat-playlist",
        "--dump-single-json",
        "--no-warnings",
        "--skip-download",
        target,
    ]);
    let output = tokio::time::timeout(SEARCH_TIMEOUT, run("yt-dlp", &args))
        .await
        .map_err(|_| search_timeout())??;
    let parsed: SearchOutput =
        serde_json::from_slice(&output).context("could not read yt-dlp search results")?;
    Ok(parsed.entries)
}

/// Fetches one video's full metadata. Returns it parsed and as the raw JSON text.
async fn fetch_info(id: &str) -> Result<(Info, String)> {
    let _slot = LOOKUPS.acquire().await?;
    let url = format!("https://music.youtube.com/watch?v={id}");
    let mut args = NETWORK_ARGS.to_vec();
    args.extend([
        "--no-warnings",
        "--no-playlist",
        "--skip-download",
        "--dump-json",
        url.as_str(),
    ]);
    let output = tokio::time::timeout(SEARCH_TIMEOUT, run("yt-dlp", &args))
        .await
        .map_err(|_| search_timeout())??;
    let info = serde_json::from_slice(&output).context("could not read video details")?;
    Ok((info, String::from_utf8_lossy(&output).into_owned()))
}

/// Artist names from a video's metadata. Song uploads list them directly; channel names like
/// `Daft Punk - Topic` are the fallback.
fn artists_of(info: &Info) -> Vec<String> {
    if let Some(list) = info.artists.as_ref().filter(|a| !a.is_empty()) {
        return list.clone();
    }
    if let Some(artist) = info.artist.as_deref() {
        return artist.split(", ").map(str::to_string).collect();
    }
    info.uploader.iter().cloned().collect()
}

// ---------- Downloading ----------

/// Downloads a match's audio as `<dir>/<id>.mp3` and returns that path.
/// `on_step` is called as the download progresses and when conversion starts.
pub async fn download_mp3(m: &Match, dir: &Path, on_step: impl Fn(Step)) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;

    // Reuse the metadata we already fetched, so yt-dlp does not look the video up again.
    if let Some(raw) = &m.info_json {
        let info_file = dir.join("info.json");
        tokio::fs::write(&info_file, raw.as_bytes()).await?;
        let first = run_download(dir, &info_file.to_string_lossy(), true, &on_step).await;
        if first.is_ok() {
            return finish(dir, &m.id);
        }
    }
    run_download(dir, &m.url(), false, &on_step).await?;
    finish(dir, &m.id)
}

fn finish(dir: &Path, id: &str) -> Result<PathBuf> {
    let mp3 = dir.join(format!("{id}.mp3"));
    if !mp3.exists() {
        bail!("yt-dlp finished but {} was not created", mp3.display());
    }
    Ok(mp3)
}

async fn run_download(
    dir: &Path,
    source: &str,
    is_info_file: bool,
    on_step: &impl Fn(Step),
) -> Result<()> {
    let template = dir.join("%(id)s.%(ext)s").to_string_lossy().into_owned();
    let download_progress = format!(
        "download:{PROGRESS_TAG} download %(progress.downloaded_bytes)s %(progress.total_bytes)s \
         %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s %(info.ext)s \
         %(info.acodec)s %(info.abr)s %(info.asr)s %(info.audio_channels)s"
    );
    let convert_progress = format!("postprocess:{PROGRESS_TAG} convert");
    let mut args = NETWORK_ARGS.to_vec();
    args.extend([
        "--no-playlist",
        "--no-warnings",
        "--quiet",
        "--progress",
        "--newline",
        "--color",
        "never",
        "--progress-template",
        download_progress.as_str(),
        "--progress-template",
        convert_progress.as_str(),
        "--extract-audio",
        "--audio-format",
        "mp3",
        "--audio-quality",
        "0",
        "--output",
        template.as_str(),
    ]);
    // yt-dlp only looks in its own folder and PATH. If ffmpeg sits next to spotify-dl.exe,
    // tell yt-dlp where it is.
    let ffmpeg = tool_path("ffmpeg").to_string_lossy().into_owned();
    if Path::new(&ffmpeg).is_absolute() {
        args.extend(["--ffmpeg-location", ffmpeg.as_str()]);
    }
    if is_info_file {
        args.extend(["--load-info-json", source]);
    } else {
        args.push(source);
    }

    let mut last_report = Instant::now() - PROGRESS_INTERVAL;
    run_streaming("yt-dlp", &args, |line| {
        let Some(rest) = line.trim().strip_prefix(PROGRESS_TAG) else {
            return;
        };
        let mut parts = rest.split_whitespace();
        match parts.next() {
            Some("download") => {
                let Some((transfer, format)) = parse_download(parts.collect()) else {
                    return;
                };
                let finished = transfer.total.is_some_and(|t| transfer.downloaded >= t);
                if finished || last_report.elapsed() >= PROGRESS_INTERVAL {
                    last_report = Instant::now();
                    on_step(Step::Downloading(transfer, format));
                }
            }
            Some("convert") => on_step(Step::Converting),
            _ => {}
        }
    })
    .await
}

/// Reads the fields of one `download` progress line. yt-dlp prints `NA` for unknown values.
fn parse_download(fields: Vec<&str>) -> Option<(Transfer, SourceFormat)> {
    if fields.len() < 10 {
        return None;
    }
    let num = |i: usize| fields[i].parse::<f64>().ok();
    let text = |i: usize| (fields[i] != "NA" && fields[i] != "none").then(|| fields[i].to_string());
    let transfer = Transfer {
        downloaded: num(0)? as u64,
        total: num(1).or_else(|| num(2)).map(|t| t as u64),
        speed: num(3),
        eta_secs: num(4).map(|e| e as u64),
    };
    let format = SourceFormat {
        container: text(5).unwrap_or_default(),
        codec: text(6).unwrap_or_default(),
        bitrate_kbps: num(7),
        sample_rate: num(8).map(|v| v as u32),
        channels: num(9).map(|v| v as u32),
    };
    Some((transfer, format))
}

/// Reads bitrate, sample rate, channels and length from the finished MP3. Returns `None` if
/// `ffprobe` is not installed or cannot read the file.
pub async fn probe_mp3(path: &Path) -> Option<Mp3Info> {
    #[derive(Deserialize)]
    struct Probe {
        #[serde(default)]
        streams: Vec<Stream>,
        format: Format,
    }
    #[derive(Deserialize)]
    struct Stream {
        sample_rate: Option<String>,
        channels: Option<u32>,
    }
    #[derive(Deserialize)]
    struct Format {
        bit_rate: Option<String>,
        duration: Option<String>,
        size: Option<String>,
    }

    let output = command("ffprobe")
        .args([
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let probe: Probe = serde_json::from_slice(&output.stdout).ok()?;
    let stream = probe.streams.first();
    Some(Mp3Info {
        bitrate_kbps: probe
            .format
            .bit_rate
            .and_then(|b| b.parse::<f64>().ok())
            .map(|b| (b / 1000.0).round() as u32),
        sample_rate: stream
            .and_then(|s| s.sample_rate.as_deref())
            .and_then(|s| s.parse().ok()),
        channels: stream.and_then(|s| s.channels),
        duration_secs: probe.format.duration.and_then(|d| d.parse().ok()),
        size_bytes: probe
            .format
            .size
            .and_then(|s| s.parse().ok())
            .unwrap_or_default(),
    })
}

// ---------- Running tools ----------

/// Runs a tool to completion and returns its stdout.
async fn run(tool: &str, args: &[&str]) -> Result<Vec<u8>> {
    let output = command(tool)
        .args(args)
        .output()
        .await
        .with_context(|| not_found(tool))?;
    if !output.status.success() {
        bail!(
            "{tool} failed: {}",
            last_line(&String::from_utf8_lossy(&output.stderr))
        );
    }
    Ok(output.stdout)
}

/// Runs a tool and hands each stdout line to `on_line` as it arrives.
async fn run_streaming(tool: &str, args: &[&str], mut on_line: impl FnMut(&str)) -> Result<()> {
    let mut child = command(tool)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| not_found(tool))?;

    // Drain stderr in the background so a full pipe can never block the child.
    let mut stderr = child.stderr.take().context("stderr was not captured")?;
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut stdout = BufReader::new(child.stdout.take().context("stdout was not captured")?);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let read = tokio::time::timeout(STALL_TIMEOUT, stdout.read_until(b'\n', &mut buf))
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "{tool} stopped responding for {} seconds. Check your connection \
                     and run `yt-dlp -U` to update yt-dlp.",
                    STALL_TIMEOUT.as_secs()
                )
            })??;
        if read == 0 {
            break;
        }
        on_line(&String::from_utf8_lossy(&buf));
    }

    let status = child.wait().await?;
    let stderr = stderr_task.await.unwrap_or_default();
    if !status.success() {
        bail!("{tool} failed: {}", last_line(&stderr));
    }
    Ok(())
}

fn last_line(text: &str) -> &str {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("no error message")
}

/// Checks that yt-dlp and ffmpeg can be started, so the user gets one clear message up front.
pub async fn check_tools() -> Result<()> {
    // yt-dlp wants `--version`; ffmpeg wants `-version`.
    for (tool, flag) in [("yt-dlp", "--version"), ("ffmpeg", "-version")] {
        command(tool)
            .arg(flag)
            .output()
            .await
            .with_context(|| format!("{tool} was not found. Install it and add it to PATH."))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_download;

    #[test]
    fn parses_a_progress_line() {
        let line = "1048576 5316281 NA 373832.9 14 webm opus 132.748 48000 2";
        let (t, f) = parse_download(line.split_whitespace().collect()).unwrap();
        assert_eq!((t.downloaded, t.total), (1_048_576, Some(5_316_281)));
        assert_eq!(t.eta_secs, Some(14));
        assert_eq!((f.container.as_str(), f.codec.as_str()), ("webm", "opus"));
        assert_eq!((f.sample_rate, f.channels), (Some(48000), Some(2)));
        assert!((f.bitrate_kbps.unwrap() - 132.748).abs() < 1e-9);
    }

    #[test]
    fn handles_unknown_fields_and_estimated_totals() {
        let line = "2048 NA 900000 NA NA m4a mp4a.40.2 NA NA NA";
        let (t, f) = parse_download(line.split_whitespace().collect()).unwrap();
        assert_eq!(t.total, Some(900_000));
        assert_eq!((t.speed, t.eta_secs), (None, None));
        assert_eq!(f.bitrate_kbps, None);
        assert!(parse_download(vec!["1", "2"]).is_none());
    }
}
