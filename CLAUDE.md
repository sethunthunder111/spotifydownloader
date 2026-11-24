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

- `.env`, `.spotify_refresh_token.secret.json`, and anything under `%APPDATA%\spotify-dl` hold credentials. Never print, commit, or paste their values.
- `.env.example` is the only env file that belongs in git.

## Git and release

- Do not commit or push unless the user asks.
- Release builds are produced by `.github/workflows/release.yml` on `v*` tags. Do not create tags without being asked.
- Repo owner: `sethunthunder111`. License: MIT.
