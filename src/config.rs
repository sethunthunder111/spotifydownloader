use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Name of the per-user folder used for config and the stored Spotify refresh token.
const APP_DIR_NAME: &str = "spotify-dl";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    download_path: Option<PathBuf>,
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

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(&self.data)?;
        std::fs::write(&self.file, text)
            .with_context(|| format!("cannot write {}", self.file.display()))
    }
}

/// Folder holding `config.json` and `refresh_token.json`.
pub fn config_dir() -> PathBuf {
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
