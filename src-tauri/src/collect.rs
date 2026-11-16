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

#[cfg(test)]
mod tests {
    use super::*;

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
