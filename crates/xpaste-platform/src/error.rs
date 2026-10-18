#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("剪切板访问失败：{0}")]
    Clipboard(String),
    #[error("当前平台不支持")]
    Unsupported,
}
