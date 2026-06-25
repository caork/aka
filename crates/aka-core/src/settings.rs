//! Global AKA runtime settings stored in `$AKA_HOME/settings.json`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths::aka_home;

/// Default global indexing budget. Users can override it in Settings or with
/// `AKA_INDEX_MAX_SECS` for one-off runs.
pub const DEFAULT_INDEX_MAX_SECS: u64 = 60;
pub const MIN_INDEX_MAX_SECS: u64 = 10;
pub const MAX_INDEX_MAX_SECS: u64 = 24 * 60 * 60;
pub const DEFAULT_LSP_ENRICHMENT_ENABLED: bool = false;
pub const DEFAULT_LSP_ENRICHMENT_MAX_SECS: u64 = 30;
pub const MIN_LSP_ENRICHMENT_MAX_SECS: u64 = 5;
pub const MAX_LSP_ENRICHMENT_MAX_SECS: u64 = 10 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AkaSettings {
    #[serde(default = "default_index_max_secs")]
    pub index_max_secs: u64,
    #[serde(default)]
    pub lsp_enrichment_enabled: bool,
    #[serde(default = "default_lsp_enrichment_max_secs")]
    pub lsp_enrichment_max_secs: u64,
}

impl Default for AkaSettings {
    fn default() -> Self {
        Self {
            index_max_secs: DEFAULT_INDEX_MAX_SECS,
            lsp_enrichment_enabled: DEFAULT_LSP_ENRICHMENT_ENABLED,
            lsp_enrichment_max_secs: DEFAULT_LSP_ENRICHMENT_MAX_SECS,
        }
    }
}

impl AkaSettings {
    pub fn load() -> Result<Self, SettingsError> {
        Self::load_from(&settings_path())
    }

    pub fn load_from(path: &Path) -> Result<Self, SettingsError> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut settings: Self =
                    serde_json::from_slice(&bytes).map_err(|source| SettingsError::Json {
                        path: path.to_path_buf(),
                        source,
                    })?;
                settings.normalize();
                Ok(settings)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(SettingsError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self) -> Result<Self, SettingsError> {
        self.save_to(&settings_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<Self, SettingsError> {
        let io = |source| SettingsError::Io {
            path: path.to_path_buf(),
            source,
        };
        let mut normalized = *self;
        normalized.normalize();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(&normalized).expect("settings serialize");
        fs::write(&tmp, body).map_err(io)?;
        fs::rename(&tmp, path).map_err(io)?;
        Ok(normalized)
    }

    fn normalize(&mut self) {
        self.index_max_secs = clamp_index_max_secs(self.index_max_secs);
        self.lsp_enrichment_max_secs = clamp_lsp_enrichment_max_secs(self.lsp_enrichment_max_secs);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed settings {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub fn settings_path() -> PathBuf {
    aka_home().join("settings.json")
}

pub fn clamp_index_max_secs(seconds: u64) -> u64 {
    seconds.clamp(MIN_INDEX_MAX_SECS, MAX_INDEX_MAX_SECS)
}

pub fn clamp_lsp_enrichment_max_secs(seconds: u64) -> u64 {
    seconds.clamp(MIN_LSP_ENRICHMENT_MAX_SECS, MAX_LSP_ENRICHMENT_MAX_SECS)
}

fn default_index_max_secs() -> u64 {
    DEFAULT_INDEX_MAX_SECS
}

fn default_lsp_enrichment_max_secs() -> u64 {
    DEFAULT_LSP_ENRICHMENT_MAX_SECS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_settings_use_sixty_second_default() {
        let dir = std::env::temp_dir().join("aka-settings-missing-test");
        let _ = fs::remove_dir_all(&dir);
        let settings = AkaSettings::load_from(&dir.join("settings.json")).unwrap();
        assert_eq!(settings.index_max_secs, 60);
        assert!(!settings.lsp_enrichment_enabled);
        assert_eq!(settings.lsp_enrichment_max_secs, 30);
    }

    #[test]
    fn settings_roundtrip_and_clamp_index_budget() {
        let dir = std::env::temp_dir().join("aka-settings-roundtrip-test");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");

        let saved = AkaSettings {
            index_max_secs: 3,
            lsp_enrichment_enabled: true,
            lsp_enrichment_max_secs: 1,
        }
        .save_to(&path)
        .unwrap();
        assert_eq!(saved.index_max_secs, MIN_INDEX_MAX_SECS);
        assert!(saved.lsp_enrichment_enabled);
        assert_eq!(saved.lsp_enrichment_max_secs, MIN_LSP_ENRICHMENT_MAX_SECS);

        let loaded = AkaSettings::load_from(&path).unwrap();
        assert_eq!(loaded.index_max_secs, MIN_INDEX_MAX_SECS);
        assert!(loaded.lsp_enrichment_enabled);
        assert_eq!(loaded.lsp_enrichment_max_secs, MIN_LSP_ENRICHMENT_MAX_SECS);
    }
}
