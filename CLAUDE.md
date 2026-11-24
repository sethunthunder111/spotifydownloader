# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

`spotify-dl` is a Rust CLI and terminal UI that downloads Spotify tracks, albums and playlists as tagged MP3s. It finds each track on YouTube through `yt-dlp` and converts it with `ffmpeg`. Windows is the primary target.

The original Node.js version lives in `legacy/` and is no longer maintained. Do not edit it.

## Commands

```powershell
cargo build --release           # target\release\spotify-dl.exe
cargo run -- --help             # run from source
cargo test                      # unit tests (input parsing, filename sanitizing)
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

CI (`.github/workflows/ci.yml`) runs fmt check, clippy with `-D warnings`, and tests on Windows. Keep all three green.

## Layout

| File | Role |
|------|------|
| `src/main.rs` | clap CLI, `.env` loading, dispatch to TUI or a job |
| `src/tui.rs` | ratatui full-screen UI (runs when no subcommand is given) |
| `src/jobs.rs` | download pipeline shared by CLI and TUI; `Sink` for output |
| `src/spotify.rs` | Spotify Web API client, client-credentials and user auth, login callback |
| `src/youtube.rs` | `yt-dlp` search and MP3 extraction |
| `src/tags.rs` | ID3v2.4 tags and cover art (`id3` crate) |
| `src/input.rs` | parses links, URIs and bare IDs (has unit tests) |
| `src/config.rs` | saved settings in `%APPDATA%\spotify-dl\config.json` |
| `docs/` | landing page (`index.html`) and developer docs |
| `dist/` | local release copy of the exe (gitignored) |

See `docs/architecture.md` for how the pieces fit together.

## Conventions

- Errors: use `anyhow` with `.context(...)` so messages say what failed and where.
- Output from jobs goes through `Sink`, never straight to `println!` in shared code. This keeps the TUI log pane working.
- Spotify playlist items use `/playlists/{id}/items` and accept both `item` and `track` keys. Spotify renamed these in February 2026.
- Search limit is capped at 10 by Spotify. The code uses 5.
- The OAuth redirect uses `127.0.0.1`, not `localhost`. The default port comes from `SPOTIFY_REDIRECT_URI`.
- Crate APIs (`id3`, `ratatui`, `reqwest`): check current docs through Context7 before changing calls. `add_picture` is deprecated in `id3`; use `add_frame`.

## Secrets

- `.env`, `.spotify_refresh_token.secret.json`, and anything under `%APPDATA%\spotify-dl` hold credentials. Never print, commit, or paste their values. Print key names and lengths only.
- `.env.example` is the only env file that belongs in git.
- `dirs::config_dir()` ignores the `APPDATA` environment variable. Always set `SPOTIFY_DL_CONFIG_DIR` to a scratch folder when testing settings, login or the TUI. A test run once overwrote the real `config.json`. Never change the saved `download_path`.

## Git and release

- Do not commit or push unless the user asks.
- Release builds are produced by `.github/workflows/release.yml` on `v*` tags. Do not create tags without being asked.
- Never add `Co-Authored-By` or "Generated with Claude Code" lines to commits or PRs (global user rule).
- This file is tracked in git. The HANDOFF section at the end is temporary: delete it when that work is finished.
- Repo owner: `sethunthunder111`. License: MIT. Last release: `v0.1.2`.

---

## HANDOFF: unfinished work in the working tree

Everything in this section is on branch `wip/youtube-music`, on top of `v0.1.2`. Check that branch out first; `main` does not contain this work. **The build on that branch is broken.** New and rewritten files exist, but their callers were not updated and nothing has been compiled yet. Finish the steps below, then delete this section. When done, merge the branch into `main` only if the user asks.

### The task (from the user)

1. Verbose details while downloading: progress bars, speed, MB done, bitrate, channels and similar stream details.
2. ASCII art of the album art. The user also asked where the album art is. It is embedded in each MP3 as an ID3v2.4 front-cover `APIC` frame and was never a separate file. Also save `cover.jpg` or `cover.png` in album and playlist folders.
3. Albums and playlists each get their own subfolder under the main downloads folder, and their tracks download into it.
4. Parallel downloading.
5. Match the original Spotify song using YouTube Music, not plain YouTube.

### State of each file

| File | State |
|------|-------|
| `Cargo.toml` | Added `image = 0.25.10` (jpeg and png only). |
| `src/format.rs` (new) | Done, with tests: bytes, rate, duration, bar, kHz, channels. Not yet declared with `mod` in `main.rs`. |
| `src/art.rs` (new) | Done, with tests: `Cover::decode(bytes)` and `Cover::ascii(cols, rows) -> Vec<Vec<Cell{ch, rgb}>>`. Not yet declared with `mod`. |
| `src/matching.rs` (new) | Done, with tests: `score(query, candidate, tolerance)` and `title_similarity`. Not yet declared with `mod`. |
| `src/youtube.rs` | Fully rewritten (API below). Not compiled. `jobs.rs` still calls the old API. |
| `src/spotify.rs` | `playlist()` now returns `Playlist { name, image_url, tracks }`. `jobs.rs` still expects `Vec<Track>`. |
| `src/tags.rs`, `src/config.rs`, `src/jobs.rs`, `src/tui.rs`, `src/main.rs`, docs | Not changed yet. |

### New `youtube.rs` API

- Types: `TrackQuery { title, artists, album, duration_ms }`, `Match { id, title, artists, album, duration_secs, source: Source::{YouTubeMusic, YouTube}, score, .. }` with `Match::url()`.
- Progress: `Transfer { downloaded, total, speed, eta_secs }`, `SourceFormat { container, codec, bitrate_kbps, sample_rate, channels }`, `Step::Downloading(Transfer, SourceFormat)`, `Step::Converting`.
- Functions: `find_match(&TrackQuery) -> Result<Option<Match>>`, `download_mp3(&Match, dir, on_step)`, `probe_mp3(path) -> Option<Mp3Info>` (uses `ffprobe`, optional), `check_tools()`.
- Behaviour:
  - Searches YouTube Music Songs, fetches full metadata for the top 4 candidates, and scores them.
  - Falls back to plain YouTube if nothing scores at least 3.0 out of 5. The log must say when this happens.
  - Reuses the saved metadata with `--load-info-json` for the download, retrying by URL if that fails.
  - A global `LOOKUPS` semaphore caps concurrent yt-dlp metadata calls at 4.

### Verified facts (do not re-derive)

- YouTube Music search `https://music.youtube.com/search?q=…#songs` works with `yt-dlp --flat-playlist --dump-single-json`, but returns only `id` and `title`. The `ytmsearch` prefix is not supported.
- `yt-dlp --dump-json --skip-download https://music.youtube.com/watch?v=<id>` returns `track`, `artists`, `album`, `duration` and `uploader` (such as `X - Topic`). Example: `fa5IWHDbftI` is Daft Punk "One More Time", album Discovery, 320 s.
- Progress template: `%(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s %(progress.eta)s %(info.ext)s %(info.acodec)s %(info.abr)s %(info.asr)s %(info.audio_channels)s` gives lines like `1024 5316281 NA 373832.99 14 webm opus 132.748 48000 2`. `NA` means unknown.
- `ffprobe` is on PATH. Its JSON gives `bit_rate`, `sample_rate`, `channels`, `duration` and `size`. The MP3 is V0 (about 243 kbps) while the source is about 133 kbps opus, so the file is bigger than the source. Show both numbers.
- yt-dlp hangs on some networks over IPv6, so every call uses `--force-ipv4 --socket-timeout 20`.

### Design decisions already made (follow them)

**Events (`jobs.rs`)**
- `Event::{Log(String), Progress(String), Track(TrackStatus), TrackDone(id), Overall{done, failed, total}, Cover(Arc<Cover>)}`.
- `TrackStatus { id, label, stage: Searching|Downloading|Converting|Tagging, transfer, format }`.

**Sink**
- Replace `Sink::Stdout` with `Sink::Stdout(Arc<Mutex<CliView>>)` and a `Sink::stdout()` constructor. `main.rs` currently uses the bare variant and must change.
- The CLI shows one aggregated status line with `\r`: bar, percent, total speed, MB done and total, and tracks done and total.
- The CLI prints the cover art once at job start, only if stdout is a terminal. On Windows enable VT mode with `ratatui::crossterm::ansi_support::supports_ansi()`. Respect `NO_COLOR`.

**Cover display**
- Per job, not per track:
  - Song or search shows the track's art.
  - Album shows the album art.
  - Playlist shows the playlist cover.
  - File shows none.
- Per-track tags still embed each track's own art, through a shared cache (a tokio `Mutex<HashMap<url, Option<Arc<Vec<u8>>>>>` inside an `Arc<Ctx>{http, sink, covers}`).
- Save the cover in album and playlist folders, skipping it if it already exists.

**Folders**
- Album: `base/sanitize("Artist - Album")`, using the first track's first artist.
- Playlist: `base/sanitize(playlist name)`.
- Trim names to about 100 characters.
- Song, search and file jobs go in `base`.
- Skip a track whose target file already exists, and log that.

**Parallelism**
- Config field `parallel`, default 3, clamped to 1–8. CLI global flag `-j/--jobs N` and `settings --parallel N`. TUI Settings field "Parallel downloads", which makes the settings list 5 items.
- Use a `Semaphore` plus `JoinSet` in `download_all`. Dropping the `JoinSet` aborts the tasks when the TUI quits. `kill_on_drop` already covers child processes.

**Log lines per track**
- All prefixed `[i/N]`: header, match (title, artists, album, length, source, score), source (container/codec, kbps, kHz, channels, size), `saved: <path>`, mp3 (probe: kbps, kHz, channels, size, length, time taken, average speed).
- Change `log_style` in `tui.rs` to use `contains("saved:")` and similar, because lines no longer start with those words.

**TUI layout**
- Split the right column: a "Now downloading" panel on top and the log below.
  - Panel height about 14 rows if the right column is at least 22 rows tall.
  - About 8 rows with no art if it is at least 14 rows tall.
  - No panel if it is shorter.
- Art goes on the left at about 2×rows columns (for example 24×12). Re-render with `Cover::ascii` per size and colour with `Color::Rgb`.
- On the right of the art: an overall bar, then a two-line entry per active track (name and stage; then bar, percent, speed, downloaded and total, ETA, format).

### Remaining steps, in order

1. Add `mod art; mod format; mod matching;` to `main.rs`.
2. `tags.rs`: change to `write(path, track, cover: Option<&[u8]>)`, with no network, and detect the MIME type from magic bytes.
3. `config.rs`: add the `parallel` getter and setter, with a test.
4. Rewrite `jobs.rs` per the design above. Remove the old `minutes` helper in favour of `format::duration`. Keep `sanitize_filename` and its test, and add folder-name tests.
5. `main.rs`: use `Sink::stdout()`, add the `-j` flag and `settings --parallel`, put `parallel` into `Settings`, and show it in the `settings` output.
6. `tui.rs`: handle the new events, add the panel, and add the Parallel setting.
7. Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test`.
8. Test for real (below).
9. Update README, `docs/` and the landing page (`docs/index.html`): YouTube Music matching, parallel downloads, per-album folders, verbose details, ASCII cover art, the `-j` flag. Update the layout table in this file.
10. Ask the user before committing, pushing or tagging. The next version would be `0.1.3`. Then delete this HANDOFF section.

### How to test

- **Song, end to end.** Run from the project root so `.env` supplies the credentials:
  `cargo run -q -- -p <temp folder> song https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC`
  Check that the log says YouTube Music, the detail lines look right, and the MP3 has title, artist, album, `TDRC` year and a cover (use `uv run --with mutagen`).
- **Album.** Use a short album with `-j 3`. Check the subfolder, the cover file, parallel progress, and that nothing hangs.
- **Fallback and rejection.** Try an obscure track that YouTube Music lacks, and check the log says it fell back to YouTube.
- **Playlist.** Needs the user's own playlist and a login. Say so if you cannot test it.
- **TUI.** Drive it with `uv run --with pywinpty --with pyte` and isolate the config with `SPOTIFY_DL_CONFIG_DIR`. Notes:
  - The shell tool layer collapses `\\` into `\`, so use `chr(92)` for backslashes in scripts.
  - `\x7f` works as Backspace.
  - Wait about 4 seconds after typing long strings.
  - Also try a small terminal (for example 100×20) to confirm the panel degrades gracefully.
