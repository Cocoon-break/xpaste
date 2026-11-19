//! Pro 功能的外壳：对外接口固定在 `types` 和 `imp` 里。
//! 私有实现（`pro/rust/mod.rs`）在场时用它，否则用 `stub.rs` 的空实现。

// rustfmt::skip：没有 pro/ 时 `cargo fmt --check` 也不会因找不到文件而失败
#[cfg(pro)]
#[rustfmt::skip]
#[path = "../../../pro/rust/mod.rs"]
mod imp;
#[cfg(not(pro))]
mod stub;
pub use imp::*;
#[cfg(not(pro))]
use stub as imp;
pub mod types;
pub use types::*;
