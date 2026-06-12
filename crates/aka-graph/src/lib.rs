//! aka-graph — SQLite 持久层 + 内存 CSR 邻接 + 遍历查询 + 确定性布局 / LOD 聚合。
//!
//! 设计（已拍板）：不引嵌入式图数据库、不保 Cypher。
//! - [`GraphStore`]：SQLite 持久（nodes/edges/edge_types/meta + positions/clusters）。
//! - [`Adjacency`]：启动时构建的内存 CSR（正反向），callees/callers/impact/neighbors
//!   全部走内存 BFS。
//! - [`compute_layout`]：两级 phyllotaxis 确定性静态布局，坐标写回 SQLite，
//!   前端（Cosmograph/WebGL）只渲染不计算。
//! - LOD：[`GraphStore::lod_snapshot`]（截断快照）/ [`GraphStore::lod_snapshot_binary`]
//!   （二进制形态）/ [`GraphStore::cluster_graph`]（簇级聚合视图）。

pub mod adjacency;
pub mod ego;
pub mod error;
pub mod layout;
pub mod lod;
pub mod process;
pub mod store;

pub use adjacency::{Adjacency, Neighbor, CALLS_TYPE, IMPACT_EDGE_TYPES};
pub use ego::EGO_RING_STEP;
pub use error::{GraphError, Result};
pub use layout::compute_layout;
pub use lod::{
    ClusterEdge, ClusterGraph, ClusterLodGraph, ClusterNode, LodBinary, LodGraph, LodNode,
};
pub use process::{CommunityMembership, ProcessMembership, ProcessStepRow};
pub use store::{GraphStore, IngestStats, NodeRow, PositionRow};
