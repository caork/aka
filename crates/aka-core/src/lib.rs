//! aka-core — 域模型、工件合同类型、仓库注册表、工件摄取、增量索引编排。

pub mod artifact;
pub mod types;

pub use artifact::{ArtifactDir, ArtifactError, NdjsonIter};
pub use types::*;
