//! 长截图拼接（M23 规则 13）：同一区域连续截到的帧（RGBA、行紧密排列、宽高不变），逐行比对找出这一帧落在已拼好的
//! 长图的什么位置，把新露出的行接进结果。顶部、底部不随滚动变化的行（吸顶的标题栏、底部工具栏）只保留一份：
//! 标题栏留在结果最上面，底栏留在最下面。每帧底部的几行（窗口圆角、渐隐、没画完的半行）先不接，
//! 等它们滚上来、在下一帧里画好了再接；结束时补上最后一帧的底部。
//!
//! 新帧在长图最后几屏里找位置，而不只和上一帧比：滚得太快接不上时，往回滚到预览里已有的任何位置都能接着拼。
//!
//! 只按行哈希精确比对。试过对不上时按亮度轮廓近似比对、再逐像素核对（应对小数像素滚动、每帧带噪点的画面），
//! 一行行很像的内容（每行只差几个字）在真实截屏上会错开整行接上、悄悄弄坏长图，所以不做：宁可报没接上（见已知限制）。

use std::hash::{DefaultHasher, Hash, Hasher};

/// 结果最多这么多像素（约 190 MB 的 RGBA）；行数另有上限。到上限后不再接
pub const MAX_PIXELS: usize = 48_000_000;
pub const MAX_ROWS: usize = 32_000;
/// 新帧在长图最后这么多屏里找位置
const SEARCH_SCREENS: usize = 4;

#[derive(Debug, Clone, Copy)]
struct Row {
    hash: u64,
    /// 整行同一种颜色（空白处）：能和任何空白行对上，不能作为对齐的依据
    plain: bool,
}

/// `push` 的结果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// 没有新内容：画面没动、只有零星几行变化（光标闪烁），或往回滚到了已拼好的位置
    Same,
    /// 长图长高了这么多行
    Added(usize),
    /// 在长图最后几屏里找不到这一帧：滚得太快（两帧之间超过一屏）、往回滚得太远，或画面整体变了。
    /// 不接，往回滚到已拼好的部分就能接着拼
    Lost,
    /// 已到上限，不再接
    Full,
}

pub struct Stitch {
    w: usize,
    h: usize,
    /// 每行只比左边这么多像素：右侧的滚动条随滚动移动，整列都会对不上
    cmp_w: usize,
    /// 每帧底部这么多行先不接，等它们滚上来再接：窗口的圆角、渐隐、还没画完的半行都在这里
    guard: usize,
    /// 上一次送进来的帧（不论接没接上）：看画面动没动、哪些行不随滚动变化
    prev: Vec<Row>,
    /// 上一次接上的那一帧，结束时补上它从 `end` 行起的部分（底部不随滚动变化的行也在里面）
    last: Vec<u8>,
    last_rows: Vec<Row>,
    out: Vec<u8>,
    out_rows: Vec<Row>,
    /// `out` 的最后一行是 `last` 的第 `end - 1` 行
    end: usize,
    max_rows: usize,
    /// 到过上限：之后一律报 `Full`
    full: bool,
}

impl Stitch {
    /// 第一帧作为结果的开头。`skip_right`：比对时忽略右侧的像素数（滚动条）；`guard`：每帧底部暂不接的行数，
    /// 最多帧高的 1/4。
    pub fn new(w: usize, h: usize, first: &[u8], skip_right: usize, guard: usize) -> Self {
        assert_eq!(first.len(), w * h * 4, "帧尺寸不符");
        let cmp_w = w.saturating_sub(skip_right).max(w.min(1));
        let guard = guard.min(h / 4);
        let end = h - guard;
        let r = rows(first, w, cmp_w);
        Self {
            w,
            h,
            cmp_w,
            guard,
            out: first[..end * w * 4].to_vec(),
            out_rows: r[..end].to_vec(),
            prev: r.clone(),
            last: first.to_vec(),
            last_rows: r,
            end,
            max_rows: (MAX_PIXELS / w.max(1)).min(MAX_ROWS).max(h),
            full: false,
        }
    }

    /// 结果当前的行数（含结束时才补上的最后一帧底部）
    pub fn height(&self) -> usize {
        self.out_rows.len() + self.h - self.end
    }

    /// 接上一帧；接上时这一帧留作 `last`，不再拷贝。
    pub fn push(&mut self, cur: Vec<u8>) -> Step {
        assert_eq!(cur.len(), self.w * self.h * 4, "帧尺寸不符");
        if self.full {
            return Step::Full;
        }
        let b = rows(&cur, self.w, self.cmp_w);
        let h = self.h;
        let same = |x: &Row, y: &Row| x.hash == y.hash;
        if self.prev.iter().zip(&b).all(|(x, y)| same(x, y)) {
            return Step::Same;
        }
        // 不随滚动变化的顶部、底部；中间至少留一行
        let top = self
            .prev
            .iter()
            .zip(&b)
            .take_while(|(x, y)| same(x, y))
            .count()
            .min(h - 1);
        let bottom = self
            .prev
            .iter()
            .rev()
            .zip(b.iter().rev())
            .take_while(|(x, y)| same(x, y))
            .count()
            .min(h - top - 1);
        self.prev.clone_from(&b);
        let (lo, hi) = (top, h - bottom);
        // 只有零星几行变了（光标闪烁、悬停高亮）：没有滚动
        if hi - lo <= h / 20 {
            return Step::Same;
        }
        let row = self.w * 4;
        // 不动的底部比暂不接的行高（聊天输入框、状态栏）：已接进结果的那几行其实是底栏，退回去，结束时随最后一帧补上
        if self.end > hi {
            let n = (self.end - hi).min(self.out_rows.len());
            self.out_rows.truncate(self.out_rows.len() - n);
            self.out.truncate(self.out.len() - n * row);
            self.end -= n;
        }
        // 已拼好的长图：已接的行，加上上次接上那帧里还没接的行（到不动的底部为止）
        let done = self.out_rows.len();
        let pend = &self.last_rows[self.end..hi.max(self.end)];
        let len = done + pend.len();
        let at = |i: usize| {
            if i < done {
                self.out_rows[i]
            } else {
                pend[i - done]
            }
        };
        // 这一帧第 lo 行落在长图第 p 行
        let window = SEARCH_SCREENS * h;
        let Some(p) = locate(len, at, &b[lo..hi], window) else {
            return Step::Lost;
        };
        let to = hi.saturating_sub(self.guard).max(lo);
        // 长图的末尾是这一帧的第 lo + len - p 行；这一帧可靠的部分没超出它：没有新内容（往回滚了，或没动）
        if lo + len - p >= to {
            return Step::Same;
        }
        // 从长图的第 max(done, p) 行起换成这一帧的行（还没接的行以这一帧为准）
        let from = lo + done.max(p) - p;
        let grown = done.max(p) + h - from;
        if grown > self.max_rows {
            self.full = true;
            return Step::Full;
        }
        let before = self.height();
        if p > done {
            let (a, z) = (self.end, self.end + p - done);
            self.out.extend_from_slice(&self.last[a * row..z * row]);
            self.out_rows.extend_from_slice(&self.last_rows[a..z]);
        }
        self.out.extend_from_slice(&cur[from * row..to * row]);
        self.out_rows.extend_from_slice(&b[from..to]);
        self.last = cur;
        self.last_rows = b;
        self.end = to;
        Step::Added(self.height().saturating_sub(before))
    }

    /// 长截图期间旁边的预览（M23 规则 13）：把拼好的图缩到宽 `pw`，只取最下面最多 `max_ph` 行（最新接上的部分，
    /// 用户据此看接缝对不对）。每个输出像素取源区域里 2 × 2 个采样点的平均，只碰输出大小的像素，几毫秒。
    /// 返回宽、高、RGBA。
    pub fn preview(&self, pw: usize, max_ph: usize) -> (usize, usize, Vec<u8>) {
        let row = self.w * 4;
        let done = self.out_rows.len();
        let total = self.height();
        let pw = pw.clamp(1, self.w);
        let k = self.w as f64 / pw as f64;
        let ph = ((total as f64 / k).round() as usize).clamp(1, max_ph.max(1));
        // 预览第 0 行对应源图这一行
        let y0 = total as f64 - ph as f64 * k;
        let src = |y: usize| -> &[u8] {
            if y < done {
                &self.out[y * row..(y + 1) * row]
            } else {
                let y = self.end + y - done;
                &self.last[y * row..(y + 1) * row]
            }
        };
        let mut px = Vec::with_capacity(pw * ph * 4);
        for oy in 0..ph {
            let ys = [0.25, 0.75].map(|d| ((y0 + (oy as f64 + d) * k) as usize).min(total - 1));
            for ox in 0..pw {
                let xs = [0.25, 0.75].map(|d| (((ox as f64 + d) * k) as usize).min(self.w - 1));
                let mut sum = [0u32; 4];
                for y in ys {
                    let r = src(y);
                    for x in xs {
                        for (c, v) in sum.iter_mut().zip(&r[x * 4..x * 4 + 4]) {
                            *c += u32::from(*v);
                        }
                    }
                }
                px.extend(sum.map(|c| (c / 4) as u8));
            }
        }
        (pw, ph, px)
    }

    /// 拼好的图：宽、高、RGBA。
    pub fn finish(mut self) -> (usize, usize, Vec<u8>) {
        let row = self.w * 4;
        self.out.extend_from_slice(&self.last[self.end * row..]);
        (self.w, self.out.len() / row, self.out)
    }
}

fn rows(px: &[u8], w: usize, cmp_w: usize) -> Vec<Row> {
    px.chunks_exact(w * 4)
        .map(|r| {
            let r = &r[..cmp_w * 4];
            let mut s = DefaultHasher::new();
            r.hash(&mut s);
            let px = r.as_chunks::<4>().0;
            Row {
                hash: s.finish(),
                plain: px.windows(2).all(|p| p[0] == p[1]),
            }
        })
        .collect()
}

/// 在长图（共 `len` 行，`at(i)` 取第 i 行）最后 `window` 行里找 `band` 的位置：`band[0]` 对应长图第几行。
/// 重叠至少 1/5（不少于 8 行），对不上的行不超过重叠的 1/4（帧边上的圆角、渐隐、悬停高亮都会对不上），
/// 至少 4 行非空白行对上；取对上的非空白行最多的位置，一样多取靠后的（离当前滚动位置最近）。
/// 还要明显胜出：对上的非空白行至少是离它 3 行以外的次好位置的 2 倍。一行行很像的内容（代码、表格、
/// 每行只差几个字的列表）错开整行也能对上不少行，重叠又少时分不清，宁可报没接上，不能接错。
/// ponytail: 逐位置比对，最坏约 窗口行数 × 帧高 次比较（2 倍屏整屏约千万次、十几毫秒）；不够快时先按非空白行的哈希找候选位置。
fn locate(len: usize, at: impl Fn(usize) -> Row, band: &[Row], window: usize) -> Option<usize> {
    let m = band.len();
    let min = min_overlap(m);
    let mut ok = Vec::new();
    for p in len.saturating_sub(window)..=len.saturating_sub(min) {
        let overlap = m.min(len - p);
        let limit = overlap / 4;
        let (mut miss, mut hits) = (0, 0);
        for (k, y) in band[..overlap].iter().enumerate() {
            let x = at(p + k);
            if x.hash != y.hash {
                miss += 1;
                if miss > limit {
                    break;
                }
            } else if !x.plain {
                hits += 1;
            }
        }
        if miss <= limit && hits >= 4 {
            ok.push((p, hits));
        }
    }
    let &(p, best) = ok.iter().max_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)))?;
    let far = ok
        .iter()
        .filter(|(q, _)| q.abs_diff(p) > 3)
        .map(|s| s.1)
        .max()
        .unwrap_or(0);
    (best >= 2 * far).then_some(p)
}

/// 找位置时要求的最少重叠行数：帧（去掉不动的顶部、底部）的 1/5，不少于 8 行
fn min_overlap(m: usize) -> usize {
    (m / 5).max(8).min(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 6;

    /// 一张“长页面”：第 i 行每个像素都由 i 编码出颜色，每行都不同，也不是纯色
    fn page(n: usize) -> Vec<[u8; 4]> {
        (0..n)
            .map(|i| [(i % 251) as u8, (i / 251) as u8, 7, 255])
            .collect()
    }

    /// 从页面第 `y` 行起截 `h` 行：`header`、`footer` 行固定为纯色（吸顶、吸底），滚动条在最右一列，
    /// 最下面 `corner` 行最左一个像素是窗口圆角外的黑色
    fn shot(
        p: &[[u8; 4]],
        y: usize,
        h: usize,
        header: usize,
        footer: usize,
        corner: usize,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        for r in 0..h {
            for x in 0..W {
                let px = if x == 0 && r >= h - corner {
                    [0, 0, 0, 255]
                } else if r < header {
                    [200, 0, 0, 255]
                } else if r >= h - footer {
                    [0, 200, 0, 255]
                } else if x == W - 1 {
                    [y as u8, 0, 0, 255]
                } else {
                    let mut c = p[y + r];
                    c[2] = x as u8;
                    c
                };
                out.extend_from_slice(&px);
            }
        }
        out
    }

    /// 去掉滚动条那一列（来自不同的帧）再比
    fn strip(v: &[u8]) -> Vec<u8> {
        v.as_chunks::<{ W * 4 }>()
            .0
            .iter()
            .flat_map(|r| r[..(W - 1) * 4].to_vec())
            .collect()
    }

    #[test]
    fn stitches_scrolling_page_with_sticky_header_footer() {
        let p = page(400);
        let (h, hd, ft) = (40, 5, 3);
        let f = |y| shot(&p, y, h, hd, ft, 0);
        let mut s = Stitch::new(W, h, &f(0), 1, 4);
        assert_eq!(s.push(f(0)), Step::Same);
        assert_eq!(s.push(f(7)), Step::Added(7));
        assert_eq!(s.push(f(30)), Step::Added(23));
        // 往回滚到已拼好的部分：没有新内容；再往下滚接着拼
        assert_eq!(s.push(f(10)), Step::Same);
        assert_eq!(s.push(f(50)), Step::Added(20));
        assert_eq!(s.height(), h + 50);
        let (w, rows, px) = s.finish();
        assert_eq!((w, rows), (W, h + 50));
        // 等于一张高 h + 50 的窗口一次截下：顶栏、页面第 hd..50 + h - ft 行、底栏
        assert_eq!(strip(&px), strip(&shot(&p, 0, h + 50, hd, ft, 0)));
    }

    #[test]
    fn footer_taller_than_guard() {
        let p = page(400);
        let f = |y| shot(&p, y, 40, 0, 8, 0);
        let mut s = Stitch::new(W, 40, &f(0), 1, 4);
        assert_eq!(s.push(f(7)), Step::Added(7));
        assert_eq!(s.push(f(20)), Step::Added(13));
        let (_, rows, px) = s.finish();
        assert_eq!(rows, 60);
        assert_eq!(strip(&px), strip(&shot(&p, 0, 60, 0, 8, 0)));
    }

    #[test]
    fn window_corners_stay_only_at_the_bottom() {
        let p = page(400);
        let f = |y| shot(&p, y, 40, 0, 0, 3);
        let mut s = Stitch::new(W, 40, &f(0), 1, 4);
        assert_eq!(s.push(f(12)), Step::Added(12));
        assert_eq!(s.push(f(31)), Step::Added(19));
        let (_, rows, px) = s.finish();
        assert_eq!(rows, 71);
        assert_eq!(strip(&px), strip(&shot(&p, 0, 71, 0, 0, 3)));
    }

    #[test]
    fn preview_shows_the_newest_rows() {
        let p = page(400);
        let f = |y| shot(&p, y, 40, 0, 0, 0);
        let mut s = Stitch::new(W, 40, &f(0), 1, 4);
        assert_eq!(s.push(f(30)), Step::Added(30));
        // 原尺寸、最多 10 行：是长图最下面 10 行（含最后一帧暂不接的底部）
        let (w, h, px) = s.preview(W, 10);
        assert_eq!((w, h), (W, 10));
        assert_eq!(strip(&px), strip(&shot(&p, 60, 10, 0, 0, 0)));
        // 缩小一半：高度跟着减半，不超过上限
        let (w, h, _) = s.preview(W / 2, 1000);
        assert_eq!((w, h), (W / 2, 35));
    }

    #[test]
    fn jump_beyond_one_screen_is_lost_until_scrolled_back() {
        let p = page(400);
        let f = |y| shot(&p, y, 40, 0, 0, 0);
        let mut s = Stitch::new(W, 40, &f(0), 1, 4);
        assert_eq!(s.push(f(20)), Step::Added(20));
        assert_eq!(s.push(f(100)), Step::Lost);
        assert_eq!(s.push(f(140)), Step::Lost);
        assert_eq!(s.height(), 60);
        // 滚回到和已拼好的部分有重叠的任何位置（这里比上次接上的还靠上）都能接着拼
        assert_eq!(s.push(f(5)), Step::Same);
        assert_eq!(s.push(f(45)), Step::Added(25));
        let (_, rows, px) = s.finish();
        assert_eq!(rows, 85);
        assert_eq!(strip(&px), strip(&shot(&p, 0, 85, 0, 0, 0)));
    }

    #[test]
    fn few_changed_rows_are_not_a_scroll() {
        let blank = vec![255u8; W * 40 * 4];
        let mut other = blank.clone();
        other[..4].copy_from_slice(&[0, 0, 0, 255]);
        let mut s = Stitch::new(W, 40, &blank, 0, 0);
        assert_eq!(s.push(other), Step::Same, "只有一行变化不算滚动");
    }

    #[test]
    fn stops_at_max_rows() {
        let p = page(400);
        let f = |y| shot(&p, y, 40, 0, 0, 0);
        let mut s = Stitch::new(W, 40, &f(0), 1, 0);
        s.max_rows = 50;
        assert_eq!(s.push(f(8)), Step::Added(8));
        assert_eq!(s.push(f(11)), Step::Full);
        // 到上限后接着滚过一屏也仍是 Full，不变成对不上
        assert_eq!(s.push(f(200)), Step::Full);
        assert_eq!(s.finish().1, 48);
    }

    /// 模拟别的设备：宽 64，内容在竖直方向平滑（像文字笔画），帧从小数像素位置 `y` 起线性插值（Windows 125% / 150%
    /// 缩放下一个逻辑像素的滚动是小数个物理像素），每个像素再加上 ±`noise` 的抖动（广色域、HDR 换算与抖色）。
    struct Dev {
        rows: Vec<[f32; 64]>,
    }

    impl Dev {
        fn new(n: usize, seed: u32) -> Self {
            let mut x = seed;
            let mut rnd = move || {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (x >> 8) as f32 / (1 << 24) as f32
            };
            let raw: Vec<[f32; 64]> = (0..n + 8)
                .map(|_| std::array::from_fn(|_| rnd() * 255.0))
                .collect();
            // 竖直方向 5 行平均
            let rows = (0..n)
                .map(|i| std::array::from_fn(|c| (0..5).map(|k| raw[i + k][c]).sum::<f32>() / 5.0))
                .collect();
            Self { rows }
        }

        fn shot(&self, y: f32, h: usize, noise: u8, seed: u32) -> Vec<u8> {
            let mut x = seed;
            let mut out = Vec::with_capacity(64 * h * 4);
            for r in 0..h {
                let f = y + r as f32;
                let (i, t) = (f.floor() as usize, f.fract());
                for c in 0..64 {
                    let v = self.rows[i][c] * (1.0 - t) + self.rows[i + 1][c] * t;
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    let j = if noise == 0 {
                        0.0
                    } else {
                        (x >> 24) as f32 % (2.0 * f32::from(noise) + 1.0) - f32::from(noise)
                    };
                    let v = (v + j).clamp(0.0, 255.0) as u8;
                    out.extend_from_slice(&[v, v, v, 255]);
                }
            }
            out
        }
    }

    /// 已知限制：小数像素滚动、每帧带噪点时精确比对接不上，但一定报没接上，不会接错
    #[test]
    fn fractional_or_noisy_frames_are_lost_not_misjoined() {
        let d = Dev::new(600, 7);
        let mut s = Stitch::new(64, 60, &d.shot(0.0, 60, 0, 1), 0, 4);
        for y in [13.5, 37.25, 61.75] {
            assert_eq!(s.push(d.shot(y, 60, 0, 2)), Step::Lost, "y = {y}");
        }
        // 整数像素、没有噪点时照常接上
        assert_eq!(s.push(d.shot(20.0, 60, 0, 3)), Step::Added(20));
        let mut s = Stitch::new(64, 60, &d.shot(0.0, 60, 2, 1), 0, 4);
        for (i, y) in [10.0, 30.0].into_iter().enumerate() {
            assert_eq!(s.push(d.shot(y, 60, 2, 10 + i as u32)), Step::Lost);
        }
        assert_eq!(s.height(), 60);
    }

    #[test]
    fn unrelated_content_is_lost_not_misjoined() {
        let (a, b) = (Dev::new(600, 3), Dev::new(600, 4));
        let mut s = Stitch::new(64, 60, &a.shot(0.0, 60, 2, 1), 0, 4);
        assert_eq!(s.push(b.shot(20.0, 60, 2, 2)), Step::Lost);
        assert_eq!(s.height(), 60);
    }

    /// 一行行几乎一样的内容（每行 8 像素高，只有最左两列编码行号，其余是同一行“文字”）：
    /// 甩过已拼好的部分时不能错开整行接上，滚回来后正常接
    #[test]
    fn repetitive_lines_never_misjoin() {
        let glyph: [u8; 8] = [0, 200, 60, 200, 120, 30, 200, 0];
        let shot = |y: usize, noise: bool| -> Vec<u8> {
            let mut out = Vec::new();
            for r in 0..60 {
                let (line, k) = ((y + r) / 8, (y + r) % 8);
                for c in 0..64u8 {
                    let v = if c < 2 {
                        (line * 37 % 251) as u8
                    } else {
                        glyph[k].wrapping_add(c % 3)
                    };
                    let v = if noise {
                        v.saturating_add(((r + usize::from(c)) % 3) as u8)
                    } else {
                        v
                    };
                    out.extend_from_slice(&[v, v, v, 255]);
                }
            }
            out
        };
        let mut s = Stitch::new(64, 60, &shot(0, false), 0, 4);
        assert_eq!(s.push(shot(20, false)), Step::Added(20));
        for y in [90, 130, 170] {
            assert_eq!(
                s.push(shot(y, false)),
                Step::Lost,
                "y = {y} 已超出拼好的部分"
            );
        }
        assert_eq!(s.height(), 80);
        assert_eq!(s.push(shot(40, false)), Step::Added(20));
        // 带噪点：一帧也接不上（已知限制），但不接错
        let mut s = Stitch::new(64, 60, &shot(0, true), 0, 4);
        for y in [20, 90, 130, 40] {
            assert_eq!(s.push(shot(y, true)), Step::Lost, "噪点下 y = {y}");
        }
    }
}
