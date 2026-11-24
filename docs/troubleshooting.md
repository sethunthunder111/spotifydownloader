# Troubleshooting

## Setup

**`Spotify client ID and secret are not set`**
Open **Settings** in the app and enter both values. Or create `.env` next to `spotify-dl.exe` (copy `.env.example`) and fill them in, or set them as environment variables. From the command line: `spotify-dl settings --client-id <id> --client-secret <secret>`.

**I changed the client ID in Settings but nothing changed**
A `.env` file or an environment variable overrides saved values. Settings shows `(from .env or environment)` next to a value that is being overridden. Edit or delete that `.env` entry.

**`Spotify rejected the client credentials`**
The client ID or secret is wrong. Copy them again from the Spotify developer dashboard. Rotate the secret there if it was ever shared.

**Browser login says the redirect URI is not allowed**
Add `http://127.0.0.1:5000/callback` under Redirect URIs in your Spotify app settings. Use `127.0.0.1`, not `localhost`. If you set `SPOTIFY_REDIRECT_URI`, the two values must match exactly.

**`cannot listen on port 5000 (is it in use?)`**
Another program is using the port. Close it, or change `SPOTIFY_REDIRECT_URI` to another port and update the Spotify app to match.

## Downloads

**`could not run yt-dlp` / `could not run ffmpeg`**
Install them:

```powershell
winget install yt-dlp.yt-dlp
winget install Gyan.FFmpeg
```

Or place `yt-dlp.exe` and `ffmpeg.exe` in the same folder as `spotify-dl.exe`. Open a new terminal after installing so PATH is refreshed.

**`no YouTube Music or YouTube match ... with a similar length`**
YouTube Music had no song scoring at least 3.0 out of 5, and plain YouTube had nothing within 30 seconds of the Spotify length. This avoids wrong uploads, but it also skips some tracks, such as live versions. Search by hand, then download the URL with `yt-dlp` if you need that exact version.

**`no good YouTube Music match, using a plain YouTube result instead`**
Not an error. The track is not on YouTube Music as a song, or its title, artist or length did not match, so the first plain YouTube result of a similar length is used. Those are often music videos, which may have intros or different mixes.

**Downloads are slow or fail with several at once**
Lower the number of parallel downloads with `spotify-dl settings --parallel 1`, or use `-j 1` for one run.

**Where is the album art?**
It is embedded in every MP3 as the front cover. Albums and playlists also get a `cover.jpg` (or `cover.png`) in their folder. The ASCII art in the terminal is only a preview.

**`yt-dlp failed: ...`**
YouTube changes often break yt-dlp. Update it:

```powershell
yt-dlp -U
```

**A downloaded file has no cover art or tags**
Tagging failed after the MP3 was saved. The log shows `warning: could not write tags`. Check that the folder is writable and the file is not open in another program.

## Playlists

**`Spotify returned no items; you may not own or collaborate on this playlist`**
Spotify only returns track lists for playlists you own or collaborate on. Other playlists return metadata only. Use an album or search instead.

**Playlist login keeps opening the browser**
The saved refresh token was rejected, so spotify-dl logs in again each time. Run `spotify-dl login` once. If it keeps happening, delete `%APPDATA%\spotify-dl\refresh_token.json` and log in again.

**`Spotify API ... returned 403`**
Your Spotify app may not have access to that endpoint. Check that the account owns your developer app and that the app is in a mode that allows the request.

## Settings

**Files go to the wrong folder**
Check the active folder with `spotify-dl settings`. `-p` overrides the saved setting for one run only. Change the saved setting with `spotify-dl settings --set "<folder>"`.

## Reset

To reset everything:

1. Delete `%APPDATA%\spotify-dl\` (config and login token).
2. Log in again with `spotify-dl login` if you use playlists.
