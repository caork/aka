//! aka 数据目录约定：`$AKA_HOME`（默认 `~/.aka`）。
//!
//! ```text
//! ~/.aka/
//!   registry.json            # 仓库注册表
//!   repos/<slug>-<hash8>/    # 每仓库数据
//!     engine-cache/           # embedded AKA engine cache 工作目录
//!     index-state.json        # 文件哈希/engine/合同版本快照，用于安全复用索引
//!     parse-cache/            # 内容寻址 parse/fact ownership cache
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
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let hash = fnv1a(repo_path.to_string_lossy().as_bytes());
    format!("{slug}-{hash:08x}")
}

/// Convert Rust/Win32 verbatim paths (for example `\\?\D:\repo`) back to
/// ordinary absolute paths before passing them to external tools or showing
/// them to users. Rust file APIs can handle verbatim paths, but the native AKA
/// C engine currently discovers zero files when it receives that form.
pub fn user_facing_path(path: &Path) -> PathBuf {
    strip_windows_verbatim(path).unwrap_or_else(|| path.to_path_buf())
}

#[cfg(windows)]
fn strip_windows_verbatim(path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Component::Prefix(prefix) = components.next()? else {
        return None;
    };
    match prefix.kind() {
        Prefix::VerbatimDisk(drive) => {
            let mut normalized = PathBuf::from(format!("{}:\\", drive as char));
            normalized.extend(components);
            Some(normalized)
        }
        Prefix::VerbatimUNC(server, share) => {
            let mut normalized = PathBuf::from(format!(
                "\\\\{}\\{}",
                server.to_string_lossy(),
                share.to_string_lossy()
            ));
            normalized.extend(components);
            Some(normalized)
        }
        _ => None,
    }
}

#[cfg(not(windows))]
fn strip_windows_verbatim(path: &Path) -> Option<PathBuf> {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return Some(PathBuf::from(format!(r"\\{rest}")));
    }
    s.strip_prefix(r"\\?\").map(PathBuf::from)
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

    pub fn engine_cache_dir(&self) -> PathBuf {
        self.root.join("engine-cache")
    }

    pub fn index_state_path(&self) -> PathBuf {
        self.root.join("index-state.json")
    }

    pub fn parse_cache_dir(&self) -> PathBuf {
        self.root.join("parse-cache")
    }

    pub fn parse_cache_manifest_path(&self) -> PathBuf {
        self.parse_cache_dir().join("manifest.json")
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

    #[test]
    fn user_facing_path_strips_windows_verbatim_disk_prefix() {
        let path = Path::new(r"\\?\D:\dev\DataStudioFrontend");
        assert_eq!(
            user_facing_path(path).to_string_lossy(),
            r"D:\dev\DataStudioFrontend"
        );
    }

    #[test]
    fn user_facing_path_strips_windows_verbatim_unc_prefix() {
        let path = Path::new(r"\\?\UNC\server\share\repo");
        assert_eq!(
            user_facing_path(path).to_string_lossy(),
            r"\\server\share\repo"
        );
    }
}
