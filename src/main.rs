//! spotify-dl: download Spotify tracks, albums and playlists as tagged MP3 files.
//!
//! Run without arguments for the terminal UI, or use a subcommand for scripting.

mod art;
mod config;
mod format;
mod input;
mod isrc;
mod jobs;
mod matching;
mod paths;
mod spotify;
mod tags;
mod tui;
mod youtube;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use config::{Config, MAX_PARALLEL, MIN_PARALLEL, clamp_parallel, describe};
use jobs::{Export, IsrcRequest, Job, Settings, Sink, run_job};

const DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:5000/callback";

#[derive(Parser)]
#[command(
    name = "spotify-dl",
    version,
    about = "Download Spotify music as tagged MP3s"
)]
struct Cli {
    /// Download folder for this run only (does not change the saved setting)
    #[arg(short = 'p', long = "path", global = true)]
    path: Option<PathBuf>,

    /// Tracks to download at the same time, 1 to 8 (this run only)
    #[arg(short = 'j', long = "jobs", global = true, value_name = "N")]
    jobs: Option<usize>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Download one track (URL, URI or ID)
    Song { input: String },
    /// Download every track of an album (URL, URI or ID)
    Album { input: String },
    /// Download every track of a playlist you own or follow (URL, URI or ID)
    Playlist { input: String },
    /// Search Spotify and download the best match
    Query { query: String },
    /// Download the tracks listed in a text file (one link or ID per line)
    File { path: PathBuf },
    /// Show or change saved settings
    Settings {
        /// Save a new default download folder
        #[arg(long)]
        set: Option<PathBuf>,
        /// Save your Spotify app's client ID
        #[arg(long)]
        client_id: Option<String>,
        /// Save your Spotify app's client secret (it may end up in your shell history)
        #[arg(long)]
        client_secret: Option<String>,
        /// Save how many tracks are downloaded at the same time, 1 to 8
        #[arg(long, value_name = "N")]
        parallel: Option<usize>,
    },
    /// Log in to Spotify (needed for playlists)
    Login,
    /// Find ISRC codes for a song or a playlist, and export them as CSV or JSON
    Isrc {
        /// Song or playlist link, URI or ID, or a song name to search for
        input: String,
        /// Save a CSV file. Give a file name, or leave it out to use "<name> - ISRC.csv" in the
        /// download folder
        #[arg(long, value_name = "FILE", num_args = 0..=1)]
        csv: Option<Option<PathBuf>>,
        /// Save a JSON file. Give a file name, or leave it out to use "<name> - ISRC.json" in
        /// the download folder
        #[arg(long, value_name = "FILE", num_args = 0..=1)]
        json: Option<Option<PathBuf>>,
    },
}

/// Turns an optional `--csv [FILE]` style flag into an export target.
fn export_target(flag: Option<Option<PathBuf>>) -> Export {
    match flag {
        None => Export::Off,
        Some(None) => Export::Default,
        Some(Some(path)) => Export::Path(path),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            pause_if_console_would_close();
            ExitCode::FAILURE
        }
    }
}

/// When the exe is double-clicked, Windows opens a console that vanishes as soon as the
/// program exits, so the user never sees the error. Wait for Enter in that case.
#[cfg(windows)]
fn pause_if_console_would_close() {
    unsafe extern "system" {
        fn GetConsoleProcessList(process_list: *mut u32, process_count: u32) -> u32;
    }
    let mut pids = [0u32; 2];
    // SAFETY: `pids` is a valid buffer of 2 elements, matching the count passed.
    let attached = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), 2) };
    if attached == 1 {
        eprintln!("\nPress Enter to close this window.");
        let _ = std::io::stdin().read_line(&mut String::new());
    }
}

#[cfg(not(windows))]
fn pause_if_console_would_close() {}

async fn run() -> Result<()> {
    load_env_files();
    let cli = Cli::parse();
    let mut config = Config::load();
    config.adopt_env_credentials();

    if let Some(Command::Settings {
        set,
        client_id,
        client_secret,
        parallel,
    }) = &cli.command
    {
        return show_or_set_settings(
            &mut config,
            set.clone(),
            client_id.as_deref(),
            client_secret.as_deref(),
            *parallel,
        );
    }

    // Missing credentials are reported when a job starts, so the TUI can open and let the
    // user enter them in Settings.
    let (client_id, id_source) = config.client_id();
    let (client_secret, secret_source) = config.client_secret();
    let settings = Settings {
        client_id,
        client_secret,
        id_source,
        secret_source,
        redirect_uri: std::env::var("SPOTIFY_REDIRECT_URI")
            .unwrap_or_else(|_| DEFAULT_REDIRECT_URI.to_string()),
        download_dir: cli.path.clone().unwrap_or_else(|| config.download_dir()),
        parallel: cli.jobs.map_or_else(|| config.parallel(), clamp_parallel),
    };

    let job = match cli.command {
        None => return tui::run(settings, config).await,
        Some(Command::Song { input }) => Job::Song(input),
        Some(Command::Album { input }) => Job::Album(input),
        Some(Command::Playlist { input }) => Job::Playlist(input),
        Some(Command::Query { query }) => Job::Search(query),
        Some(Command::File { path }) => Job::File(path),
        Some(Command::Login) => Job::Login,
        Some(Command::Isrc { input, csv, json }) => Job::Isrc(IsrcRequest {
            input,
            csv: export_target(csv),
            json: export_target(json),
        }),
        Some(Command::Settings { .. }) => unreachable!("handled above"),
    };
    run_job(job, &settings, &Sink::stdout()).await
}

fn show_or_set_settings(
    config: &mut Config,
    set: Option<PathBuf>,
    client_id: Option<&str>,
    client_secret: Option<&str>,
    parallel: Option<usize>,
) -> Result<()> {
    let mut changed = false;
    if let Some(path) = set {
        config.set_download_dir(path)?;
        changed = true;
    }
    if let Some(id) = client_id {
        config.set_client_id(id)?;
        changed = true;
    }
    if let Some(secret) = client_secret {
        config.set_client_secret(secret)?;
        changed = true;
    }
    if let Some(n) = parallel {
        if !(MIN_PARALLEL..=MAX_PARALLEL).contains(&n) {
            bail!("--parallel must be between {MIN_PARALLEL} and {MAX_PARALLEL}");
        }
        config.set_parallel(n)?;
        changed = true;
    }
    if changed {
        println!("Saved.");
    }
    println!("Download folder: {}", config.download_dir().display());
    println!("Parallel:        {} tracks at a time", config.parallel());
    println!("Config file:     {}", config.config_file().display());
    let (id, id_source) = config.client_id();
    let (secret, secret_source) = config.client_secret();
    println!("Client ID:       {}", describe(&id, id_source, false));
    println!(
        "Client secret:   {}",
        describe(&secret, secret_source, true)
    );
    Ok(())
}

/// Loads `.env` from the executable's folder and from the current folder (executable's folder wins).
fn load_env_files() {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join(".env")))
    {
        let _ = dotenvy::from_path(dir);
    }
    let _ = dotenvy::dotenv();
}
