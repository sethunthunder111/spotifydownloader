//! Saved settings: download folder and Spotify credentials.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Fewest and most tracks downloaded at the same time.
pub const MIN_PARALLEL: usize = 1;
pub const MAX_PARALLEL: usize = 8;
const DEFAULT_PARALLEL: usize = 3;

/// Name of the per-user folder used for config and the stored Spotify refresh token.
const APP_DIR_NAME: &str = "spotify-dl";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    download_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parallel: Option<usize>,
}

/// Where a credential comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An environment variable or `.env` file. These win over saved values.
    Env,
    /// Saved in the config file through Settings.
    Saved,
    Missing,
}

/// User settings persisted as JSON in the per-user config directory.
#[derive(Debug)]
pub struct Config {
    file: PathBuf,
    data: ConfigFile,
}

impl Config {
    pub fn load() -> Self {
        let file = config_dir().join("config.json");
        let data = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self { file, data }
    }

    pub fn config_file(&self) -> &Path {
        &self.file
    }

    /// Directory finished MP3s are written to.
    pub fn download_dir(&self) -> PathBuf {
        self.data
            .download_path
            .clone()
            .unwrap_or_else(default_download_dir)
    }

    pub fn set_download_dir(&mut self, path: PathBuf) -> Result<()> {
        let absolute = std::path::absolute(&path)
            .with_context(|| format!("invalid path: {}", path.display()))?;
        std::fs::create_dir_all(&absolute)
            .with_context(|| format!("cannot create {}", absolute.display()))?;
        self.data.download_path = Some(absolute);
        self.save()
    }

    /// How many tracks are downloaded at the same time.
    pub fn parallel(&self) -> usize {
        clamp_parallel(self.data.parallel.unwrap_or(DEFAULT_PARALLEL))
    }

    pub fn set_parallel(&mut self, n: usize) -> Result<()> {
        self.data.parallel = Some(clamp_parallel(n));
        self.save()
    }

    /// The client ID to use and where it came from. The environment wins over saved values.
    pub fn client_id(&self) -> (String, Source) {
        pick(
            std::env::var("SPOTIFY_CLIENT_ID").ok(),
            self.data.client_id.as_deref(),
        )
    }

    /// The client secret to use and where it came from.
    pub fn client_secret(&self) -> (String, Source) {
        pick(
            std::env::var("SPOTIFY_CLIENT_SECRET").ok(),
            self.data.client_secret.as_deref(),
        )
    }

    /// Saves credentials found in the environment or a `.env` file when none are saved yet,
    /// so the app keeps working without that file. Returns true if it saved them.
    pub fn adopt_env_credentials(&mut self) -> bool {
        let saved = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        if saved(&self.data.client_id) || saved(&self.data.client_secret) {
            return false;
        }
        let env = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        let (Some(id), Some(secret)) = (env("SPOTIFY_CLIENT_ID"), env("SPOTIFY_CLIENT_SECRET"))
        else {
            return false;
        };
        self.data.client_id = Some(id.trim().to_string());
        self.data.client_secret = Some(secret.trim().to_string());
        self.save().is_ok()
    }

    pub fn saved_client_id(&self) -> &str {
        self.data.client_id.as_deref().unwrap_or("")
    }

    pub fn set_client_id(&mut self, id: &str) -> Result<()> {
        self.data.client_id = Some(non_empty(id, "client ID")?);
        self.save()
    }

    pub fn set_client_secret(&mut self, secret: &str) -> Result<()> {
        self.data.client_secret = Some(non_empty(secret, "client secret")?);
        self.save()
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.data)?;
        std::fs::write(&self.file, text)
            .with_context(|| format!("cannot write {}", self.file.display()))
    }
}

/// Keeps a parallel download count in the supported range.
pub fn clamp_parallel(n: usize) -> usize {
    n.clamp(MIN_PARALLEL, MAX_PARALLEL)
}

fn non_empty(value: &str, what: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("the {what} cannot be empty");
    }
    Ok(value.to_string())
}

/// Picks the environment value if it is set, otherwise the saved one.
fn pick(env_value: Option<String>, saved: Option<&str>) -> (String, Source) {
    if let Some(value) = env_value.map(|v| v.trim().to_string())
        && !value.is_empty()
    {
        return (value, Source::Env);
    }
    match saved.map(str::trim).filter(|v| !v.is_empty()) {
        Some(value) => (value.to_string(), Source::Saved),
        None => (String::new(), Source::Missing),
    }
}

/// One line describing a credential without ever printing a secret.
pub fn describe(value: &str, source: Source, hidden: bool) -> String {
    let shown = match source {
        Source::Missing => return "not set".to_string(),
        _ if hidden => "set (hidden)",
        _ => value,
    };
    match source {
        Source::Env => format!("{shown} (from .env or environment)"),
        _ => format!("{shown} (saved)"),
    }
}

/// Folder holding `config.json` and `refresh_token.json`.
/// Set `SPOTIFY_DL_CONFIG_DIR` to use another folder, for example when testing.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("SPOTIFY_DL_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

fn default_download_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::download_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigFile, Source, clamp_parallel, pick};

    #[test]
    fn parallel_defaults_to_three_and_is_clamped() {
        let mut config = Config {
            file: std::path::PathBuf::from("unused.json"),
            data: ConfigFile::default(),
        };
        assert_eq!(config.parallel(), 3);
        config.data.parallel = Some(99);
        assert_eq!(config.parallel(), 8);
        config.data.parallel = Some(0);
        assert_eq!(config.parallel(), 1);
        assert_eq!(clamp_parallel(5), 5);
    }

    #[test]
    fn environment_wins_over_saved() {
        let (value, source) = pick(Some(" env ".into()), Some("saved"));
        assert_eq!((value.as_str(), source), ("env", Source::Env));
    }

    #[test]
    fn falls_back_to_saved_then_missing() {
        assert_eq!(pick(None, Some("saved")), ("saved".into(), Source::Saved));
        assert_eq!(pick(Some("  ".into()), Some("saved")).1, Source::Saved);
        assert_eq!(pick(None, Some(" ")), (String::new(), Source::Missing));
        assert_eq!(pick(None, None).1, Source::Missing);
    }
}
