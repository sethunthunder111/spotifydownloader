# Architecture

spotify-dl turns a Spotify link into a tagged MP3 in four steps: resolve the Spotify metadata, find a matching YouTube video, extract audio, and write tags.

```
            ┌──────────────┐   Track list    ┌────────────┐
 CLI args ─▶│  main.rs     │───────────────▶ │  jobs.rs   │──▶ Sink (stdout | TUI log)
 TUI input ▶│  tui.rs      │   Job           │            │
            └──────────────┘                 └─────┬──────┘
                                     ┌─────────────┼──────────────┐
                                     ▼             ▼              ▼
                               spotify.rs     youtube.rs      tags.rs
                               (Web API,      (yt-dlp,        (id3 + cover
                                OAuth)         ffmpeg)         art)
```

## Entry points

`main.rs` parses the CLI with clap. It loads `.env` from the executable's folder and then from the current folder. It also resolves the download folder, which comes from `-p`, then the saved setting, then the default.

- With no subcommand, it calls `tui::run`.
- With a subcommand, it builds a `Job` and calls `jobs::run_job` with a `Sink::Stdout`.
- `settings` and `login` are handled directly.

## Jobs

A `Job` is one user request: `Song`, `Album`, `Playlist`, `Search`, `File`, or `Login`.

`run_job` does the following:

1. Creates a `SpotifyClient` for this job. Tokens are cached in memory and the refresh token is stored on disk.
2. Resolves the job into a list of `Track` values.
3. Calls `download_one` for each track, logging progress as it goes.

Album and playlist jobs continue after a failed track and report a total at the end. A single-track job returns the error.

## Output: `Sink`

`Sink` is either stdout (CLI) or an unbounded channel (TUI). Shared code only calls `sink.log(...)`. The TUI drains the channel on every frame, so jobs never block the UI.

## Terminal UI

`tui.rs` uses ratatui with crossterm.

- The screen has three areas: a header with the download folder, a menu on the left, and a log pane on the right. A footer shows either the input prompt or a status line.
- Jobs run in `tokio::spawn`. The UI loop polls input every 100 ms.
- Only one job runs at a time. Starting a second shows a message instead.
- Quitting aborts a running job.

## Spotify (`spotify.rs`)

Two auth modes are used:

| Mode | Used for | Stored where |
|------|----------|--------------|
| Client credentials | tracks, albums, search | in memory |
| Authorization code (user) | playlists | `%APPDATA%\spotify-dl\refresh_token.json` |

The login flow:

1. Binds a TCP listener on `127.0.0.1` at the port from `SPOTIFY_REDIRECT_URI`.
2. Opens the browser to Spotify's authorize URL with a random `state`.
3. Waits up to 5 minutes for the callback, checks `state`, and reads `code`.
4. Exchanges the code for tokens and saves the refresh token.

On later runs, the stored refresh token is used. If Spotify rejects it, the login flow runs again.

HTTP 429 responses are retried up to 3 times using the `Retry-After` header.

## YouTube (`youtube.rs`)

- `find_match` runs `yt-dlp --flat-playlist --dump-single-json ytsearch5:<query>` and returns the first result whose duration is within 30 seconds of the Spotify track.
- `download_mp3` runs `yt-dlp --extract-audio --audio-format mp3 --audio-quality 0` into a temp folder, named by video ID.
- Both commands look for `yt-dlp.exe` or `ffmpeg.exe` next to the executable first, then on PATH.

## Downloading one track (`jobs::download_one`)

1. Build the query: `Artist - Title`, with `'`, `"`, `,`, `.`, and `/` removed.
2. Find the YouTube match. No match within tolerance is an error.
3. Download the MP3 into `%TEMP%\spotify-dl\<track id>`.
4. Move it to `<download folder>\<sanitized name>.mp3`. Rename is tried first, with copy as the fallback across drives.
5. Write tags. A tag failure is logged as a warning and does not fail the track.
6. Delete the temp folder.

## Tags (`tags.rs`)

Writes ID3v2.4 with title, artist (joined with `/`), album, year, and front cover. The cover is downloaded from the Spotify image URL.

## Configuration

| Item | Location |
|------|----------|
| Saved download folder | `%APPDATA%\spotify-dl\config.json` |
| Refresh token | `%APPDATA%\spotify-dl\refresh_token.json` |
| Credentials | `.env` next to the exe, then `.env` in the current folder, then environment variables |
