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
| `src/jobs.rs` | download pipeline shared by CLI and TUI (folders, parallel downloads, per-track log); `Sink` for output |
| `src/spotify.rs` | Spotify Web API client, client-credentials and user auth, login callback |
| `src/youtube.rs` | `yt-dlp` search (YouTube Music first, YouTube fallback), MP3 extraction, `ffprobe` |
| `src/matching.rs` | scores YouTube Music candidates against the Spotify track (has unit tests) |
| `src/tags.rs` | ID3v2.4 tags and cover art (`id3` crate); takes cover bytes, no network |
| `src/art.rs` | cover image to coloured ASCII art (`image` crate) |
| `src/format.rs` | bytes, speed, duration, bar, kHz and channel formatting |
| `src/input.rs` | parses links, URIs and bare IDs; `detect` tells a track link, playlist link, bare ID and search text apart (has unit tests) |
| `src/isrc.rs` | ISRC lookup results (`Listing`) and CSV/JSON export (has unit tests) |
| `src/config.rs` | saved settings in `%APPDATA%\spotify-dl\config.json` (download folder, credentials, `parallel` 1 to 8) |
| `docs/` | landing page (`index.html`) and developer docs |
| `dist/` | local release copy of the exe (gitignored) |

See `docs/architecture.md` for how the pieces fit together.

## Conventions

- Errors: use `anyhow` with `.context(...)` so messages say what failed and where.
- yt-dlp calls always pass `--force-ipv4 --socket-timeout 20`. Metadata lookups are capped at 4 at once by the `LOOKUPS` semaphore in `youtube.rs`.
- Albums go in `Artist - Album` subfolders and playlists in a subfolder named after them (see `jobs.rs`). Songs, searches and file jobs use the download folder.
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
- Repo owner: `sethunthunder111`. License: MIT. Last release: `v0.1.2`.
