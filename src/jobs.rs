//! Download jobs shared by the command-line subcommands and the terminal UI.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinSet;

use crate::art::Cover;
use crate::config::{Source, clamp_parallel};
use crate::format;
use crate::input::{Kind, parse_id};
use crate::spotify::{SpotifyClient, Track};
use crate::tags;
use crate::youtube::{self, Mp3Info, SourceFormat, Step, Transfer};

/// What the CLI draws for the cover art: about twice as many columns as rows, because a
/// terminal cell is about twice as tall as it is wide.
const CLI_ART_ROWS: u16 = 12;
/// Longest folder name made from an album or playlist title.
const FOLDER_NAME_LIMIT: usize = 100;
/// Width of the CLI status line, so a shorter update fully covers a longer one.
const STATUS_WIDTH: usize = 79;

/// Where a track is in the download pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Searching,
    Downloading,
    Converting,
    Tagging,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Searching => "searching",
            Stage::Downloading => "downloading",
            Stage::Converting => "converting",
            Stage::Tagging => "tagging",
        }
    }
}

/// The live state of one track that is being downloaded.
#[derive(Debug, Clone)]
pub struct TrackStatus {
    pub id: String,
    pub label: String,
    pub stage: Stage,
    pub transfer: Option<Transfer>,
    pub format: Option<SourceFormat>,
}

/// A message from a running job.
pub enum Event {
    /// A permanent line for the log.
    Log(String),
    /// What the job is doing right now. Replaces the previous progress text; empty clears it.
    Progress(String),
    /// A track started or moved to a new stage, or its transfer numbers changed.
    Track(TrackStatus),
    /// A track is finished (saved, skipped or failed) and leaves the active list. Always
    /// followed by an `Overall` event with the new counts.
    TrackDone(String),
    /// How many tracks are finished so far.
    Overall {
        done: usize,
        failed: usize,
        total: usize,
    },
    /// The cover art of the job, to show as ASCII art.
    Cover(Arc<Cover>),
}

/// Where job output goes: stdout for the CLI, a channel for the TUI.
#[derive(Clone)]
pub enum Sink {
    Stdout(Arc<Mutex<CliView>>),
    Channel(UnboundedSender<Event>),
}

impl Sink {
    pub fn stdout() -> Self {
        Sink::Stdout(Arc::new(Mutex::new(CliView::new())))
    }

    pub fn log(&self, message: impl Into<String>) {
        let message = message.into();
        match self {
            Sink::Stdout(view) => view_of(view).log(&message),
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Log(message));
            }
        }
    }

    pub fn progress(&self, message: impl Into<String>) {
        let message = message.into();
        match self {
            Sink::Stdout(view) => view_of(view).set_message(message),
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Progress(message));
            }
        }
    }

    pub fn track(&self, status: TrackStatus) {
        match self {
            Sink::Stdout(view) => view_of(view).update(status),
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Track(status));
            }
        }
    }

    /// A track is finished (saved, skipped or failed): it leaves the active list and the
    /// overall counts change in one step.
    pub fn track_finished(&self, id: &str, done: usize, failed: usize, total: usize) {
        match self {
            Sink::Stdout(view) => {
                let mut view = view_of(view);
                view.forget(id);
                view.set_overall(done, failed, total);
            }
            Sink::Channel(tx) => {
                let _ = tx.send(Event::TrackDone(id.to_string()));
                let _ = tx.send(Event::Overall {
                    done,
                    failed,
                    total,
                });
            }
        }
    }

    pub fn overall(&self, done: usize, failed: usize, total: usize) {
        match self {
            Sink::Stdout(view) => view_of(view).set_overall(done, failed, total),
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Overall {
                    done,
                    failed,
                    total,
                });
            }
        }
    }

    pub fn cover(&self, cover: Arc<Cover>) {
        match self {
            Sink::Stdout(view) => view_of(view).print_cover(&cover),
            Sink::Channel(tx) => {
                let _ = tx.send(Event::Cover(cover));
            }
        }
    }
}

/// On Windows this also switches the console to understand ANSI escape codes.
#[cfg(windows)]
fn terminal_understands_ansi() -> bool {
    ratatui::crossterm::ansi_support::supports_ansi()
}

#[cfg(not(windows))]
fn terminal_understands_ansi() -> bool {
    true
}

/// Locks the CLI view. A poisoned lock only means another thread panicked mid-update; the
/// numbers are still usable for drawing.
fn view_of(view: &Mutex<CliView>) -> std::sync::MutexGuard<'_, CliView> {
    view.lock().unwrap_or_else(|e| e.into_inner())
}

/// What the CLI shows: log lines, and one status line that sums up every active download.
pub struct CliView {
    is_terminal: bool,
    /// Whether colour escape codes work in this terminal and the user has not opted out.
    colour: bool,
    message: String,
    active: HashMap<String, (Option<Transfer>, Stage)>,
    /// Bytes of tracks already finished, counted towards the totals.
    finished_bytes: u64,
    done: usize,
    failed: usize,
    total: usize,
    status_shown: bool,
}

impl CliView {
    fn new() -> Self {
        let is_terminal = std::io::stdout().is_terminal();
        let ansi = is_terminal && terminal_understands_ansi();
        let colour = ansi && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        Self {
            is_terminal,
            colour,
            message: String::new(),
            active: HashMap::new(),
            finished_bytes: 0,
            done: 0,
            failed: 0,
            total: 0,
            status_shown: false,
        }
    }

    fn log(&mut self, message: &str) {
        self.clear_status();
        println!("{message}");
        self.draw_status();
    }

    fn set_message(&mut self, message: String) {
        self.message = message;
        self.draw_status();
    }

    fn update(&mut self, status: TrackStatus) {
        self.active
            .insert(status.id, (status.transfer, status.stage));
        self.draw_status();
    }

    /// Drops a finished track from the active list, keeping its bytes in the totals. The
    /// caller redraws.
    fn forget(&mut self, id: &str) {
        if let Some((Some(transfer), _)) = self.active.remove(id) {
            self.finished_bytes += transfer.total.unwrap_or(transfer.downloaded);
        }
    }

    fn set_overall(&mut self, done: usize, failed: usize, total: usize) {
        (self.done, self.failed, self.total) = (done, failed, total);
        self.draw_status();
    }

    fn print_cover(&mut self, cover: &Cover) {
        if !self.is_terminal {
            return;
        }
        self.clear_status();
        let mut out = String::new();
        for row in cover.ascii(CLI_ART_ROWS * 2, CLI_ART_ROWS) {
            for cell in row {
                if self.colour {
                    let [r, g, b] = cell.rgb;
                    out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{}", cell.ch));
                } else {
                    out.push(cell.ch);
                }
            }
            if self.colour {
                out.push_str("\x1b[0m");
            }
            out.push('\n');
        }
        print!("{out}");
        self.draw_status();
    }

    /// The status line: overall bar, percent, speed, bytes and track counts.
    fn status_line(&self) -> String {
        if self.active.is_empty() && self.total == 0 {
            return self.message.clone();
        }
        let active_fraction: f64 = self
            .active
            .values()
            .filter_map(|(t, _)| t.as_ref())
            .filter_map(|t| {
                t.total
                    .map(|total| t.downloaded as f64 / total.max(1) as f64)
            })
            .sum();
        let finished = (self.done + self.failed) as f64;
        let fraction = if self.total == 0 {
            0.0
        } else {
            ((finished + active_fraction) / self.total as f64).min(1.0)
        };
        let transfers: Vec<&Transfer> = self
            .active
            .values()
            .filter_map(|(t, _)| t.as_ref())
            .collect();
        let speed: f64 = transfers.iter().filter_map(|t| t.speed).sum();
        let downloaded = self.finished_bytes + transfers.iter().map(|t| t.downloaded).sum::<u64>();
        let known_total = self.finished_bytes
            + transfers
                .iter()
                .map(|t| t.total.unwrap_or(t.downloaded))
                .sum::<u64>();
        format!(
            "{} {:>3.0}%  {}  {} / {}  {}/{} tracks",
            format::bar(fraction, 20),
            fraction * 100.0,
            format::rate(speed),
            format::bytes(downloaded),
            format::bytes(known_total),
            self.done + self.failed,
            self.total,
        )
    }

    fn draw_status(&mut self) {
        if !self.is_terminal {
            return;
        }
        let line = self.status_line();
        if line.is_empty() && !self.status_shown {
            return;
        }
        let text: String = line.chars().take(STATUS_WIDTH).collect();
        let mut out = std::io::stdout();
        let _ = write!(out, "\r{text:<STATUS_WIDTH$}");
        let _ = out.flush();
        self.status_shown = true;
    }

    fn clear_status(&mut self) {
        if self.status_shown {
            let mut out = std::io::stdout();
            let _ = write!(out, "\r{:STATUS_WIDTH$}\r", "");
            self.status_shown = false;
        }
    }
}

/// Everything a job needs that does not change while the program runs.
#[derive(Clone)]
pub struct Settings {
    pub client_id: String,
    pub client_secret: String,
    pub id_source: Source,
    pub secret_source: Source,
    pub redirect_uri: String,
    pub download_dir: PathBuf,
    /// How many tracks are downloaded at the same time (1 to 8).
    pub parallel: usize,
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

/// Shared by every track of a job.
struct Ctx {
    http: reqwest::Client,
    sink: Sink,
    /// Downloaded cover images by URL, so an album's art is fetched once for all its tracks.
    /// `None` records a failed fetch so it is not retried (or reported) for every track.
    covers: tokio::sync::Mutex<HashMap<String, Option<Arc<Vec<u8>>>>>,
}

impl Ctx {
    async fn cover_bytes(&self, url: &str) -> Option<Arc<Vec<u8>>> {
        // Holding the lock while fetching makes concurrent tracks wait for the first fetch
        // instead of each downloading the same image.
        let mut covers = self.covers.lock().await;
        if let Some(found) = covers.get(url) {
            return found.clone();
        }
        let fetched = match fetch_image(&self.http, url).await {
            Ok(data) => Some(Arc::new(data)),
            Err(e) => {
                self.sink
                    .log(format!("warning: could not download cover art: {e:#}"));
                None
            }
        };
        covers.insert(url.to_string(), fetched.clone());
        fetched
    }

    /// Shows the job's cover as ASCII art and returns the image bytes.
    async fn show_cover(&self, url: Option<&str>) -> Option<Arc<Vec<u8>>> {
        let bytes = self.cover_bytes(url?).await?;
        match Cover::decode(&bytes) {
            Ok(cover) => self.sink.cover(Arc::new(cover)),
            Err(e) => self.sink.log(format!("warning: {e:#}")),
        }
        Some(bytes)
    }
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

pub async fn run_job(job: Job, settings: &Settings, sink: &Sink) -> Result<()> {
    let result = run_job_inner(job, settings, sink).await;
    sink.progress("");
    result
}

async fn run_job_inner(job: Job, settings: &Settings, sink: &Sink) -> Result<()> {
    if settings.client_id.trim().is_empty() || settings.client_secret.trim().is_empty() {
        bail!(
            "Spotify client ID and secret are not set. Open Settings in the menu to enter them, \
             run `spotify-dl settings --client-id <id> --client-secret <secret>`, or add \
             SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET to a .env file next to spotify-dl.exe."
        );
    }
    let mut spotify = SpotifyClient::new(
        settings.client_id.clone(),
        settings.client_secret.clone(),
        settings.redirect_uri.clone(),
        sink.clone(),
    )?;
    let ctx = Arc::new(Ctx {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?,
        sink: sink.clone(),
        covers: tokio::sync::Mutex::new(HashMap::new()),
    });

    if let Job::Login = job {
        spotify.login().await?;
        sink.log("Spotify login saved. Playlist downloads will use it from now on.");
        return Ok(());
    }

    sink.progress("Checking yt-dlp and ffmpeg...");
    youtube::check_tools().await?;
    std::fs::create_dir_all(&settings.download_dir)
        .with_context(|| format!("cannot create {}", settings.download_dir.display()))?;
    let base = settings.download_dir.as_path();
    let parallel = clamp_parallel(settings.parallel);

    match job {
        Job::Song(input) => {
            let id = parse_id(&input, Kind::Track)?;
            sink.progress("Looking up the track on Spotify...");
            let track = spotify.track(&id).await?;
            sink.log(format!(
                "Track: {} ({})",
                track.display_name(),
                format::duration(track.duration_ms / 1000)
            ));
            ctx.show_cover(track.image_url.as_deref()).await;
            download_all(&ctx, base, vec![track], 1).await?;
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
            let artist = album.tracks.first().and_then(|t| t.artists.first());
            let dir = base.join(album_folder(artist.map(String::as_str), &album.name));
            let cover = ctx.show_cover(album.image_url.as_deref()).await;
            save_cover_file(sink, &dir, cover.as_deref().map(Vec::as_slice));
            download_all(&ctx, &dir, album.tracks, parallel).await?;
        }
        Job::Playlist(input) => {
            let id = parse_id(&input, Kind::Playlist)?;
            sink.progress("Loading the playlist from Spotify...");
            let playlist = spotify.playlist(&id).await?;
            sink.log(format!(
                "Playlist: {} ({} playable tracks)",
                playlist.name,
                playlist.tracks.len()
            ));
            let dir = base.join(folder_name(&playlist.name));
            let cover = ctx.show_cover(playlist.image_url.as_deref()).await;
            save_cover_file(sink, &dir, cover.as_deref().map(Vec::as_slice));
            download_all(&ctx, &dir, playlist.tracks, parallel).await?;
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
                format::duration(track.duration_ms / 1000)
            ));
            ctx.show_cover(track.image_url.as_deref()).await;
            download_all(&ctx, base, vec![track], 1).await?;
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
            download_all(&ctx, base, tracks, parallel).await?;
        }
        Job::Login => unreachable!("handled above"),
    }
    Ok(())
}

/// Saves the job's cover next to its tracks as `cover.jpg` or `cover.png`, unless the folder
/// already has one.
fn save_cover_file(sink: &Sink, dir: &Path, cover: Option<&[u8]>) {
    let Some(data) = cover else {
        return;
    };
    let file = dir.join(format!("cover.{}", tags::image_extension(data)));
    if dir.join("cover.jpg").exists() || dir.join("cover.png").exists() {
        return;
    }
    let saved = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&file, data));
    match saved {
        Ok(()) => sink.log(format!("Cover saved: {}", file.display())),
        Err(e) => sink.log(format!("warning: could not save {}: {e}", file.display())),
    }
}

/// How one track ended.
enum Outcome {
    Saved,
    /// The file was already there.
    Skipped,
}

/// Running totals for a batch, shared by its tasks.
#[derive(Default)]
struct Tally {
    saved: usize,
    skipped: usize,
    failed: usize,
    first_error: Option<anyhow::Error>,
}

impl Tally {
    fn finished(&self) -> usize {
        self.saved + self.skipped
    }
}

/// Downloads `tracks` into `dir`, up to `parallel` at a time.
///
/// A failing track is logged and does not stop the others. When there is only one track, its
/// error is returned instead, so the caller reports it once.
async fn download_all(
    ctx: &Arc<Ctx>,
    dir: &Path,
    tracks: Vec<Track>,
    parallel: usize,
) -> Result<()> {
    let total = tracks.len();
    ctx.sink.overall(0, 0, total);
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("cannot create {}", dir.display()))?;

    let slots = Arc::new(Semaphore::new(parallel.max(1)));
    let dir = Arc::new(dir.to_path_buf());
    let tally = Arc::new(Mutex::new(Tally::default()));
    // Dropping the set (when the job is cancelled) aborts the tasks that are still running.
    let mut tasks = JoinSet::new();
    for (i, track) in tracks.into_iter().enumerate() {
        // Wait for a free slot before starting the next track, so tracks start in order.
        let permit = slots
            .clone()
            .acquire_owned()
            .await
            .context("download queue closed")?;
        let (ctx, dir, tally) = (ctx.clone(), dir.clone(), tally.clone());
        tasks.spawn(async move {
            let result = download_one(&ctx, &dir, &track, i + 1, total).await;
            drop(permit);
            record(&ctx, &tally, &track, i + 1, total, result);
        });
    }
    while let Some(finished) = tasks.join_next().await {
        if let Err(e) = finished {
            // A task that panicked never reported itself.
            let mut t = tally.lock().unwrap_or_else(|e| e.into_inner());
            t.failed += 1;
            t.first_error
                .get_or_insert(anyhow!("a download task stopped unexpectedly: {e}"));
            ctx.sink.overall(t.finished(), t.failed, total);
        }
    }

    let mut t = tally.lock().unwrap_or_else(|e| e.into_inner());
    if total == 1
        && let Some(e) = t.first_error.take()
    {
        return Err(e);
    }
    let mut summary = format!(
        "Finished: {}/{total} tracks saved to {}",
        t.finished(),
        dir.display()
    );
    if t.skipped > 0 {
        summary.push_str(&format!(" ({} already there)", t.skipped));
    }
    if t.failed > 0 {
        summary.push_str(&format!(", {} failed", t.failed));
    }
    ctx.sink.log(summary);
    Ok(())
}

/// Counts a finished track and tells the sink, so the track leaves the active list and the
/// overall counts change together.
fn record(
    ctx: &Ctx,
    tally: &Mutex<Tally>,
    track: &Track,
    index: usize,
    total: usize,
    result: Result<Outcome>,
) {
    let mut t = tally.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(Outcome::Saved) => t.saved += 1,
        Ok(Outcome::Skipped) => t.skipped += 1,
        Err(e) => {
            t.failed += 1;
            if total > 1 {
                ctx.sink.log(format!(
                    "[{index}/{total}] [fail] {}: {e:#}",
                    track.display_name()
                ));
            }
            t.first_error.get_or_insert(e);
        }
    }
    ctx.sink
        .track_finished(&track.id, t.finished(), t.failed, total);
}

/// Finds one track on YouTube Music (or YouTube), downloads it as MP3 into `dir` and tags it.
async fn download_one(
    ctx: &Ctx,
    dir: &Path,
    track: &Track,
    index: usize,
    total: usize,
) -> Result<Outcome> {
    let sink = &ctx.sink;
    let name = track.display_name();
    let tag = format!("[{index}/{total}]");
    let target = dir.join(sanitize_filename(&format!("{name}.mp3")));
    let status = |stage: Stage, transfer: Option<Transfer>, format: Option<SourceFormat>| {
        sink.track(TrackStatus {
            id: track.id.clone(),
            label: name.clone(),
            stage,
            transfer,
            format,
        });
    };

    if target.exists() {
        sink.log(format!(
            "{tag} skipped, already exists: {}",
            target.display()
        ));
        return Ok(Outcome::Skipped);
    }
    sink.log(format!(
        "{tag} {name} ({})",
        format::duration(track.duration_ms / 1000)
    ));
    let started = Instant::now();

    status(Stage::Searching, None, None);
    let query = youtube::TrackQuery {
        title: &track.name,
        artists: &track.artists,
        album: track.album.as_deref(),
        duration_ms: track.duration_ms,
    };
    let video = youtube::find_match(&query).await?.ok_or_else(|| {
        anyhow!("no YouTube Music or YouTube match for \"{name}\" with a similar length")
    })?;
    if video.source == youtube::Source::YouTube {
        sink.log(format!(
            "{tag} no good YouTube Music match, using a plain YouTube result instead"
        ));
    }
    sink.log(format!("{tag} match: {}", describe_match(&video)));

    // Two entries of one playlist may be the same track, so the folder includes the position.
    let temp_dir = std::env::temp_dir()
        .join("spotify-dl")
        .join(format!("{}-{index}", track.id));
    let result = async {
        // The last numbers yt-dlp reported, and when conversion began.
        let seen: Mutex<(Option<Transfer>, SourceFormat, Option<Instant>)> =
            Mutex::new((None, SourceFormat::default(), None));
        let download_started = Instant::now();
        status(Stage::Downloading, None, None);
        let mp3 = youtube::download_mp3(&video, &temp_dir, |step| match step {
            Step::Downloading(transfer, format) => {
                status(Stage::Downloading, Some(transfer), Some(format.clone()));
                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                (seen.0, seen.1) = (Some(transfer), format);
            }
            Step::Converting => {
                let mut seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                seen.2.get_or_insert_with(Instant::now);
                status(Stage::Converting, seen.0, Some(seen.1.clone()));
            }
        })
        .await?;
        let (transfer, format, converting_since) =
            seen.into_inner().unwrap_or_else(|e| e.into_inner());
        let download_time = converting_since.unwrap_or_else(Instant::now) - download_started;
        if let Some(line) = describe_source(transfer.as_ref(), &format) {
            sink.log(format!("{tag} source: {line}"));
        }

        move_file(&mp3, &target).await?;
        status(Stage::Tagging, transfer, Some(format));
        let cover = match &track.image_url {
            Some(url) => ctx.cover_bytes(url).await,
            None => None,
        };
        if let Err(e) = tags::write(&target, track, cover.as_deref().map(Vec::as_slice)) {
            sink.log(format!(
                "{tag} warning: saved but could not write tags: {e:#}"
            ));
        }
        Ok::<_, anyhow::Error>((transfer, download_time))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp_dir).await;
    let (transfer, download_time) = result?;

    sink.log(format!("{tag} saved: {}", target.display()));
    let probe = youtube::probe_mp3(&target).await;
    let size = std::fs::metadata(&target).map_or(0, |m| m.len());
    sink.log(format!(
        "{tag} mp3: {}",
        describe_mp3(
            probe.as_ref(),
            size,
            started.elapsed(),
            transfer.as_ref(),
            download_time
        )
    ));
    Ok(Outcome::Saved)
}

/// `"One More Time" by Daft Punk, album Discovery, 5:20, YouTube Music, score 4.8/5`.
fn describe_match(m: &youtube::Match) -> String {
    let mut parts = vec![format!("\"{}\"", m.title)];
    if !m.artists.is_empty() {
        parts.push(format!("by {}", m.artists.join(", ")));
    }
    if let Some(album) = &m.album {
        parts.push(format!("album {album}"));
    }
    if let Some(secs) = m.duration_secs {
        parts.push(format::duration(secs.round() as u64));
    }
    parts.push(m.source.label().to_string());
    if m.source == youtube::Source::YouTubeMusic {
        parts.push(format!("score {:.1}/5", m.score));
    }
    parts.join(", ")
}

/// `webm/opus, 133 kbps, 48 kHz, stereo, 5.1 MB`, or `None` if yt-dlp reported nothing.
fn describe_source(transfer: Option<&Transfer>, f: &SourceFormat) -> Option<String> {
    let mut parts = Vec::new();
    match (f.container.is_empty(), f.codec.is_empty()) {
        (false, false) => parts.push(format!("{}/{}", f.container, f.codec)),
        (false, true) => parts.push(f.container.clone()),
        (true, false) => parts.push(f.codec.clone()),
        (true, true) => {}
    }
    if let Some(kbps) = f.bitrate_kbps {
        parts.push(format!("{kbps:.0} kbps"));
    }
    if let Some(hz) = f.sample_rate {
        parts.push(format::khz(hz));
    }
    if let Some(n) = f.channels {
        parts.push(format::channels(n));
    }
    if let Some(t) = transfer {
        parts.push(format::bytes(t.total.unwrap_or(t.downloaded)));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// `243 kbps, 44.1 kHz, stereo, 9.7 MB, 5:20, took 14s, avg 410.2 KB/s`.
fn describe_mp3(
    probe: Option<&Mp3Info>,
    file_size: u64,
    took: Duration,
    transfer: Option<&Transfer>,
    download_time: Duration,
) -> String {
    let mut parts = Vec::new();
    if let Some(p) = probe {
        if let Some(kbps) = p.bitrate_kbps {
            parts.push(format!("{kbps} kbps"));
        }
        if let Some(hz) = p.sample_rate {
            parts.push(format::khz(hz));
        }
        if let Some(n) = p.channels {
            parts.push(format::channels(n));
        }
    }
    let size = probe.map_or(file_size, |p| p.size_bytes).max(file_size);
    parts.push(format::bytes(size));
    if let Some(secs) = probe.and_then(|p| p.duration_secs) {
        parts.push(format::duration(secs.round() as u64));
    }
    parts.push(format!("took {}s", took.as_secs()));
    if let Some(t) = transfer {
        let secs = download_time.as_secs_f64();
        if secs > 0.0 {
            parts.push(format!("avg {}", format::rate(t.downloaded as f64 / secs)));
        }
    }
    parts.join(", ")
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

/// A folder name from an album or playlist title: shortened, then made safe.
fn folder_name(title: &str) -> String {
    let short: String = title.chars().take(FOLDER_NAME_LIMIT).collect();
    sanitize_filename(&short)
}

/// `Artist - Album`, or just the album when the artist is unknown.
fn album_folder(artist: Option<&str>, album: &str) -> String {
    match artist {
        Some(artist) => folder_name(&format!("{artist} - {album}")),
        None => folder_name(album),
    }
}

#[cfg(test)]
mod tests {
    use super::{album_folder, folder_name, sanitize_filename};

    #[test]
    fn replaces_reserved_characters() {
        assert_eq!(sanitize_filename("AC/DC: Back?.mp3"), "AC_DC_ Back_.mp3");
        assert_eq!(sanitize_filename("name. "), "name");
        assert_eq!(sanitize_filename("..."), "untitled");
    }

    #[test]
    fn album_folders_are_artist_dash_album() {
        assert_eq!(
            album_folder(Some("Daft Punk"), "Discovery"),
            "Daft Punk - Discovery"
        );
        assert_eq!(
            album_folder(Some("AC/DC"), "Back In Black"),
            "AC_DC - Back In Black"
        );
        assert_eq!(album_folder(None, "Discovery"), "Discovery");
    }

    #[test]
    fn folder_names_are_shortened_and_never_empty() {
        let long = "x".repeat(300);
        assert_eq!(folder_name(&long).chars().count(), 100);
        // Cutting can leave a trailing space or dot, which Windows does not allow.
        let cut = format!("{} .tail", "y".repeat(99));
        assert_eq!(folder_name(&cut), "y".repeat(99));
        assert_eq!(folder_name("???"), "___");
        assert_eq!(folder_name(""), "untitled");
    }
}
