//! 传送带和搅拌机同时开着时，新复制归谁：归后打开的那一个。

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

/// 全局递增的戳，打开时取一个
pub fn stamp() -> u64 {
    NEXT.fetch_add(1, Ordering::SeqCst) + 1
}

#[derive(Debug, PartialEq)]
pub enum Taker {
    Belt,
    Mixer,
}

/// 开着的里戳大的那个（`None` 表示收着）
pub fn taker(belt: Option<u64>, mixer: Option<u64>) -> Option<Taker> {
    match (belt, mixer) {
        (Some(b), Some(m)) if m > b => Some(Taker::Mixer),
        (Some(_), _) => Some(Taker::Belt),
        (None, Some(_)) => Some(Taker::Mixer),
        (None, None) => None,
    }
}

#[derive(Debug, PartialEq)]
pub enum Toggle {
    Open,
    Expand,
    Mini,
    Close,
}

/// 快捷键按下：关着就打开；角标时展开；展开时有东西收成角标、空着关掉
pub fn toggle(open: bool, mini: bool, has_items: bool) -> Toggle {
    match (open, mini, has_items) {
        (false, _, _) => Toggle::Open,
        (true, true, _) => Toggle::Expand,
        (true, false, true) => Toggle::Mini,
        (true, false, false) => Toggle::Close,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_by_state() {
        assert_eq!(toggle(false, false, false), Toggle::Open);
        assert_eq!(toggle(true, true, true), Toggle::Expand);
        assert_eq!(toggle(true, false, true), Toggle::Mini);
        assert_eq!(toggle(true, false, false), Toggle::Close);
    }

    #[test]
    fn later_opener_takes() {
        assert_eq!(taker(Some(3), Some(5)), Some(Taker::Mixer));
        assert_eq!(taker(Some(6), Some(5)), Some(Taker::Belt));
        assert_eq!(taker(None, Some(5)), Some(Taker::Mixer));
        assert_eq!(taker(Some(2), None), Some(Taker::Belt));
        assert_eq!(taker(None, None), None);
        assert!(stamp() < stamp());
    }
}
