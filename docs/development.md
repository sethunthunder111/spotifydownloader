# Development

## Prerequisites

- Rust stable (edition 2024, Rust 1.88 or newer for let-chains)
- `yt-dlp` and `ffmpeg` on PATH, for running real downloads
- A Spotify developer app, with `http://127.0.0.1:5000/callback` as a redirect URI

## Setup

```powershell
git clone https://github.com/sethunthunder111/spotifydownloader
cd spotifydownloader
copy .env.example .env     # then fill in SPOTIFY_CLIENT_ID and SPOTIFY_CLIENT_SECRET
cargo run
```

## Day-to-day commands

```powershell
cargo run -- song <link>           # run a subcommand from source
cargo test                         # unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all
cargo build --release              # target\release\spotify-dl.exe
```

## Tests

Unit tests cover pure logic:

- `input.rs`: link, URI, and ID parsing, including locale prefixes and wrong-kind rejection.
- `jobs.rs`: filename sanitizing.

Network paths (Spotify, YouTube, tagging) have no automated tests. Verify them by hand with a real link:

```powershell
cargo run -- song https://open.spotify.com/track/<id>
cargo run -- query "Artist - Title"
cargo run -- settings --set "$env:TEMP\spotify-dl-test"
```

Use a throwaway download folder so you do not overwrite your music library.

## Adding a subcommand

1. Add a variant to `Command` in `src/main.rs`.
2. Map it to a `Job` variant in `run`.
3. Add a `Job` variant in `src/jobs.rs` and handle it in `run_job`.
4. Add a menu entry in `src/tui.rs` (`MENU`, `Action`, `submit`, `prompt`).
5. Document it in `README.md` and `docs/index.html`.

## Releasing

1. Bump `version` in `Cargo.toml`.
2. Commit, then tag: `git tag v0.1.1 && git push origin v0.1.1`.
3. `.github/workflows/release.yml` builds the exe and attaches `spotify-dl-windows-x64.zip` to a GitHub release.

## Troubleshooting the dev loop

- **`could not run yt-dlp`**: install it, or copy `yt-dlp.exe` into `target\debug` or `target\release`.
- **`cannot listen on port 5000`**: another process owns the port. Change `SPOTIFY_REDIRECT_URI` and the Spotify app setting to match.
- **Clippy fails in CI but not locally**: run `cargo clippy --all-targets -- -D warnings`. CI uses the same flags.
- **`cargo fmt --check` fails in CI**: run `cargo fmt --all` and commit.
