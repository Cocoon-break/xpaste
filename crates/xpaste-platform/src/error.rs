#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("剪切板访问失败：{0}")]
    Clipboard(String),
    #[error("截屏失败：{0}")]
    Capture(String),
    #[error("录屏失败：{0}")]
    Record(String),
    /// macOS 没有辅助功能权限，不能模拟按键
    #[error("没有辅助功能权限")]
    NoAccessibility,
    #[error("当前平台不支持")]
    Unsupported,
}
