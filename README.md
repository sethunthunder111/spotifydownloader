# spotify-dl

Download Spotify tracks, albums and playlists as tagged MP3 files. Written in Rust, shipped as a single Windows exe with a terminal UI.

Website: https://sethunthunder111.github.io/spotifydownloader/

## Features

- Songs, albums and playlists from a link, URI or ID, or search by "Artist - Title"
- Batch download from a text file
- Tracks are matched on YouTube Music first, by title, artist, album and length, so you get the album recording. Plain YouTube is the fallback, and the log says when it is used
- Parallel downloads (3 at a time by default, up to 8)
- Albums and playlists get their own subfolder, with `cover.jpg` saved inside
- Detailed progress while downloading: speed, MB done, ETA, source and MP3 bitrate, sample rate and channels
- ASCII art of the cover in the terminal
- ID3v2.4 tags: title, artist, album, year and front cover art
- ISRC finder for songs and playlists, with CSV and JSON export
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
3. Give spotify-dl the app's client ID and secret. Either open **Settings** in the app and enter them there, or copy `.env.example` to `.env` next to the exe and fill in `SPOTIFY_CLIENT_ID` and `SPOTIFY_CLIENT_SECRET`.

## Usage

```text
spotify-dl                          Open the terminal UI
spotify-dl song <link|uri|id>       Download one track
spotify-dl album <link|uri|id>      Download an album
spotify-dl playlist <link|uri|id>   Download a playlist (needs login)
spotify-dl query "Artist - Title"   Search and download the best match
spotify-dl file <list.txt>          Download every link in a file (# starts a comment)
spotify-dl isrc <link|uri|id|name>  Show the ISRC code of a song, or list a playlist's ISRCs
spotify-dl isrc <...> --csv [file] --json [file]
                                    Also save the result as CSV and/or JSON
spotify-dl login                    Log in to Spotify again
spotify-dl settings [--set <dir>]   Show or change the download folder
spotify-dl settings --parallel <n>  Save how many tracks download at once (1 to 8)
spotify-dl settings --client-id <id> --client-secret <secret>
                                    Save your Spotify credentials
spotify-dl -p <dir> <command>       Use another download folder for one run
spotify-dl -j <n> <command>         Download n tracks at once for one run
```

Songs, searches and file lists are saved straight into the download folder. An album goes into `<download folder>\Artist - Album\` and a playlist into `<download folder>\<playlist name>\`, each with a `cover.jpg` (or `cover.png`) next to the tracks. A track whose file already exists is skipped, so you can run an album again to fill in the gaps.

The cover art is also embedded in every MP3 as the ID3 front cover, so most players show it without the separate file.

### ISRC codes

An ISRC identifies one recording, which makes it useful for matching tracks across services. `spotify-dl isrc` accepts a song or playlist link, URI or ID, or a song name to search for. Albums are not supported.

```text
spotify-dl isrc https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC
spotify-dl isrc "Daft Punk - One More Time" --csv
spotify-dl isrc <playlist link> --csv my-list.csv --json my-list.json
```

A playlist prints one line per track. `--csv` and `--json` save the result; without a file name they write `<name> - ISRC.csv` or `.json` in the download folder. Put the link or name before the flags. In the terminal UI, **Find ISRC code** does the same and always saves both files for playlists.

The CSV starts with a UTF-8 byte order mark so Excel reads accented names correctly, and cells that start with `=`, `+` or `@` get a leading `'` so spreadsheets do not run them. The JSON has the playlist name and link, counts, and one object per track with `isrc` (or `null`), title, artists, album, release date, duration and Spotify link.

Spotify does not give an ISRC for local files or unavailable items, so those are skipped and counted. Playlists need your Spotify login, and Spotify only lists tracks for playlists you own or collaborate on.

Downloads go to your Music folder in a `spotify-dl` subfolder unless you change it. In the terminal UI, **Settings** lets you pick the folder with Tab completion and enter your Spotify client ID and secret.

Credentials saved through Settings are stored as plain text in `%APPDATA%\spotify-dl\config.json`. A `.env` file or environment variables, if present, override them.

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
| `src/youtube.rs` | YouTube Music and YouTube search through yt-dlp, MP3 extraction |
| `src/matching.rs` | Scores YouTube Music candidates against the Spotify track |
| `src/tags.rs` | ID3 tag and cover art writing |
| `src/art.rs` | Turns cover art into coloured ASCII art |
| `src/format.rs` | Sizes, speeds, durations and progress bars |
| `src/input.rs` | Link, URI and ID parsing |
| `src/isrc.rs` | ISRC lookup results and their CSV and JSON export |
| `src/config.rs` | Saved settings and credentials |
| `src/paths.rs` | Folder completion for the Settings screen |
| `legacy/` | The original Node.js version, kept for reference |
| `docs/` | Landing page, architecture, development and troubleshooting docs |

## Notes

- The Node.js version in `legacy/` is no longer maintained.
- `ffprobe` (part of ffmpeg) is optional. When it is found, the log shows the bitrate, sample rate and channels of each finished MP3.
- Only download music you have the right to download.

## License

Released under the [MIT License](LICENSE).
