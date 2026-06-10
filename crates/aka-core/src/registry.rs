//! 仓库注册表 — `$AKA_HOME/registry.json`。
//! 单机单写者场景，整文件读写 + 原子替换（临时文件 rename）足够。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::paths::aka_home;
use crate::types::ArtifactStats;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoEntry {
    pub name: String,
    pub repo_path: PathBuf,
    pub data_dir: PathBuf,
    /// 索引完成时间（unix 秒）；None = 注册过但尚未成功索引。
    #[serde(default)]
    pub indexed_at: Option<u64>,
    #[serde(default)]
    pub engine_sha: Option<String>,
    #[serde(default)]
    pub stats: ArtifactStats,
    /// 语义检索开关（默认关——用户拍板：embedding 须手动开启）。
    #[serde(default)]
    pub embeddings_enabled: bool,
    /// 仓库来源：`local` / `git` / `zip`（旧 registry.json 缺省回落 local）。
    #[serde(default = "default_source_kind")]
    pub source_kind: String,
    /// git 来源的 clone URL（local / zip 为 None）。
    #[serde(default)]
    pub source_url: Option<String>,
}

fn default_source_kind() -> String {
    "local".to_string()
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub repos: Vec<RepoEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed registry {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

fn registry_path() -> PathBuf {
    aka_home().join("registry.json")
}

impl Registry {
    pub fn load() -> Result<Self, RegistryError> {
        Self::load_from(&registry_path())
    }

    pub fn load_from(path: &Path) -> Result<Self, RegistryError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| RegistryError::Json {
                path: path.to_path_buf(),
                source,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(RegistryError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn save(&self) -> Result<(), RegistryError> {
        self.save_to(&registry_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), RegistryError> {
        let io = |source| RegistryError::Io {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        let body = serde_json::to_vec_pretty(self).expect("registry serialize");
        fs::write(&tmp, body).map_err(io)?;
        fs::rename(&tmp, path).map_err(io)?;
        Ok(())
    }

    pub fn find(&self, repo_path: &Path) -> Option<&RepoEntry> {
        self.repos.iter().find(|r| r.repo_path == repo_path)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&RepoEntry> {
        self.repos.iter().find(|r| r.name == name)
    }

    pub fn find_by_name_mut(&mut self, name: &str) -> Option<&mut RepoEntry> {
        self.repos.iter_mut().find(|r| r.name == name)
    }

    pub fn remove_by_name(&mut self, name: &str) -> bool {
        let before = self.repos.len();
        self.repos.retain(|r| r.name != name);
        self.repos.len() != before
    }

    /// 插入或更新（按 repo_path 去重）。
    pub fn upsert(&mut self, entry: RepoEntry) {
        match self.repos.iter_mut().find(|r| r.repo_path == entry.repo_path) {
            Some(slot) => *slot = entry,
            None => self.repos.push(entry),
        }
    }

    pub fn remove(&mut self, repo_path: &Path) -> bool {
        let before = self.repos.len();
        self.repos.retain(|r| r.repo_path != repo_path);
        self.repos.len() != before
    }
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_upsert() {
        let dir = std::env::temp_dir().join("aka-registry-test");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("registry.json");

        let mut reg = Registry::load_from(&path).unwrap();
        assert!(reg.repos.is_empty());

        reg.upsert(RepoEntry {
            name: "demo".into(),
            repo_path: "/tmp/demo".into(),
            data_dir: "/tmp/data".into(),
            indexed_at: Some(now_unix()),
            engine_sha: None,
            stats: ArtifactStats::default(),
            embeddings_enabled: false,
            source_kind: "git".into(),
            source_url: Some("https://example.com/demo.git".into()),
        });
        reg.save_to(&path).unwrap();

        let mut reg2 = Registry::load_from(&path).unwrap();
        assert_eq!(reg2.repos.len(), 1);
        assert!(!reg2.repos[0].embeddings_enabled, "embedding 默认必须是关");
        assert_eq!(reg2.repos[0].source_kind, "git");

        reg2.upsert(RepoEntry {
            name: "demo2".into(),
            repo_path: "/tmp/demo".into(),
            data_dir: "/tmp/data".into(),
            indexed_at: None,
            engine_sha: None,
            stats: ArtifactStats::default(),
            embeddings_enabled: false,
            source_kind: "local".into(),
            source_url: None,
        });
        assert_eq!(reg2.repos.len(), 1, "同路径 upsert 不新增");
        assert_eq!(reg2.repos[0].name, "demo2");
    }

    #[test]
    fn old_registry_without_source_fields_defaults_to_local() {
        // 旧版 registry.json（无 sourceKind/sourceUrl）必须能加载且回落 local。
        let body = r#"{"repos":[{"name":"old","repoPath":"/tmp/old","dataDir":"/tmp/old-data"}]}"#;
        let dir = std::env::temp_dir().join("aka-registry-compat-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("registry.json");
        fs::write(&path, body).unwrap();

        let reg = Registry::load_from(&path).unwrap();
        assert_eq!(reg.repos.len(), 1);
        assert_eq!(reg.repos[0].source_kind, "local");
        assert_eq!(reg.repos[0].source_url, None);
    }
}
