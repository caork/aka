//! File-hash based index state for safe no-change reuse.
//!
//! This is the Rust-side landing zone for the GitNexus `fileHashes` idea.  The
//! current engine is still invoked as a whole for changed repositories, but an
//! unchanged repository can skip the expensive CBM parse and Rust index rebuild
//! when the contract/engine/settings/file hashes all match.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::types::CONTRACT_VERSION;

const INDEX_STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFingerprint {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexState {
    pub version: u32,
    pub contract_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_sha: Option<String>,
    pub no_chunks: bool,
    pub files: BTreeMap<String, FileFingerprint>,
}

impl IndexState {
    pub fn compute(
        repo: &Path,
        engine_sha: Option<String>,
        no_chunks: bool,
    ) -> std::io::Result<Self> {
        let mut files = BTreeMap::new();
        collect_files(repo, repo, &mut files)?;
        Ok(Self {
            version: INDEX_STATE_VERSION,
            contract_version: CONTRACT_VERSION,
            engine_sha,
            no_chunks,
            files,
        })
    }

    pub fn is_reusable_for(&self, current: &Self) -> bool {
        self.version == INDEX_STATE_VERSION
            && self.contract_version == CONTRACT_VERSION
            && self.contract_version == current.contract_version
            && self.engine_sha == current.engine_sha
            && self.no_chunks == current.no_chunks
            && self.files == current.files
    }
}

pub fn load_index_state(path: &Path) -> std::io::Result<Option<IndexState>> {
    match File::open(path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            let state = serde_json::from_reader(reader)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(Some(state))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_index_state(path: &Path, state: &IndexState) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    {
        let file = File::create(&tmp)?;
        serde_json::to_writer_pretty(file, state).map_err(std::io::Error::other)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

fn collect_files(
    repo: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, FileFingerprint>,
) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if is_skipped_dir(&entry.file_name().to_string_lossy()) {
                continue;
            }
            collect_files(repo, &path, out)?;
        } else if file_type.is_file() {
            if is_skipped_file(&entry.file_name().to_string_lossy()) {
                continue;
            }
            let Some(rel) = repo_relative(repo, &path) else {
                continue;
            };
            out.insert(rel, fingerprint_file(&path)?);
        }
    }
    Ok(())
}

fn fingerprint_file(path: &Path) -> std::io::Result<FileFingerprint> {
    let mut file = File::open(path)?;
    let size = file.metadata()?.len();
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(FileFingerprint {
        hash: format!("{:x}", hasher.finalize()),
        size,
    })
}

fn repo_relative(repo: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(repo)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
}

fn is_skipped_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".aka"
            | ".claude"
            | ".cursor"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | "coverage"
            | "__pycache__"
            | ".venv"
            | "venv"
            | ".next"
            | ".nuxt"
            | ".turbo"
    )
}

fn is_skipped_file(name: &str) -> bool {
    matches!(name, ".DS_Store")
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!("{e}.tmp"))
        .unwrap_or_else(|| "tmp".into());
    tmp.set_extension(ext);
    tmp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("aka-incremental-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn computes_stable_hashes_and_skips_heavy_dirs() {
        let dir = temp_dir("stable");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub fn main() {}\n").unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::write(dir.join("node_modules/pkg/index.js"), "ignored").unwrap();

        let a = IndexState::compute(&dir, Some("sha".into()), false).unwrap();
        let b = IndexState::compute(&dir, Some("sha".into()), false).unwrap();
        assert_eq!(a, b);
        assert!(a.files.contains_key("src/lib.rs"));
        assert!(!a.files.contains_key("node_modules/pkg/index.js"));
    }

    #[test]
    fn detects_content_changes() {
        let dir = temp_dir("change");
        std::fs::write(dir.join("a.ts"), "one").unwrap();
        let a = IndexState::compute(&dir, None, false).unwrap();
        std::fs::write(dir.join("a.ts"), "two").unwrap();
        let b = IndexState::compute(&dir, None, false).unwrap();
        assert!(!a.is_reusable_for(&b));
    }

    #[test]
    fn roundtrips_state() {
        let dir = temp_dir("roundtrip");
        std::fs::write(dir.join("a.ts"), "one").unwrap();
        let state = IndexState::compute(&dir, Some("engine".into()), true).unwrap();
        let path = dir.join("state.json");
        save_index_state(&path, &state).unwrap();
        assert_eq!(load_index_state(&path).unwrap(), Some(state));
    }
}
