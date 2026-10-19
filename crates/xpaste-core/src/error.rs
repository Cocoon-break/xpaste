#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    InvalidSettings(String),
    #[cfg(feature = "store")]
    #[error("数据库错误：{0}")]
    Db(#[from] rusqlite::Error),
    #[error("文件读写错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("JSON 错误：{0}")]
    Json(#[from] serde_json::Error),
}
