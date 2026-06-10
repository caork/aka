//! 工件目录读取 — 流式 NDJSON，不整体载入内存（十亿级数据天花板在磁盘）。

use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::types::{ChunkRec, EdgeRec, Manifest, NodeRec, CONTRACT_VERSION};

#[derive(Debug, thiserror::Error)]
pub enum ArtifactError {
    #[error("artifact dir missing manifest.json (incomplete emit?): {0}")]
    MissingManifest(PathBuf),
    #[error("contract version mismatch: artifact={found}, supported={supported}")]
    ContractVersion { found: u32, supported: u32 },
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed json at {path}:{line}: {source}")]
    Json {
        path: PathBuf,
        line: u64,
        #[source]
        source: serde_json::Error,
    },
}

/// 一个已校验完整性（manifest 存在 + 合同版本匹配）的工件目录。
pub struct ArtifactDir {
    dir: PathBuf,
    pub manifest: Manifest,
}

impl ArtifactDir {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let dir = dir.as_ref().to_path_buf();
        let manifest_path = dir.join("manifest.json");
        let file = File::open(&manifest_path)
            .map_err(|_| ArtifactError::MissingManifest(manifest_path.clone()))?;
        let manifest: Manifest =
            serde_json::from_reader(BufReader::new(file)).map_err(|source| ArtifactError::Json {
                path: manifest_path,
                line: 0,
                source,
            })?;
        if manifest.contract_version != CONTRACT_VERSION {
            return Err(ArtifactError::ContractVersion {
                found: manifest.contract_version,
                supported: CONTRACT_VERSION,
            });
        }
        Ok(Self { dir, manifest })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }

    pub fn nodes(&self) -> Result<NdjsonIter<NodeRec>, ArtifactError> {
        NdjsonIter::open(self.dir.join("nodes.ndjson"))
    }

    pub fn edges(&self) -> Result<NdjsonIter<EdgeRec>, ArtifactError> {
        NdjsonIter::open(self.dir.join("edges.ndjson"))
    }

    /// chunks.ndjson 在 --no-chunks 时缺省 — 返回空迭代器而非报错。
    pub fn chunks(&self) -> Result<Option<NdjsonIter<ChunkRec>>, ArtifactError> {
        let path = self.dir.join("chunks.ndjson");
        if !path.exists() {
            return Ok(None);
        }
        NdjsonIter::open(path).map(Some)
    }
}

/// 流式逐行反序列化迭代器。空行跳过；坏行报错并带行号。
pub struct NdjsonIter<T> {
    path: PathBuf,
    lines: Lines<BufReader<File>>,
    line_no: u64,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> NdjsonIter<T> {
    fn open(path: PathBuf) -> Result<Self, ArtifactError> {
        let file = File::open(&path).map_err(|source| ArtifactError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(Self {
            path,
            lines: BufReader::with_capacity(1 << 20, file).lines(),
            line_no: 0,
            _marker: PhantomData,
        })
    }
}

impl<T: DeserializeOwned> Iterator for NdjsonIter<T> {
    type Item = Result<T, ArtifactError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            self.line_no += 1;
            match self.lines.next()? {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_str(&line).map_err(|source| {
                        ArtifactError::Json {
                            path: self.path.clone(),
                            line: self.line_no,
                            source,
                        }
                    }));
                }
                Err(source) => {
                    return Some(Err(ArtifactError::Io {
                        path: self.path.clone(),
                        source,
                    }))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_artifact(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        let mut f = File::create(dir.join("nodes.ndjson")).unwrap();
        writeln!(f, r#"{{"id":"n1","label":"Function","properties":{{"name":"foo","filePath":"a.ts","startLine":1,"endLine":3}}}}"#).unwrap();
        writeln!(f).unwrap();
        writeln!(f, r#"{{"id":"n2","label":"Class","properties":{{"name":"Bar"}}}}"#).unwrap();
        let mut f = File::create(dir.join("edges.ndjson")).unwrap();
        writeln!(f, r#"{{"id":"e1","sourceId":"n1","targetId":"n2","type":"CALLS","confidence":0.9,"reason":"local-call"}}"#).unwrap();
        let mut f = File::create(dir.join("manifest.json")).unwrap();
        writeln!(f, r#"{{"contractVersion":0,"engineVersion":"t","repoPath":"/r","generatedAt":"2026-06-10T00:00:00Z","stats":{{"files":1,"nodes":2,"edges":1,"chunks":0}}}}"#).unwrap();
    }

    #[test]
    fn reads_artifact_dir() {
        let dir = std::env::temp_dir().join("aka-core-artifact-test");
        let _ = std::fs::remove_dir_all(&dir);
        write_artifact(&dir);

        let art = ArtifactDir::open(&dir).unwrap();
        assert_eq!(art.manifest.stats.nodes, 2);

        let nodes: Vec<NodeRec> = art.nodes().unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name(), Some("foo"));
        assert_eq!(nodes[0].file_path(), Some("a.ts"));
        assert_eq!(nodes[0].start_line(), Some(1));

        let edges: Vec<EdgeRec> = art.edges().unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(edges[0].edge_type, "CALLS");
        assert_eq!(edges[0].source_id, "n1");

        assert!(art.chunks().unwrap().is_none());
    }

    #[test]
    fn missing_manifest_is_incomplete() {
        let dir = std::env::temp_dir().join("aka-core-artifact-test-incomplete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            ArtifactDir::open(&dir),
            Err(ArtifactError::MissingManifest(_))
        ));
    }
}
