//! YouTube search and audio extraction through the `yt-dlp` command (which needs `ffmpeg`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;

/// Allowed difference between the Spotify and YouTube durations, in seconds.
const DURATION_TOLERANCE_SECS: f64 = 30.0;
const SEARCH_RESULTS: u32 = 5;
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

#[derive(Debug, Clone)]
pub struct VideoMatch {
    pub id: String,
    pub title: String,
    pub duration_secs: Option<f64>,
}

impl VideoMatch {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

/// What yt-dlp is doing while it fetches a track.
pub enum Step {
    Downloading {
        percent: String,
        eta: Option<String>,
    },
    Converting,
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

/// Searches YouTube for `query` and returns the first result whose duration is close to
/// `expected_ms`. If the expected duration is unknown, the top result is used.
pub async fn find_match(query: &str, expected_ms: u64) -> Result<Option<VideoMatch>> {
    let search = format!("ytsearch{SEARCH_RESULTS}:{query}");
    let mut args = NETWORK_ARGS.to_vec();
    args.extend([
        "--flat-playlist",
        "--dump-single-json",
        "--no-warnings",
        "--skip-download",
        search.as_str(),
    ]);
    let output = tokio::time::timeout(SEARCH_TIMEOUT, run("yt-dlp", &args))
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "YouTube search did not answer within {} seconds. Check your connection \
                 and run `yt-dlp -U` to update yt-dlp.",
                SEARCH_TIMEOUT.as_secs()
            )
        })??;
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
        duration_secs: e.duration,
    }))
}

/// Downloads a video's audio as `<dir>/<id>.mp3` and returns that path.
/// `on_step` is called as the download progresses and when conversion starts.
pub async fn download_mp3(
    video: &VideoMatch,
    dir: &Path,
    on_step: impl Fn(Step),
) -> Result<PathBuf> {
    tokio::fs::create_dir_all(dir).await?;
    let template = dir.join("%(id)s.%(ext)s").to_string_lossy().into_owned();
    let url = video.url();
    let download_progress =
        format!("download:{PROGRESS_TAG} download %(progress._percent_str)s %(progress._eta_str)s");
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
    args.push(url.as_str());

    let mut last_percent = None;
    run_streaming("yt-dlp", &args, |line| {
        let Some(rest) = line.trim().strip_prefix(PROGRESS_TAG) else {
            return;
        };
        let mut parts = rest.split_whitespace();
        match parts.next() {
            Some("download") => {
                let percent = parts.next().unwrap_or("").to_string();
                // yt-dlp reports many times per second; only pass on whole-percent changes.
                let whole = percent.split('.').next().map(str::to_string);
                if whole == last_percent {
                    return;
                }
                last_percent = whole;
                let eta = parts
                    .next()
                    .filter(|e| !e.is_empty() && *e != "Unknown" && *e != "NA")
                    .map(str::to_string);
                on_step(Step::Downloading { percent, eta });
            }
            Some("convert") => on_step(Step::Converting),
            _ => {}
        }
    })
    .await?;

    let mp3 = dir.join(format!("{}.mp3", video.id));
    if !mp3.exists() {
        bail!("yt-dlp finished but {} was not created", mp3.display());
    }
    Ok(mp3)
}

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
