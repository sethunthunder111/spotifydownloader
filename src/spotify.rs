//! Minimal Spotify Web API client: client-credentials and user (authorization code) auth,
//! plus the handful of endpoints the downloader needs.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, de::DeserializeOwned};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::config::config_dir;
use crate::jobs::Sink;

const API: &str = "https://api.spotify.com/v1";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const SCOPES: &str = "playlist-read-private playlist-read-collaborative user-library-read";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_RETRIES: u32 = 3;
/// Longest rate-limit wait we sit through before giving up.
const MAX_RETRY_WAIT_SECS: u64 = 60;

// ---------- Models ----------

#[derive(Debug, Clone)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub image_url: Option<String>,
    pub duration_ms: u64,
    pub year: Option<String>,
}

impl Track {
    /// "Artist - Title", the name used for matching, logging and the output file.
    pub fn display_name(&self) -> String {
        let artist = self
            .artists
            .first()
            .map(String::as_str)
            .unwrap_or("Unknown");
        format!("{artist} - {}", self.name)
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub name: String,
    pub image_url: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct Album {
    pub name: String,
    pub image_url: Option<String>,
    pub year: Option<String>,
    pub tracks: Vec<Track>,
}

#[derive(Deserialize)]
struct Image {
    url: String,
}

#[derive(Deserialize)]
struct Artist {
    name: String,
}

#[derive(Deserialize)]
struct AlbumRef {
    name: String,
    #[serde(default)]
    images: Vec<Image>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct TrackObject {
    id: Option<String>,
    name: String,
    #[serde(default)]
    artists: Vec<Artist>,
    album: Option<AlbumRef>,
    #[serde(default)]
    duration_ms: u64,
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    is_local: bool,
}

impl TrackObject {
    fn into_track(self, fallback_album: Option<&Album>) -> Option<Track> {
        if self.is_local || self.kind.as_deref().is_some_and(|k| k != "track") {
            return None;
        }
        let id = self.id?;
        let (album, image_url, year) = match (&self.album, fallback_album) {
            (Some(a), _) => (
                Some(a.name.clone()),
                a.images.first().map(|i| i.url.clone()),
                year_of(a.release_date.as_deref()),
            ),
            (None, Some(alb)) => (
                Some(alb.name.clone()),
                alb.image_url.clone(),
                alb.year.clone(),
            ),
            (None, None) => (None, None, None),
        };
        Some(Track {
            id,
            name: self.name,
            artists: self.artists.into_iter().map(|a| a.name).collect(),
            album,
            image_url,
            duration_ms: self.duration_ms,
            year,
        })
    }
}

fn year_of(release_date: Option<&str>) -> Option<String> {
    release_date.and_then(|d| d.get(..4)).map(str::to_string)
}

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
    next: Option<String>,
}

#[derive(Deserialize)]
struct AlbumResponse {
    name: String,
    #[serde(default)]
    images: Vec<Image>,
    release_date: Option<String>,
}

#[derive(Deserialize)]
struct PlaylistMeta {
    name: String,
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
struct PlaylistEntry {
    // Spotify renamed `track` to `item` in February 2026; accept both.
    item: Option<TrackObject>,
    track: Option<TrackObject>,
}

#[derive(Deserialize)]
struct SearchResponse {
    tracks: Page<TrackObject>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize, serde::Serialize)]
struct StoredRefreshToken {
    refresh_token: String,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

impl CachedToken {
    fn new(value: String, expires_in: u64) -> Self {
        Self {
            value,
            // Refresh a minute early so a token never expires mid-request.
            expires_at: Instant::now() + Duration::from_secs(expires_in.saturating_sub(60)),
        }
    }
    fn is_valid(&self) -> bool {
        Instant::now() < self.expires_at
    }
}

// ---------- Client ----------

pub struct SpotifyClient {
    http: Client,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    sink: Sink,
    app_token: Option<CachedToken>,
    user_token: Option<CachedToken>,
}

impl SpotifyClient {
    /// `sink` receives login progress messages, so they show up in the TUI log pane
    /// instead of being printed over the full-screen UI.
    pub fn new(
        client_id: String,
        client_secret: String,
        redirect_uri: String,
        sink: Sink,
    ) -> Result<Self> {
        let http = Client::builder()
            .user_agent(concat!("spotify-dl/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            client_id,
            client_secret,
            redirect_uri,
            sink,
            app_token: None,
            user_token: None,
        })
    }

    /// Client-credentials token for public data (tracks, albums, search).
    async fn app_token(&mut self) -> Result<String> {
        if let Some(t) = self.app_token.as_ref().filter(|t| t.is_valid()) {
            return Ok(t.value.clone());
        }
        let resp: TokenResponse = self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await?
            .error_for_status()
            .context("Spotify rejected the client credentials; check SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET")?
            .json()
            .await?;
        let token = CachedToken::new(resp.access_token.clone(), resp.expires_in);
        self.app_token = Some(token);
        Ok(resp.access_token)
    }

    /// User token for private data (playlists). Runs the browser login if no refresh token is stored.
    pub async fn user_token(&mut self) -> Result<String> {
        if let Some(t) = self.user_token.as_ref().filter(|t| t.is_valid()) {
            return Ok(t.value.clone());
        }
        let resp = match load_refresh_token().await? {
            Some(refresh) => match self.refresh_user_token(&refresh).await {
                Ok(resp) => resp,
                Err(e) => {
                    self.sink.log(format!(
                        "Stored Spotify login is no longer valid ({e}). Logging in again."
                    ));
                    self.run_login().await?
                }
            },
            None => self.run_login().await?,
        };
        self.store_user_token(resp).await
    }

    /// Forces a fresh browser login (the `login` command).
    pub async fn login(&mut self) -> Result<()> {
        let resp = self.run_login().await?;
        self.store_user_token(resp).await?;
        Ok(())
    }

    async fn store_user_token(&mut self, resp: TokenResponse) -> Result<String> {
        if let Some(new_refresh) = &resp.refresh_token {
            save_refresh_token(new_refresh).await?;
        }
        self.user_token = Some(CachedToken::new(resp.access_token.clone(), resp.expires_in));
        Ok(resp.access_token)
    }

    async fn run_login(&mut self) -> Result<TokenResponse> {
        let (port, callback_path) = self.callback_target()?;
        let state = random_state();
        let url = Url::parse_with_params(
            AUTHORIZE_URL,
            &[
                ("client_id", self.client_id.as_str()),
                ("response_type", "code"),
                ("redirect_uri", self.redirect_uri.as_str()),
                ("scope", SCOPES),
                ("state", state.as_str()),
            ],
        )?;

        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .with_context(|| format!("cannot listen on port {port} (is it in use?)"))?;
        self.sink.log("Opening your browser for Spotify login...");
        self.sink.log(format!("If it does not open, visit:\n{url}"));
        if let Err(e) = open::that(url.as_str()) {
            self.sink
                .log(format!("Could not open the browser automatically: {e}"));
        }

        let code = tokio::time::timeout(
            AUTH_TIMEOUT,
            wait_for_callback(listener, &callback_path, &state),
        )
        .await
        .map_err(|_| anyhow!("timed out waiting for Spotify login (5 minutes)"))??;

        let resp: TokenResponse = self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", self.redirect_uri.as_str()),
            ])
            .send()
            .await?
            .error_for_status()
            .context("Spotify refused the authorization code")?
            .json()
            .await?;
        self.sink.log("Logged in to Spotify.");
        Ok(resp)
    }

    async fn refresh_user_token(&self, refresh: &str) -> Result<TokenResponse> {
        Ok(self
            .http
            .post(TOKEN_URL)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Port and path the local login server must answer on, taken from the redirect URI.
    fn callback_target(&self) -> Result<(u16, String)> {
        let url = Url::parse(&self.redirect_uri)
            .with_context(|| format!("invalid SPOTIFY_REDIRECT_URI: {}", self.redirect_uri))?;
        let port = url
            .port_or_known_default()
            .ok_or_else(|| anyhow!("SPOTIFY_REDIRECT_URI has no port"))?;
        Ok((port, url.path().to_string()))
    }

    /// GET with bounded retries on rate limiting (HTTP 429).
    async fn get<T: DeserializeOwned>(
        &self,
        token: &str,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T> {
        let url = format!("{API}{path}");
        let mut attempt = 0;
        loop {
            let resp = self
                .http
                .get(&url)
                .bearer_auth(token)
                .query(query)
                .send()
                .await?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS {
                let wait = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(2);
                // Spotify can ask for waits of many hours. Fail with a clear message instead
                // of sleeping silently.
                if attempt >= MAX_RETRIES || wait > MAX_RETRY_WAIT_SECS {
                    bail!(
                        "Spotify is rate limiting this app. Try again in about {} minutes.",
                        wait.div_ceil(60)
                    );
                }
                attempt += 1;
                self.sink
                    .progress(format!("Spotify asked to slow down. Waiting {wait} s..."));
                tokio::time::sleep(Duration::from_secs(wait)).await;
                continue;
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                bail!("Spotify API {path} returned {status}: {body}");
            }
            return Ok(resp.json().await?);
        }
    }

    pub async fn track(&mut self, id: &str) -> Result<Track> {
        let token = self.app_token().await?;
        let obj: TrackObject = self.get(&token, &format!("/tracks/{id}"), &[]).await?;
        obj.into_track(None)
            .ok_or_else(|| anyhow!("track {id} is not a playable Spotify track"))
    }

    pub async fn album(&mut self, id: &str) -> Result<Album> {
        let token = self.app_token().await?;
        let meta: AlbumResponse = self.get(&token, &format!("/albums/{id}"), &[]).await?;
        let header = Album {
            name: meta.name,
            image_url: meta.images.first().map(|i| i.url.clone()),
            year: year_of(meta.release_date.as_deref()),
            tracks: Vec::new(),
        };
        let mut tracks = Vec::new();
        let mut offset = 0u32;
        loop {
            let page: Page<TrackObject> = self
                .get(
                    &token,
                    &format!("/albums/{id}/tracks"),
                    &[("limit", "50".into()), ("offset", offset.to_string())],
                )
                .await?;
            let more = page.next.is_some();
            tracks.extend(
                page.items
                    .into_iter()
                    .filter_map(|t| t.into_track(Some(&header))),
            );
            if !more {
                break;
            }
            offset += 50;
        }
        Ok(Album { tracks, ..header })
    }

    pub async fn playlist(&mut self, id: &str) -> Result<Playlist> {
        let token = self.user_token().await?;
        // The name and cover are only used for the folder and display, so a failure here is
        // not fatal: the track list below reports any real access problem.
        let meta: Option<PlaylistMeta> = self
            .get(
                &token,
                &format!("/playlists/{id}"),
                &[("fields", "name,images(url)".into())],
            )
            .await
            .ok();
        let mut tracks = Vec::new();
        let mut offset = 0u32;
        loop {
            let page: serde_json::Value = self
                .get(
                    &token,
                    &format!("/playlists/{id}/items"),
                    &[("limit", "50".into()), ("offset", offset.to_string())],
                )
                .await?;
            let items = page.get("items").ok_or_else(|| {
                anyhow!(
                    "Spotify returned no items; you may not own or collaborate on this playlist"
                )
            })?;
            let entries: Vec<PlaylistEntry> = serde_json::from_value(items.clone())?;
            let more = page.get("next").is_some_and(|n| !n.is_null());
            tracks.extend(
                entries
                    .into_iter()
                    .filter_map(|e| e.item.or(e.track).and_then(|t| t.into_track(None))),
            );
            if !more {
                break;
            }
            offset += 50;
        }
        let (name, image_url) = match meta {
            Some(m) => (m.name, m.images.first().map(|i| i.url.clone())),
            None => (format!("Playlist {id}"), None),
        };
        Ok(Playlist {
            name,
            image_url,
            tracks,
        })
    }

    /// Returns the best search hit for a free-text query.
    pub async fn search_track(&mut self, query: &str) -> Result<Option<Track>> {
        let token = self.app_token().await?;
        let resp: SearchResponse = self
            .get(
                &token,
                "/search",
                &[
                    ("q", query.to_string()),
                    ("type", "track".into()),
                    ("limit", "5".into()),
                ],
            )
            .await?;
        Ok(resp
            .tracks
            .items
            .into_iter()
            .find_map(|t| t.into_track(None)))
    }
}

// ---------- Login callback & token storage ----------

async fn wait_for_callback(
    listener: TcpListener,
    callback_path: &str,
    expected_state: &str,
) -> Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = stream.read(&mut buf).await?;
        let request = String::from_utf8_lossy(&buf[..n]);
        let target = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("");
        if target.split('?').next() != Some(callback_path) {
            respond(&mut stream, "404 Not Found", "Not found.").await;
            continue;
        }

        let url = Url::parse(&format!("http://127.0.0.1{target}"))?;
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let result = if let Some(err) = params.get("error") {
            Err(anyhow!("Spotify login was not completed: {err}"))
        } else if params.get("state").map(String::as_str) != Some(expected_state) {
            Err(anyhow!("login state mismatch; please try again"))
        } else {
            params
                .get("code")
                .cloned()
                .ok_or_else(|| anyhow!("Spotify did not return an authorization code"))
        };

        match &result {
            Ok(_) => {
                respond(
                    &mut stream,
                    "200 OK",
                    "Spotify login complete. You can close this window.",
                )
                .await
            }
            Err(_) => {
                respond(
                    &mut stream,
                    "400 Bad Request",
                    "Login failed. Check the terminal.",
                )
                .await
            }
        }
        return result;
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let html =
        format!("<!doctype html><meta charset=\"utf-8\"><title>spotify-dl</title><p>{body}</p>");
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn random_state() -> String {
    use std::hash::{BuildHasher, RandomState};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:016x}", RandomState::new().hash_one(nanos))
}

fn token_file() -> std::path::PathBuf {
    config_dir().join("refresh_token.json")
}

async fn load_refresh_token() -> Result<Option<String>> {
    match tokio::fs::read_to_string(token_file()).await {
        Ok(text) => Ok(serde_json::from_str::<StoredRefreshToken>(&text)
            .ok()
            .map(|t| t.refresh_token)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

async fn save_refresh_token(token: &str) -> Result<()> {
    let path = token_file();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let text = serde_json::to_string_pretty(&StoredRefreshToken {
        refresh_token: token.to_string(),
    })?;
    tokio::fs::write(&path, text)
        .await
        .with_context(|| format!("cannot save refresh token to {}", path.display()))
}
