//! 内存 CSR 邻接 — 节点 rowid 紧凑化为 u32，正向 + 反向各一份，
//! callees/callers/impact/neighbors 全部走内存 BFS，毫秒级。

use std::collections::{HashMap, VecDeque};

use crate::error::{GraphError, Result};
use crate::store::GraphStore;

/// CALLS 边类型名（callees/callers 只走这一种）。
pub const CALLS_TYPE: &str = "CALLS";

/// impact 反向 BFS 允许的边类型集合。
pub const IMPACT_EDGE_TYPES: [&str; 17] = [
    "CALLS",
    "IMPORTS",
    "EXTENDS",
    "INHERITS",
    "IMPLEMENTS",
    "HAS_METHOD",
    "HAS_PROPERTY",
    "MEMBER_OF",
    "READS",
    "WRITES",
    "ACCESSES",
    "OVERRIDES",
    "METHOD_OVERRIDES",
    "METHOD_IMPLEMENTS",
    "HTTP_CALLS",
    "ACCESSES_RESOURCE",
    "FETCHES",
];

/// 一跳邻居（neighbors 查询结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Neighbor<'a> {
    /// 邻居的稠密下标。
    pub node: u32,
    /// 边类型名。
    pub edge_type: &'a str,
    /// true = 出边（self → node），false = 入边（node → self）。
    pub outgoing: bool,
}

struct Csr {
    offsets: Vec<u32>,
    targets: Vec<u32>,
    types: Vec<u16>,
}

impl Csr {
    fn from_edges(n: usize, edges: &[(u32, u32, u16)], reverse: bool) -> Self {
        let mut offsets = vec![0u32; n + 1];
        for &(s, t, _) in edges {
            let from = if reverse { t } else { s };
            offsets[from as usize + 1] += 1;
        }
        for i in 0..n {
            offsets[i + 1] += offsets[i];
        }
        let m = edges.len();
        let mut targets = vec![0u32; m];
        let mut types = vec![0u16; m];
        let mut cursor: Vec<u32> = offsets[..n].to_vec();
        for &(s, t, ty) in edges {
            let (from, to) = if reverse { (t, s) } else { (s, t) };
            let slot = &mut cursor[from as usize];
            let idx = *slot as usize;
            *slot += 1;
            targets[idx] = to;
            types[idx] = ty;
        }
        Self {
            offsets,
            targets,
            types,
        }
    }

    #[inline]
    fn range(&self, u: u32) -> std::ops::Range<usize> {
        self.offsets[u as usize] as usize..self.offsets[u as usize + 1] as usize
    }
}

/// 全图内存邻接。`u32` 稠密下标 ↔ `String` 节点 id 双向映射随结构暴露。
pub struct Adjacency {
    ids: Vec<String>,
    rowids: Vec<i64>,
    id_index: HashMap<String, u32>,
    type_names: Vec<String>,
    type_index: HashMap<String, u16>,
    fwd: Csr,
    rev: Csr,
    degree: Vec<u32>,
}

impl Adjacency {
    /// 从持久层全量构建。100 万边量级目标 < 2s。
    pub fn build(store: &GraphStore) -> Result<Self> {
        let conn = store.conn();

        // 节点：rowid 升序 → 稠密下标。
        let mut ids: Vec<String> = Vec::new();
        let mut rowids: Vec<i64> = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT rowid, id FROM nodes ORDER BY rowid")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                rowids.push(row.get(0)?);
                ids.push(row.get(1)?);
            }
        }
        let n = ids.len();
        if n > u32::MAX as usize {
            return Err(GraphError::Invalid(format!(
                "node count {n} exceeds u32 dense index space"
            )));
        }

        // rowid → 稠密下标：摄取后 rowid 连续时走 O(1) 偏移，否则哈希。
        let contiguous = match (rowids.first(), rowids.last()) {
            (Some(&first), Some(&last)) => last - first + 1 == n as i64,
            _ => true,
        };
        let base = rowids.first().copied().unwrap_or(0);
        let rowid_map: HashMap<i64, u32> = if contiguous {
            HashMap::new()
        } else {
            rowids
                .iter()
                .enumerate()
                .map(|(i, &r)| (r, i as u32))
                .collect()
        };
        let dense_of = |rowid: i64| -> Option<u32> {
            if contiguous {
                let off = rowid - base;
                (off >= 0 && off < n as i64).then_some(off as u32)
            } else {
                rowid_map.get(&rowid).copied()
            }
        };

        // 边类型字典。
        let mut type_names = Vec::new();
        let mut type_index = HashMap::new();
        for (tid, name) in store.edge_types()? {
            let tid = tid as usize;
            if type_names.len() <= tid {
                type_names.resize(tid + 1, String::new());
            }
            type_names[tid] = name.clone();
            type_index.insert(name, tid as u16);
        }
        if type_names.len() > u16::MAX as usize {
            return Err(GraphError::Invalid("edge type id exceeds u16".to_string()));
        }

        // 边：一次扫描进内存，再计数排序成正反两份 CSR。
        let mut edges: Vec<(u32, u32, u16)> = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT source, target, type_id FROM edges")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let s = dense_of(row.get::<_, i64>(0)?);
                let t = dense_of(row.get::<_, i64>(1)?);
                if let (Some(s), Some(t)) = (s, t) {
                    edges.push((s, t, row.get::<_, i64>(2)? as u16));
                }
            }
        }

        let fwd = Csr::from_edges(n, &edges, false);
        let rev = Csr::from_edges(n, &edges, true);
        let degree = (0..n)
            .map(|i| (fwd.offsets[i + 1] - fwd.offsets[i]) + (rev.offsets[i + 1] - rev.offsets[i]))
            .collect();

        let id_index = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i as u32))
            .collect();

        Ok(Self {
            ids,
            rowids,
            id_index,
            type_names,
            type_index,
            fwd,
            rev,
            degree,
        })
    }

    // ── 映射 ────────────────────────────────────────────────────

    /// 节点数。
    pub fn len(&self) -> u32 {
        self.ids.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// String id → 稠密下标。
    pub fn index_of_id(&self, id: &str) -> Option<u32> {
        self.id_index.get(id).copied()
    }

    /// 稠密下标 → String id。
    pub fn id_of(&self, node: u32) -> &str {
        &self.ids[node as usize]
    }

    /// 稠密下标 → SQLite rowid。
    pub fn rowid_of(&self, node: u32) -> i64 {
        self.rowids[node as usize]
    }

    /// SQLite rowid → 稠密下标（rowid 升序存储，二分查找）。
    pub fn index_of_rowid(&self, rowid: i64) -> Option<u32> {
        self.rowids.binary_search(&rowid).ok().map(|i| i as u32)
    }

    /// 总度数（入 + 出）。
    pub fn degree(&self, node: u32) -> u32 {
        self.degree[node as usize]
    }

    /// 边类型名 → type_id。
    pub fn type_id(&self, name: &str) -> Option<u16> {
        self.type_index.get(name).copied()
    }

    /// type_id → 边类型名。
    pub fn type_name(&self, type_id: u16) -> &str {
        &self.type_names[type_id as usize]
    }

    /// 出边迭代（target, type_id）。
    pub fn out_edges(&self, node: u32) -> impl Iterator<Item = (u32, u16)> + '_ {
        self.fwd
            .range(node)
            .map(|i| (self.fwd.targets[i], self.fwd.types[i]))
    }

    /// 入边迭代（source, type_id）。
    pub fn in_edges(&self, node: u32) -> impl Iterator<Item = (u32, u16)> + '_ {
        self.rev
            .range(node)
            .map(|i| (self.rev.targets[i], self.rev.types[i]))
    }

    // ── 遍历 ────────────────────────────────────────────────────

    /// 正向 BFS，只走 CALLS：node 调用了谁（传递闭包到 max_depth）。
    pub fn callees(&self, node: u32, max_depth: u32, limit: usize) -> Vec<(u32, u32)> {
        self.bfs(node, max_depth, limit, false, &self.mask_for(&[CALLS_TYPE]))
    }

    /// 反向 BFS，只走 CALLS：谁调用了 node。
    pub fn callers(&self, node: u32, max_depth: u32, limit: usize) -> Vec<(u32, u32)> {
        self.bfs(node, max_depth, limit, true, &self.mask_for(&[CALLS_TYPE]))
    }

    /// 影响面：反向 BFS 走 CALLS/IMPORTS/EXTENDS/IMPLEMENTS/HAS_METHOD/MEMBER_OF。
    pub fn impact(&self, node: u32, max_depth: u32, limit: usize) -> Vec<(u32, u32)> {
        self.bfs(
            node,
            max_depth,
            limit,
            true,
            &self.mask_for(&IMPACT_EDGE_TYPES),
        )
    }

    /// 任意类型一跳邻居（带边类型名与方向）。
    pub fn neighbors(&self, node: u32) -> Vec<Neighbor<'_>> {
        let mut out = Vec::new();
        for (v, ty) in self.out_edges(node) {
            out.push(Neighbor {
                node: v,
                edge_type: self.type_name(ty),
                outgoing: true,
            });
        }
        for (v, ty) in self.in_edges(node) {
            out.push(Neighbor {
                node: v,
                edge_type: self.type_name(ty),
                outgoing: false,
            });
        }
        out
    }

    fn mask_for(&self, names: &[&str]) -> Vec<bool> {
        let mut mask = vec![false; self.type_names.len()];
        for name in names {
            if let Some(tid) = self.type_id(name) {
                mask[tid as usize] = true;
            }
        }
        mask
    }

    /// 类型过滤 BFS。返回 (稠密下标, 深度)，按发现顺序；不含起点。
    fn bfs(
        &self,
        start: u32,
        max_depth: u32,
        limit: usize,
        reverse: bool,
        allowed: &[bool],
    ) -> Vec<(u32, u32)> {
        let n = self.ids.len();
        if start as usize >= n || max_depth == 0 || limit == 0 {
            return Vec::new();
        }
        let csr = if reverse { &self.rev } else { &self.fwd };
        let mut visited = vec![false; n];
        visited[start as usize] = true;
        let mut out = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back((start, 0u32));
        while let Some((u, d)) = queue.pop_front() {
            if d >= max_depth {
                continue;
            }
            for i in csr.range(u) {
                let ty = csr.types[i] as usize;
                if ty >= allowed.len() || !allowed[ty] {
                    continue;
                }
                let v = csr.targets[i];
                if visited[v as usize] {
                    continue;
                }
                visited[v as usize] = true;
                out.push((v, d + 1));
                if out.len() >= limit {
                    return out;
                }
                queue.push_back((v, d + 1));
            }
        }
        out
    }
}
