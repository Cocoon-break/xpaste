//! xPaste 核心逻辑：类型识别、采集、存储。不依赖 Tauri 和系统接口。

pub mod capture;
pub mod detect;
pub mod error;
pub mod model;
#[cfg(feature = "store")]
pub mod store;

pub use error::CoreError;
pub use model::*;
#[cfg(feature = "store")]
pub use store::Store;
