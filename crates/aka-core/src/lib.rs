//! aka-core — 域模型、工件合同类型、仓库注册表、engine 运行器。

pub mod artifact;
pub mod engine;
pub mod paths;
pub mod registry;
pub mod types;

pub use artifact::{ArtifactDir, ArtifactError, NdjsonIter};
pub use engine::{EngineError, EngineRunner};
pub use paths::{aka_home, RepoPaths};
pub use registry::{Registry, RegistryError, RepoEntry};
pub use types::*;
