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
    /// 发滚动等输入事件失败；消息只含错误码，不含画面内容
    #[error("发送输入事件失败：{0}")]
    Input(String),
    #[error("当前平台不支持")]
    Unsupported,
}
