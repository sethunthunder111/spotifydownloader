# spotify-dl

Download Spotify tracks, albums and playlists as tagged MP3 files. Written in Rust, shipped as a single Windows exe with a terminal UI.

Landing page: [`docs/index.html`](docs/index.html) (publish with GitHub Pages).

## Features

- Songs, albums and playlists from a link, URI or ID, or search by "Artist - Title"
- Batch download from a text file
- ID3v2.4 tags: title, artist, album, year and front cover art
- YouTube matches must have a duration close to the Spotify track
- Full-screen terminal UI (run with no arguments) and scriptable subcommands

## Requirements

- Windows 10 or 11 (64-bit)
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [ffmpeg](https://ffmpeg.org/), on `PATH` or next to `spotify-dl.exe`
- A Spotify developer app (free)

```powershell
winget install yt-dlp.yt-dlp
winget install Gyan.FFmpeg
```

## Setup

1. Create an app at the [Spotify developer dashboard](https://developer.spotify.com/dashboard).
2. Add `http://127.0.0.1:5000/callback` as a Redirect URI in the app settings.
3. Copy `.env.example` to `.env` next to the exe and fill in `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET`.

## Usage

```text
spotify-dl                          Open the terminal UI
spotify-dl song <link|uri|id>       Download one track
spotify-dl album <link|uri|id>      Download an album
spotify-dl playlist <link|uri|id>   Download a playlist (needs login)
spotify-dl query "Artist - Title"   Search and download the best match
spotify-dl file <list.txt>          Download every link in a file (# starts a comment)
spotify-dl login                    Log in to Spotify again
spotify-dl settings [--set <dir>]   Show or change the download folder
spotify-dl -p <dir> <command>       Use another download folder for one run
```

Downloads go to your Music folder in a `spotify-dl` subfolder unless you change it.

Playlists use your Spotify account, so you log in once in the browser. The token is stored in `%APPDATA%\spotify-dl\refresh_token.json`.

## Building from source

```powershell
cargo build --release
# target\release\spotify-dl.exe
```

Tests and lints:

```powershell
cargo test
cargo clippy --all-targets -- -D warnings
```

## Project layout

| Path | Purpose |
|------|---------|
| `src/main.rs` | CLI definition and dispatch |
| `src/tui.rs` | Terminal UI (ratatui) |
| `src/jobs.rs` | Download pipeline shared by CLI and TUI |
| `src/spotify.rs` | Spotify Web API client and login flow |
| `src/youtube.rs` | yt-dlp search and MP3 extraction |
| `src/tags.rs` | ID3 tag and cover art writing |
| `src/input.rs` | Link, URI and ID parsing |
| `legacy/` | The original Node.js version, kept for reference |
| `docs/` | Landing page, architecture, development and troubleshooting docs |

## Notes

- The Node.js version in `legacy/` is no longer maintained.
- Only download music you have the right to download.

## License

Released under the [MIT License](LICENSE).
