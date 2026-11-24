//! spotify-dl: download Spotify tracks, albums and playlists as tagged MP3 files.
//!
//! Run without arguments for the terminal UI, or use a subcommand for scripting.

mod config;
mod input;
mod jobs;
mod spotify;
mod tags;
mod tui;
mod youtube;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use config::Config;
use jobs::{Job, Settings, Sink, run_job};

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
    },
    /// Log in to Spotify (needed for playlists)
    Login,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    load_env_files();
    let cli = Cli::parse();
    let mut config = Config::load();

    if let Some(Command::Settings { set }) = &cli.command {
        return show_or_set_settings(&mut config, set.clone());
    }

    let settings = Settings {
        client_id: required_env("SPOTIFY_CLIENT_ID")?,
        client_secret: required_env("SPOTIFY_CLIENT_SECRET")?,
        redirect_uri: std::env::var("SPOTIFY_REDIRECT_URI")
            .unwrap_or_else(|_| DEFAULT_REDIRECT_URI.to_string()),
        download_dir: cli.path.clone().unwrap_or_else(|| config.download_dir()),
    };

    let job = match cli.command {
        None => return tui::run(settings, config).await,
        Some(Command::Song { input }) => Job::Song(input),
        Some(Command::Album { input }) => Job::Album(input),
        Some(Command::Playlist { input }) => Job::Playlist(input),
        Some(Command::Query { query }) => Job::Search(query),
        Some(Command::File { path }) => Job::File(path),
        Some(Command::Login) => Job::Login,
        Some(Command::Settings { .. }) => unreachable!("handled above"),
    };
    run_job(job, &settings, &Sink::Stdout).await
}

fn show_or_set_settings(config: &mut Config, set: Option<PathBuf>) -> Result<()> {
    if let Some(path) = set {
        config.set_download_dir(path)?;
        println!("Saved.");
    }
    println!("Download folder: {}", config.download_dir().display());
    println!("Config file:     {}", config.config_file().display());
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

fn required_env(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(value),
        _ => bail!(
            "{name} is not set. Add it to a .env file next to spotify-dl.exe \
             (see .env.example) or as an environment variable."
        ),
    }
}
