//! 向量索引 — usearch HNSW（cosine 距离，f32），node_id ↔ u64 key 双向映射
//! 持久化到同目录 sidecar 文件 `ids.json`。

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use usearch::{Index as UsearchIndex, IndexOptions, MetricKind, ScalarKind};

use crate::{Result, SearchError};

/// HNSW 索引文件名。
const VECTORS_FILE: &str = "vectors.usearch";
/// key → node_id 映射 sidecar 文件名（JSON 数组，下标即 key）。
const IDS_FILE: &str = "ids.json";
/// 初始 / 最小预留容量。
const MIN_CAPACITY: usize = 1024;

/// usearch 经 cxx FFI 抛出的异常统一转为 [`SearchError::Vector`]。
fn vec_err(e: impl std::fmt::Display) -> SearchError {
    SearchError::Vector(e.to_string())
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| SearchError::InvalidPath(path.to_path_buf()))
}

/// 基于 usearch HNSW 的向量库（cosine 距离，f32）。
///
/// 内部维护 `node_id ↔ u64 key` 双向映射：key 自增分配，正向映射在内存
/// HashMap，反向映射是 `Vec<String>`（下标即 key），随 [`save`](Self::save)
/// 持久化为同目录的 `ids.json`；[`open`](Self::open) 时自动加载两者。
pub struct VectorStore {
    index: UsearchIndex,
    dir: PathBuf,
    /// key → node_id（下标即 key）。
    ids: Vec<String>,
    /// node_id → key。
    key_of: HashMap<String, u64>,
}

impl VectorStore {
    /// 在 `dir` 下新建维度为 `dim` 的空向量库（目录不存在会创建）。
    pub fn create(dir: &Path, dim: usize) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let options = IndexOptions {
            dimensions: dim,
            metric: MetricKind::Cos,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = UsearchIndex::new(&options).map_err(vec_err)?;
        index.reserve(MIN_CAPACITY).map_err(vec_err)?;
        Ok(Self {
            index,
            dir: dir.to_path_buf(),
            ids: Vec::new(),
            key_of: HashMap::new(),
        })
    }

    /// 打开 `dir` 下既有向量库（自动加载 HNSW 文件与 `ids.json` 映射）。
    pub fn open(dir: &Path) -> Result<Self> {
        let vec_path = dir.join(VECTORS_FILE);
        let index = UsearchIndex::restore(path_str(&vec_path)?).map_err(vec_err)?;
        let ids_file = File::open(dir.join(IDS_FILE))?;
        let ids: Vec<String> = serde_json::from_reader(BufReader::new(ids_file))?;
        let key_of = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i as u64))
            .collect();
        Ok(Self {
            index,
            dir: dir.to_path_buf(),
            ids,
            key_of,
        })
    }

    /// 写入（或覆盖）一条向量。`vec` 维度必须与建库时一致。
    pub fn add(&mut self, node_id: &str, vec: &[f32]) -> Result<()> {
        let dim = self.index.dimensions();
        if vec.len() != dim {
            return Err(SearchError::DimensionMismatch {
                expected: dim,
                got: vec.len(),
            });
        }
        if let Some(&key) = self.key_of.get(node_id) {
            // 覆盖写：usearch 同 key 重复 add 会报错，先删后加。
            self.index.remove(key).map_err(vec_err)?;
            self.index.add(key, vec).map_err(vec_err)?;
            return Ok(());
        }
        if self.index.size() >= self.index.capacity() {
            let target = (self.index.capacity() * 2).max(MIN_CAPACITY);
            self.index.reserve(target).map_err(vec_err)?;
        }
        let key = self.ids.len() as u64;
        self.index.add(key, vec).map_err(vec_err)?;
        self.ids.push(node_id.to_owned());
        self.key_of.insert(node_id.to_owned(), key);
        Ok(())
    }

    /// 近邻检索：返回至多 `k` 条 `(node_id, 相似度)`，相似度 = 1 − cosine 距离
    /// （越大越相似），按相似度降序。
    pub fn search(&self, vec: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        let dim = self.index.dimensions();
        if vec.len() != dim {
            return Err(SearchError::DimensionMismatch {
                expected: dim,
                got: vec.len(),
            });
        }
        let matches = self.index.search(vec, k).map_err(vec_err)?;
        Ok(matches
            .keys
            .iter()
            .zip(matches.distances.iter())
            .filter_map(|(&key, &dist)| {
                self.ids
                    .get(key as usize)
                    .map(|id| (id.clone(), 1.0 - dist))
            })
            .collect())
    }

    /// 持久化：HNSW 写入 `vectors.usearch`，映射写入 `ids.json`（先写临时文件再原子改名）。
    pub fn save(&self) -> Result<()> {
        let vec_path = self.dir.join(VECTORS_FILE);
        self.index.save(path_str(&vec_path)?).map_err(vec_err)?;

        let tmp = self.dir.join("ids.json.tmp");
        let file = File::create(&tmp)?;
        serde_json::to_writer(BufWriter::new(file), &self.ids)?;
        std::fs::rename(&tmp, self.dir.join(IDS_FILE))?;
        Ok(())
    }

    /// 向量维度。
    pub fn dim(&self) -> usize {
        self.index.dimensions()
    }

    /// 已索引向量条数。
    pub fn len(&self) -> usize {
        self.index.size()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_vectors(n: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
        fastrand::seed(seed);
        (0..n)
            .map(|_| (0..dim).map(|_| fastrand::f32() - 0.5).collect())
            .collect()
    }

    #[test]
    fn self_query_top1_is_self() {
        let dir = tempfile::tempdir().unwrap();
        let dim = 384;
        let vectors = random_vectors(1000, dim, 42);

        let mut store = VectorStore::create(dir.path(), dim).unwrap();
        for (i, v) in vectors.iter().enumerate() {
            store.add(&format!("node{i}"), v).unwrap();
        }
        assert_eq!(store.len(), 1000);
        assert_eq!(store.dim(), dim);

        for i in [0usize, 7, 123, 999] {
            let hits = store.search(&vectors[i], 5).unwrap();
            assert_eq!(hits[0].0, format!("node{i}"), "top1 of self-query #{i}");
            assert!(hits[0].1 > 0.999, "self similarity ≈ 1, got {}", hits[0].1);
        }
    }

    #[test]
    fn save_and_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let dim = 64;
        let vectors = random_vectors(100, dim, 7);

        let mut store = VectorStore::create(dir.path(), dim).unwrap();
        for (i, v) in vectors.iter().enumerate() {
            store.add(&format!("n{i}"), v).unwrap();
        }
        store.save().unwrap();
        drop(store);

        let store = VectorStore::open(dir.path()).unwrap();
        assert_eq!(store.len(), 100);
        assert_eq!(store.dim(), dim);
        let hits = store.search(&vectors[42], 3).unwrap();
        assert_eq!(hits[0].0, "n42");
    }

    #[test]
    fn upsert_overwrites_existing_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VectorStore::create(dir.path(), 4).unwrap();
        store.add("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        store.add("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        // 覆盖 a 的向量后，应以新向量参与检索。
        store.add("a", &[0.0, 0.0, 1.0, 0.0]).unwrap();
        let hits = store.search(&[0.0, 0.0, 1.0, 0.0], 1).unwrap();
        assert_eq!(hits[0].0, "a");
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn dimension_mismatch_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VectorStore::create(dir.path(), 8).unwrap();
        let err = store.add("x", &[0.0; 4]).unwrap_err();
        assert!(matches!(
            err,
            SearchError::DimensionMismatch {
                expected: 8,
                got: 4
            }
        ));
        let err = store.search(&[0.0; 16], 3).unwrap_err();
        assert!(matches!(
            err,
            SearchError::DimensionMismatch {
                expected: 8,
                got: 16
            }
        ));
    }

    #[test]
    fn grows_beyond_initial_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = VectorStore::create(dir.path(), 4).unwrap();
        // MIN_CAPACITY=1024，写 1500 条验证自动扩容。
        for i in 0..1500 {
            let v = [i as f32, 1.0, 2.0, 3.0];
            store.add(&format!("k{i}"), &v).unwrap();
        }
        assert_eq!(store.len(), 1500);
    }
}
