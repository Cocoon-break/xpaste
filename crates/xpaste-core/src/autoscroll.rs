//! 长截图自动滚动的节奏（M23 规则 13）：纯逻辑，不碰系统、不看时间。
//! 调用方发一步滚动，等画面停下，再把这一步让长图位置变了多少（或没接上）报回来，由这里决定下一步。
//!
//! 起步每步约选区高度的 1/3，连续接上逐步加大，不超过 2/3；没接上就往回滚半步、步长减半；
//! 滚动单位换算成像素的比例按每步实际位移边走边学。

/// 步长放大倍数
const GROW: f64 = 1.25;
/// 连续这么多次没动 / 恢复不了就下结论
const LIMIT: u32 = 3;

pub struct PacerCfg {
    /// 选区高度（物理像素）
    pub frame_h: usize,
    /// 滚动单位换算成像素的初始猜测（平台给：macOS 1 点 = scale 像素；Windows 120 = 一格，约 100 × scale 像素）
    pub px_per_unit: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Act {
    /// 发这么多单位的滚动（正数往下，负数往回），并期望画面往下移动约 `expect_px` 像素（往回时为 0）
    Scroll { units: i32, expect_px: usize },
    /// 到底：按完成收尾
    Finish,
    /// 从没动过：转手动并提示“不响应”
    NoResponse,
    /// 连续 3 次恢复不了：暂停并提示“这里一直接不上”
    Stuck,
}

pub struct Pacer {
    frame_h: f64,
    ratio: f64,
    /// 当前步长（像素）
    step: f64,
    /// 连续没动的步数
    still: u32,
    /// 连续没接上的次数
    fails: u32,
    /// 页面在本次会话里动过没有（暂停、接手后保留）
    moved_ever: bool,
    /// 比例是否已由真实的一步学过
    measured: bool,
    /// 上一步是往回滚的恢复步
    backward: bool,
    /// 上一步往下滚的单位数
    last_units: i32,
}

impl Pacer {
    pub fn new(cfg: PacerCfg) -> Self {
        let frame_h = cfg.frame_h as f64;
        Pacer {
            frame_h,
            ratio: cfg.px_per_unit,
            step: frame_h / 3.0,
            still: 0,
            fails: 0,
            moved_ever: false,
            measured: false,
            backward: false,
            last_units: 1,
        }
    }

    /// 第一步（或暂停、用户接手之后重新开始）：步长回到起步值，学到的比例和“动过”保留
    pub fn start(&mut self) -> Act {
        self.step = self.frame_h / 3.0;
        self.still = 0;
        self.fails = 0;
        self.backward = false;
        self.forward()
    }

    /// 上一步发出后画面停下了：`moved` 为长图上位置的变化（像素，往回为负），`None` 为没接上
    pub fn settled(&mut self, moved: Option<i64>) -> Act {
        let Some(m) = moved else {
            self.fails += 1;
            self.still = 0;
            if self.fails >= LIMIT {
                return Act::Stuck;
            }
            // 往回滚上一步的一半，步长减半
            let back = self.step / 2.0;
            self.step = back.max(self.frame_h / 12.0);
            self.backward = true;
            return Act::Scroll {
                units: -self.units_for(back),
                expect_px: 0,
            };
        };
        self.fails = 0;
        if self.backward {
            // 恢复步接上了：回到往下滚，不学比例、不算到底，步长保持减半后的值
            self.backward = false;
            return self.forward();
        }
        if m > 0 {
            self.still = 0;
            self.moved_ever = true;
            // 新值权重 0.5
            self.ratio = (self.ratio + m as f64 / self.last_units as f64) / 2.0;
            self.measured = true;
            self.step = (self.step * GROW).min(self.frame_h * 2.0 / 3.0);
        } else {
            self.still += 1;
            if self.still >= LIMIT {
                return if self.moved_ever {
                    Act::Finish
                } else {
                    Act::NoResponse
                };
            }
        }
        self.forward()
    }

    /// 日志用：学到的比例（像素 / 单位）
    pub fn px_per_unit(&self) -> f64 {
        self.ratio
    }

    /// 比例是否已由至少一步真实移动学到（调用方据此决定要不要把预期位移交给拼接）
    pub fn measured(&self) -> bool {
        self.measured
    }

    /// 像素换成单位，四舍五入但至少 1（不会发出 0 的滚动）
    fn units_for(&self, px: f64) -> i32 {
        ((px / self.ratio).round() as i32).max(1)
    }

    fn forward(&mut self) -> Act {
        let units = self.units_for(self.step);
        self.last_units = units;
        Act::Scroll {
            units,
            expect_px: (units as f64 * self.ratio).round() as usize,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pacer(guess: f64) -> Pacer {
        Pacer::new(PacerCfg {
            frame_h: 900,
            px_per_unit: guess,
        })
    }

    fn units(a: Act) -> i32 {
        match a {
            Act::Scroll { units, .. } => units,
            other => panic!("不是 Scroll：{other:?}"),
        }
    }

    fn expect(a: Act) -> usize {
        match a {
            Act::Scroll { expect_px, .. } => expect_px,
            other => panic!("不是 Scroll：{other:?}"),
        }
    }

    /// 模拟页面：移动 = 单位 × 真实比例
    fn feed(p: &mut Pacer, a: Act, real: f64) -> Act {
        p.settled(Some((units(a) as f64 * real).round() as i64))
    }

    #[test]
    fn first_step_is_a_third_of_the_frame() {
        let mut p = pacer(2.0);
        assert_eq!(
            p.start(),
            Act::Scroll {
                units: 150,
                expect_px: 300
            }
        );
    }

    #[test]
    fn learns_the_real_ratio() {
        let mut p = pacer(2.0);
        let mut a = p.start();
        for _ in 0..4 {
            a = feed(&mut p, a, 3.0);
        }
        assert!((p.px_per_unit() - 3.0).abs() < 0.1);
        let want = units(a) as f64 * 3.0;
        assert!((expect(a) as f64 - want).abs() / want < 0.05);
    }

    #[test]
    fn grows_to_two_thirds_and_no_more() {
        let mut p = pacer(2.0);
        let mut a = p.start();
        for _ in 0..12 {
            assert!(expect(a) <= 600);
            a = feed(&mut p, a, 2.0);
        }
        assert!(expect(a) >= 590 && expect(a) <= 600);
    }

    #[test]
    fn lost_backs_off_and_recovers() {
        let mut p = pacer(2.0);
        let a = p.start();
        let back = p.settled(None);
        assert_eq!(units(back), -(units(a) / 2));
        assert_eq!(expect(back), 0);
        // 恢复步接上，下一步是原来的一半
        let a2 = p.settled(Some(-150));
        assert_eq!(units(a2), units(a) / 2);
        let a3 = feed(&mut p, a2, 2.0);
        let a4 = feed(&mut p, a3, 2.0);
        assert!(units(a3) > units(a2) && units(a4) > units(a3));
    }

    #[test]
    fn three_failed_recoveries_are_stuck() {
        let mut p = pacer(2.0);
        p.start();
        assert!(matches!(p.settled(None), Act::Scroll { .. }));
        assert!(matches!(p.settled(None), Act::Scroll { .. }));
        assert_eq!(p.settled(None), Act::Stuck);
    }

    #[test]
    fn bottom_finishes_after_three_still_steps() {
        let mut p = pacer(2.0);
        let mut a = p.start();
        for _ in 0..3 {
            a = feed(&mut p, a, 2.0);
        }
        let _ = a;
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
        assert_eq!(p.settled(Some(0)), Act::Finish);
    }

    #[test]
    fn backward_step_does_not_count_toward_bottom_or_ratio() {
        let mut p = pacer(2.0);
        let a = p.start();
        feed(&mut p, a, 2.0);
        p.settled(None);
        let r = p.px_per_unit();
        // 恢复步位移为负：不学比例、不算到底
        assert!(matches!(p.settled(Some(-100)), Act::Scroll { .. }));
        assert_eq!(p.px_per_unit(), r);
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
    }

    #[test]
    fn never_moved_is_no_response() {
        let mut p = pacer(2.0);
        p.start();
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
        assert!(matches!(p.settled(Some(0)), Act::Scroll { .. }));
        assert_eq!(p.settled(Some(0)), Act::NoResponse);
    }

    #[test]
    fn restart_keeps_ratio_not_user_distance() {
        let mut p = pacer(2.0);
        let mut a = p.start();
        for _ in 0..4 {
            a = feed(&mut p, a, 3.0);
        }
        let _ = a;
        let r = p.px_per_unit();
        let first = p.start();
        assert_eq!(p.px_per_unit(), r);
        assert_eq!(units(first), (300.0 / r).round() as i32);
        // 接手前动过，重新开始后连续不动仍是到底而不是不响应
        p.settled(Some(0));
        p.settled(Some(0));
        assert_eq!(p.settled(Some(0)), Act::Finish);
    }

    #[test]
    fn measured_after_first_real_move() {
        let mut p = pacer(2.0);
        let a = p.start();
        assert!(!p.measured());
        feed(&mut p, a, 2.0);
        assert!(p.measured());
    }

    #[test]
    fn units_are_never_zero() {
        let mut p = pacer(10_000.0);
        assert_eq!(units(p.start()), 1);
        assert_eq!(units(p.settled(None)), -1);
    }
}
