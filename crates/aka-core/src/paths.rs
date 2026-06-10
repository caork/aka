//! aka 数据目录约定：`$AKA_HOME`（默认 `~/.aka`）。
//!
//! ```text
//! ~/.aka/
//!   registry.json            # 仓库注册表
//!   repos/<slug>-<hash8>/    # 每仓库数据
//!     artifact/              # engine 产出的 NDJSON 工件
//!     graph.db               # aka-graph SQLite
//!     search/                # tantivy 索引
//!     vectors/               # 向量库（embedding 开启后）
//! ```

use std::path::{Path, PathBuf};

pub fn aka_home() -> PathBuf {
    if let Ok(custom) = std::env::var("AKA_HOME") {
        return PathBuf::from(custom);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    Path::new(&home).join(".aka")
}

/// 稳定的仓库数据目录名：目录 basename 的 slug + 绝对路径的 FNV-1a hash 前 8 位。
pub fn repo_dir_name(repo_path: &Path) -> String {
    let slug: String = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repo".into())
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let hash = fnv1a(repo_path.to_string_lossy().as_bytes());
    format!("{slug}-{hash:08x}")
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &b in bytes {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

pub struct RepoPaths {
    pub root: PathBuf,
}

impl RepoPaths {
    pub fn for_repo(repo_path: &Path) -> Self {
        Self {
            root: aka_home().join("repos").join(repo_dir_name(repo_path)),
        }
    }

    pub fn artifact_dir(&self) -> PathBuf {
        self.root.join("artifact")
    }

    pub fn graph_db(&self) -> PathBuf {
        self.root.join("graph.db")
    }

    pub fn search_dir(&self) -> PathBuf {
        self.root.join("search")
    }

    pub fn vectors_dir(&self) -> PathBuf {
        self.root.join("vectors")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_dir_name_is_stable_and_sluggy() {
        let a = repo_dir_name(Path::new("/Users/x/My Repo"));
        let b = repo_dir_name(Path::new("/Users/x/My Repo"));
        assert_eq!(a, b);
        assert!(a.starts_with("my-repo-"));
        let c = repo_dir_name(Path::new("/elsewhere/My Repo"));
        assert_ne!(a, c, "同名不同路径必须不同目录");
    }
}
