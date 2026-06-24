//! File-hash based index state and parse-cache metadata.
//!
//! This is the Rust-side landing zone for the GitNexus `fileHashes` idea.  The
//! engine may still be invoked as a whole for changed repositories, but aka now
//! records exact file deltas and fact ownership so the next layer can do
//! file-scoped graph/search replacement without guessing.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use aka_facts::{FactSource, FactSourceError};

use crate::artifact::ArtifactDir;
use crate::types::ArtifactStats;
use crate::types::CONTRACT_VERSION;

const INDEX_STATE_VERSION: u32 = 1;
const PARSE_CACHE_MANIFEST_VERSION: u32 = 1;

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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexDelta {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub unchanged: Vec<String>,
}

impl IndexDelta {
    pub fn between(previous: Option<&IndexState>, current: &IndexState) -> Self {
        let Some(previous) = previous else {
            return Self {
                added: current.files.keys().cloned().collect(),
                ..Self::default()
            };
        };

        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();
        let mut unchanged = Vec::new();

        for (path, current_fp) in &current.files {
            match previous.files.get(path) {
                None => added.push(path.clone()),
                Some(previous_fp) if previous_fp == current_fp => unchanged.push(path.clone()),
                Some(_) => modified.push(path.clone()),
            }
        }
        for path in previous.files.keys() {
            if !current.files.contains_key(path) {
                deleted.push(path.clone());
            }
        }

        Self {
            added,
            modified,
            deleted,
            unchanged,
        }
    }

    pub fn changed_count(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.changed_count() == 0
    }

    pub fn summary(&self) -> String {
        format!(
            "+{} ~{} -{} ={}",
            self.added.len(),
            self.modified.len(),
            self.deleted.len(),
            self.unchanged.len()
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileArtifactStats {
    pub hash: String,
    pub size: u64,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub chunks: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseCacheManifest {
    pub version: u32,
    pub contract_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_sha: Option<String>,
    pub no_chunks: bool,
    #[serde(default)]
    pub totals: ArtifactStats,
    #[serde(default)]
    pub last_delta: IndexDelta,
    #[serde(default)]
    pub files: BTreeMap<String, FileArtifactStats>,
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

    pub fn delta_from(&self, previous: Option<&Self>) -> IndexDelta {
        IndexDelta::between(previous, self)
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

pub fn load_parse_cache_manifest(path: &Path) -> std::io::Result<Option<ParseCacheManifest>> {
    match File::open(path) {
        Ok(file) => {
            let reader = BufReader::new(file);
            let manifest = serde_json::from_reader(reader)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn save_parse_cache_manifest(
    path: &Path,
    manifest: &ParseCacheManifest,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);
    {
        let file = File::create(&tmp)?;
        serde_json::to_writer_pretty(file, manifest).map_err(std::io::Error::other)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn build_parse_cache_manifest(
    artifact: &ArtifactDir,
    current: &IndexState,
    delta: IndexDelta,
) -> Result<ParseCacheManifest, crate::artifact::ArtifactError> {
    build_parse_cache_manifest_from_facts(artifact, current, delta)
        .map_err(crate::artifact::ArtifactError::from)
}

pub fn build_parse_cache_manifest_from_facts(
    source: &impl FactSource,
    current: &IndexState,
    delta: IndexDelta,
) -> Result<ParseCacheManifest, FactSourceError> {
    let mut files: BTreeMap<String, FileArtifactStats> = current
        .files
        .iter()
        .map(|(path, fp)| {
            (
                path.clone(),
                FileArtifactStats {
                    hash: fp.hash.clone(),
                    size: fp.size,
                    nodes: 0,
                    edges: 0,
                    chunks: 0,
                },
            )
        })
        .collect();
    let mut node_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for node in source.nodes()? {
        let node = node?;
        if let Some(file_path) = node
            .file_path()
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
        {
            files.entry(file_path.clone()).or_default().nodes += 1;
            node_files.entry(node.id).or_default().insert(file_path);
        }
    }

    for edge in source.edges()? {
        let edge = edge?;
        let mut seen = BTreeSet::new();
        if let Some(paths) = node_files.get(&edge.source_id) {
            seen.extend(paths.iter().cloned());
        }
        if let Some(paths) = node_files.get(&edge.target_id) {
            seen.extend(paths.iter().cloned());
        }
        for file_path in seen {
            files.entry(file_path).or_default().edges += 1;
        }
    }

    if let Some(chunks) = source.chunks()? {
        for chunk in chunks {
            let chunk = chunk?;
            if !chunk.file_path.is_empty() {
                files.entry(chunk.file_path).or_default().chunks += 1;
            }
        }
    }

    Ok(ParseCacheManifest {
        version: PARSE_CACHE_MANIFEST_VERSION,
        contract_version: CONTRACT_VERSION,
        engine_sha: current.engine_sha.clone(),
        no_chunks: current.no_chunks,
        totals: source.stats().clone(),
        last_delta: delta,
        files,
    })
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

    #[test]
    fn computes_file_delta() {
        let dir = temp_dir("delta");
        std::fs::write(dir.join("same.ts"), "same").unwrap();
        std::fs::write(dir.join("changed.ts"), "one").unwrap();
        std::fs::write(dir.join("deleted.ts"), "bye").unwrap();
        let previous = IndexState::compute(&dir, None, false).unwrap();

        std::fs::write(dir.join("changed.ts"), "two").unwrap();
        std::fs::write(dir.join("added.ts"), "new").unwrap();
        std::fs::remove_file(dir.join("deleted.ts")).unwrap();
        let current = IndexState::compute(&dir, None, false).unwrap();
        let delta = current.delta_from(Some(&previous));

        assert_eq!(delta.added, vec!["added.ts"]);
        assert_eq!(delta.modified, vec!["changed.ts"]);
        assert_eq!(delta.deleted, vec!["deleted.ts"]);
        assert_eq!(delta.unchanged, vec!["same.ts"]);
        assert_eq!(delta.changed_count(), 3);
        assert_eq!(delta.summary(), "+1 ~1 -1 =1");
    }

    #[test]
    fn first_delta_marks_all_files_added() {
        let dir = temp_dir("first-delta");
        std::fs::write(dir.join("a.ts"), "one").unwrap();
        std::fs::write(dir.join("b.ts"), "two").unwrap();
        let current = IndexState::compute(&dir, Some("engine".into()), false).unwrap();
        let delta = current.delta_from(None);

        assert_eq!(delta.added, vec!["a.ts", "b.ts"]);
        assert!(delta.modified.is_empty());
        assert!(delta.deleted.is_empty());
        assert!(delta.unchanged.is_empty());
    }

    #[test]
    fn builds_and_roundtrips_parse_cache_manifest() {
        let repo = temp_dir("manifest-repo");
        std::fs::write(repo.join("a.ts"), "export function a() {}\n").unwrap();
        std::fs::write(repo.join("b.ts"), "export function b() {}\n").unwrap();
        let state = IndexState::compute(&repo, Some("engine".into()), false).unwrap();

        let artifact_dir = temp_dir("manifest-artifact");
        std::fs::write(
            artifact_dir.join("nodes.ndjson"),
            concat!(
                r#"{"id":"file:a","label":"File","properties":{"name":"a.ts","filePath":"a.ts"}}"#,
                "\n",
                r#"{"id":"fn:a","label":"Function","properties":{"name":"a","filePath":"a.ts","startLine":0,"endLine":0}}"#,
                "\n",
                r#"{"id":"fn:b","label":"Function","properties":{"name":"b","filePath":"b.ts","startLine":0,"endLine":0}}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            artifact_dir.join("edges.ndjson"),
            concat!(
                r#"{"id":"e1","sourceId":"fn:a","targetId":"fn:b","type":"CALLS","confidence":0.9,"reason":"test"}"#,
                "\n",
                r#"{"id":"e2","sourceId":"fn:a","targetId":"missing","type":"CALLS","confidence":0.5,"reason":"dangling"}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            artifact_dir.join("chunks.ndjson"),
            concat!(
                r#"{"nodeId":"fn:a","kind":"ast-function","filePath":"a.ts","startLine":0,"endLine":0,"text":"a"}"#,
                "\n",
                r#"{"nodeId":"fn:b","kind":"ast-function","filePath":"b.ts","startLine":0,"endLine":0,"text":"b"}"#,
                "\n",
            ),
        )
        .unwrap();
        std::fs::write(
            artifact_dir.join("manifest.json"),
            r#"{"contractVersion":0,"engineVersion":"test","repoPath":"/r","generatedAt":"2026-06-10T00:00:00Z","stats":{"files":2,"nodes":3,"edges":2,"chunks":2}}"#,
        )
        .unwrap();

        let artifact = ArtifactDir::open(&artifact_dir).unwrap();
        let manifest =
            build_parse_cache_manifest(&artifact, &state, state.delta_from(None)).unwrap();

        assert_eq!(manifest.engine_sha.as_deref(), Some("engine"));
        assert_eq!(manifest.last_delta.summary(), "+2 ~0 -0 =0");
        assert_eq!(manifest.totals.nodes, 3);
        assert_eq!(manifest.files["a.ts"].nodes, 2);
        assert_eq!(manifest.files["a.ts"].edges, 2);
        assert_eq!(manifest.files["a.ts"].chunks, 1);
        assert_eq!(manifest.files["b.ts"].nodes, 1);
        assert_eq!(manifest.files["b.ts"].edges, 1);
        assert_eq!(manifest.files["b.ts"].chunks, 1);

        let path = artifact_dir.join("parse-cache-manifest.json");
        save_parse_cache_manifest(&path, &manifest).unwrap();
        assert_eq!(load_parse_cache_manifest(&path).unwrap(), Some(manifest));
    }
}
