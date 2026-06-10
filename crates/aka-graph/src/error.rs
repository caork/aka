//! aka-graph 统一错误类型。

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("graph database not found: {0}")]
    DbNotFound(PathBuf),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid state: {0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, GraphError>;
