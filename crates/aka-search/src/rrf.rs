//! RRF（Reciprocal Rank Fusion）混排 — 融合 BM25 与向量两路检索结果。

use std::collections::HashMap;

use crate::Hit;

/// RRF 平滑常数 K（业界惯用 60）。
pub const RRF_K: f32 = 60.0;

/// RRF 混排：融合分 = Σ 每路 `1 / (K + rank + 1)`（rank 从 0 起），按融合分降序
/// 取前 `limit` 条；返回 Hit 的 `score` 字段被替换为融合分。
///
/// - 两路都命中的 node_id 累加两路贡献；
/// - 仅 BM25 命中的直接沿用其 [`Hit`] 元数据；
/// - 仅语义命中的通过 `lookup` 兜底补全元数据（返回 `None` 则该条被丢弃，
///   因为没有元数据无法构造 [`Hit`]）。
///
/// `bm25` 与 `semantic` 都假定已按各自分数降序排列（即下标即 rank）。
pub fn rrf_merge(
    bm25: &[Hit],
    semantic: &[(String, f32)],
    limit: usize,
    lookup: impl Fn(&str) -> Option<Hit>,
) -> Vec<Hit> {
    // node_id → (融合分, 元数据)。
    let mut fused: HashMap<&str, (f32, Option<Hit>)> = HashMap::new();
    // 首次出现顺序，保证遍历/平分时结果确定。
    let mut order: Vec<&str> = Vec::new();

    for (rank, hit) in bm25.iter().enumerate() {
        let entry = fused.entry(&hit.node_id).or_insert_with(|| {
            order.push(&hit.node_id);
            (0.0, None)
        });
        entry.0 += rrf_contrib(rank);
        if entry.1.is_none() {
            entry.1 = Some(hit.clone());
        }
    }

    for (rank, (node_id, _similarity)) in semantic.iter().enumerate() {
        let entry = fused.entry(node_id).or_insert_with(|| {
            order.push(node_id);
            (0.0, None)
        });
        entry.0 += rrf_contrib(rank);
        if entry.1.is_none() {
            entry.1 = lookup(node_id);
        }
    }

    let mut out: Vec<Hit> = order
        .into_iter()
        .filter_map(|id| {
            let (score, hit) = fused.remove(id)?;
            let mut hit = hit?;
            hit.score = score;
            Some(hit)
        })
        .collect();
    // total_cmp 全序，分数相同保持首次出现顺序（sort 稳定）。
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(limit);
    out
}

#[inline]
fn rrf_contrib(rank: usize) -> f32 {
    1.0 / (RRF_K + rank as f32 + 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(node_id: &str, score: f32) -> Hit {
        Hit {
            node_id: node_id.to_owned(),
            score,
            name: node_id.to_owned(),
            file_path: format!("src/{node_id}.rs"),
            label: "Function".to_owned(),
            kind: None,
            snippet: None,
            start_line: 1,
        }
    }

    const EPS: f32 = 1e-6;

    #[test]
    fn overlap_and_disjoint_fusion() {
        // bm25: A(0), B(1)；semantic: B(0), C(1)。
        let bm25 = vec![hit("A", 10.0), hit("B", 5.0)];
        let semantic = vec![("B".to_owned(), 0.9), ("C".to_owned(), 0.8)];
        let merged = rrf_merge(&bm25, &semantic, 10, |id| Some(hit(id, 0.0)));

        // B = 1/62 + 1/61，A = 1/61，C = 1/62 → 排序 B, A, C。
        assert_eq!(
            merged
                .iter()
                .map(|h| h.node_id.as_str())
                .collect::<Vec<_>>(),
            vec!["B", "A", "C"]
        );
        assert!((merged[0].score - (1.0 / 62.0 + 1.0 / 61.0)).abs() < EPS);
        assert!((merged[1].score - 1.0 / 61.0).abs() < EPS);
        assert!((merged[2].score - 1.0 / 62.0).abs() < EPS);
    }

    #[test]
    fn semantic_only_hit_falls_back_to_lookup() {
        let bm25 = vec![hit("A", 1.0)];
        let semantic = vec![("X".to_owned(), 0.7)];
        let merged = rrf_merge(&bm25, &semantic, 10, |id| {
            (id == "X").then(|| hit("X", 0.0))
        });
        assert_eq!(merged.len(), 2);
        let x = merged.iter().find(|h| h.node_id == "X").unwrap();
        assert_eq!(x.name, "X");
        assert!((x.score - 1.0 / 61.0).abs() < EPS);
    }

    #[test]
    fn lookup_none_drops_unresolvable_semantic_hit() {
        let bm25 = vec![hit("A", 1.0)];
        let semantic = vec![("ghost".to_owned(), 0.99)];
        let merged = rrf_merge(&bm25, &semantic, 10, |_| None);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].node_id, "A");
    }

    #[test]
    fn bm25_metadata_preferred_over_lookup() {
        // 两路重叠时不调用 lookup，沿用 bm25 的元数据。
        let bm25 = vec![hit("A", 1.0)];
        let semantic = vec![("A".to_owned(), 0.5)];
        let merged = rrf_merge(&bm25, &semantic, 10, |_| {
            panic!("lookup must not be called for bm25-covered ids")
        });
        assert_eq!(merged.len(), 1);
        // 双路贡献：rank 0 + rank 0。
        assert!((merged[0].score - 2.0 / 61.0).abs() < EPS);
        assert_eq!(merged[0].file_path, "src/A.rs");
    }

    #[test]
    fn limit_truncates_after_sorting() {
        let bm25: Vec<Hit> = (0..5)
            .map(|i| hit(&format!("b{i}"), 5.0 - i as f32))
            .collect();
        let semantic: Vec<(String, f32)> = (0..5)
            .map(|i| (format!("s{i}"), 1.0 - i as f32 * 0.1))
            .collect();
        let merged = rrf_merge(&bm25, &semantic, 3, |id| Some(hit(id, 0.0)));
        assert_eq!(merged.len(), 3);
        // 两路 rank0 并列第一（b0 / s0 各 1/61），rank1 其次。
        let top2: Vec<&str> = merged[..2].iter().map(|h| h.node_id.as_str()).collect();
        assert!(top2.contains(&"b0") && top2.contains(&"s0"));
    }
}
