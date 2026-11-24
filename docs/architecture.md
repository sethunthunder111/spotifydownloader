# Architecture

spotify-dl turns a Spotify link into a tagged MP3 in four steps: resolve the Spotify metadata, find the matching recording on YouTube Music (or YouTube), extract audio, and write tags.

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
- `settings` is handled directly. `login` runs as a job.
- Missing Spotify credentials do not stop startup. `run_job` reports them, so the TUI can open and let the user enter them in Settings.

## Jobs

A `Job` is one user request: `Song`, `Album`, `Playlist`, `Search`, `File`, or `Login`.

`run_job` does the following:

1. Creates a `SpotifyClient` for this job. Tokens are cached in memory and the refresh token is stored on disk.
2. Resolves the job into a list of `Track` values.
3. Picks the output folder. Songs, searches and file jobs use the download folder. An album uses `Artist - Album` (the first track's first artist) and a playlist uses its name, both shortened to about 100 characters and made safe for Windows. The job's cover is saved there as `cover.jpg` or `cover.png`, unless one exists.
4. Calls `download_all`, which runs `download_one` for each track, up to `parallel` at a time (a `Semaphore` limits it and a `JoinSet` owns the tasks, so quitting the TUI aborts them).

A failed track is logged and the others continue; a total is reported at the end. A single-track job returns the error. A track whose target file already exists is skipped.

Cover art is fetched once per URL through a cache shared by all tracks of a job. Each track embeds its own art. The job also shows one cover as ASCII art: the track's for a song or search, the album's for an album, the playlist's for a playlist, and none for a file job.

## Output: `Sink`

`Sink` is either stdout (CLI) or an unbounded channel (TUI). Shared code only calls the sink and never prints directly. The TUI drains the channel on every frame, so jobs never block the UI.

Events: `Log`, `Progress`, `Track(TrackStatus)` (stage, transfer numbers and stream details of one active track), `TrackDone`, `Overall { done, failed, total }` and `Cover`. The CLI turns the same calls into log lines plus one status line, redrawn with `\r`: overall bar, percent, total speed, MB done and total, and tracks done and total. It prints the cover art once at the start of a job when stdout is a terminal, in colour unless `NO_COLOR` is set.

## Terminal UI

`tui.rs` uses ratatui with crossterm.

- The screen has three areas: a header with the download folder, a menu on the left, and a log pane on the right. A footer shows either the input prompt or a status line.
- While a job runs, a "Now downloading" panel appears above the log. With a right column of at least 22 rows it is 14 rows tall and has the ASCII cover on the left; from 14 rows it is 8 rows without art; below that it is hidden. It shows an overall bar and, for each active track, its name and stage, then a bar, percent, speed, MB done, ETA and stream details.
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

- `find_match` first searches YouTube Music Songs (`https://music.youtube.com/search?q=...#songs` with `--flat-playlist`, which returns only IDs and titles). It fetches full metadata (`track`, `artists`, `album`, `duration`) for the 4 results with the closest titles and scores each with `matching::score` out of 5, using title, artists, album and length. A candidate must score at least 3.0 and be within 7 seconds of the Spotify length. If none qualifies, it falls back to `ytsearch5:` on plain YouTube, accepting the first result within 30 seconds, and the log says so.
- `download_mp3` runs `yt-dlp --extract-audio --audio-format mp3 --audio-quality 0` into a temp folder, named by video ID. For a YouTube Music match it reuses the metadata already fetched (`--load-info-json`), and retries by URL if that fails. A custom `--progress-template` prints the bytes, speed, ETA and stream details that become `Step::Downloading`; `Step::Converting` marks the ffmpeg step.
- `probe_mp3` runs `ffprobe` on the finished file for bitrate, sample rate, channels and length. It is optional. The MP3 is VBR V0 (about 240 kbps), so it is usually bigger than the roughly 130 kbps source stream, and the log shows both.
- A global semaphore allows at most 4 yt-dlp metadata lookups at once, however many tracks download in parallel.
- Every call uses `--force-ipv4 --socket-timeout 20`, because yt-dlp can hang on some IPv6 networks.
- Commands look for `yt-dlp.exe`, `ffmpeg.exe` and `ffprobe.exe` next to the executable first, then on PATH.

## Downloading one track (`jobs::download_one`)

1. If `<folder>\<sanitized name>.mp3` already exists, log it and skip the track.
2. Find the match (YouTube Music, then YouTube). No match is an error.
3. Download the MP3 into `%TEMP%\spotify-dl\<track id>-<position>`.
4. Move it to the target. Rename is tried first, with copy as the fallback across drives.
5. Write tags and the cover from the shared cache. A tag failure is logged as a warning and does not fail the track.
6. Delete the temp folder.

Each track logs lines prefixed with `[i/N]`: the header, `match:` (title, artists, album, length, source, score), `source:` (container and codec, kbps, kHz, channels, size), `saved:` and `mp3:` (probe results, time taken and average speed).

## ISRC lookup (`isrc.rs`, `Job::Isrc`)

`spotify-dl isrc` reads the ISRC from each track's `external_ids`. `input::detect` decides what the input is: a track or playlist link, a bare ID (tried as a track first, then as a playlist) or text to search for. Playlist responses can leave `external_ids` out, so `SpotifyClient::fill_isrcs` looks up any missing ones with `/tracks?ids=`, 50 at a time. The job never calls yt-dlp or ffmpeg. `isrc.rs` builds the `Listing` and writes it as CSV (UTF-8 with a byte order mark, formula-safe cells) or JSON.

## Tags (`tags.rs`)

Writes ID3v2.3 with title, artist (joined with `/`), album, year, and front cover. The cover is passed in as bytes (tags does no network access) and its MIME type is read from the image's first bytes. It is stored in an `APIC` front-cover frame; it is never a separate file, apart from the `cover.jpg` saved in album and playlist folders.

## Cover art and formatting (`art.rs`, `format.rs`)

`art::Cover` decodes JPEG or PNG (the `image` crate), keeps a 64-pixel copy, and renders it at any size as characters picked by brightness plus an RGB colour per cell. The TUI draws about 2x as many columns as rows, because terminal cells are tall. `format.rs` holds the byte, speed, duration, bar, kHz and channel formatting used by the CLI line, the TUI panel and the log.

## Configuration

| Item | Location |
|------|----------|
| Saved settings (download folder, client ID and secret, parallel downloads) | `%APPDATA%\spotify-dl\config.json` |
| Refresh token | `%APPDATA%\spotify-dl\refresh_token.json` |
| Credentials | `.env` next to the exe, then `.env` in the current folder, then environment variables. If none are set, the client ID and secret saved in `config.json` through Settings. |
| Config folder override | Set `SPOTIFY_DL_CONFIG_DIR` to use another folder for `config.json` and `refresh_token.json` |
