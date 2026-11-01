//! 长截图画面流：平台回调把最新一帧写进槽位，消费者只取比上次更新的那一帧（M23 规则 11）。
//! 中间的帧直接覆盖丢弃，不积压。

use crate::{PlatformError, WinRect};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// 一帧画面：RGBA、sRGB、非预乘、行紧密排列。
pub struct StreamFrame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// 开启画面流的参数。
pub struct AreaOptions {
    /// 同 `RecordOptions::display`（macOS `CGDirectDisplayID`，Windows 显示器序号）
    pub display: u32,
    /// 显示器内的点：x, y, w, h（同 `RecordOptions::rect`）
    pub rect: (f64, f64, f64, f64),
    pub scale: f64,
    /// macOS 不截进画面的窗口编号（长截图边框）；Windows 不用
    pub exclude_window: Option<u32>,
    /// 同一区域的原生坐标（同 `capture_rect`），逐帧截屏回退用
    pub native: WinRect,
}

/// 槽位内容：（序号，最新一帧，是否已失败）。
type Slot = (u64, Option<StreamFrame>, bool);

/// “最新一帧”槽位，OS 回调线程写，长截图线程读。
pub(crate) struct Latest {
    slot: Mutex<Slot>,
    cv: Condvar,
}

impl Latest {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            slot: Mutex::new((0, None, false)),
            cv: Condvar::new(),
        })
    }

    /// 覆盖旧帧，序号加一，唤醒等待者。
    pub(crate) fn push(&self, f: StreamFrame) {
        let mut g = self.slot.lock().unwrap_or_else(PoisonError::into_inner);
        g.0 += 1;
        g.1 = Some(f);
        self.cv.notify_all();
    }

    /// 标记失败并唤醒等待者。
    pub(crate) fn fail(&self) {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).2 = true;
        self.cv.notify_all();
    }
}

pub struct AreaStream {
    latest: Arc<Latest>,
    taken: Cell<u64>,
    stop: Option<Box<dyn FnOnce() + Send>>,
    pub(crate) mode: &'static str,
}

impl AreaStream {
    pub(crate) fn new(
        latest: Arc<Latest>,
        stop: Box<dyn FnOnce() + Send>,
        mode: &'static str,
    ) -> Self {
        Self {
            latest,
            taken: Cell::new(0),
            stop: Some(stop),
            mode,
        }
    }

    /// 等一帧比上次取走的更新的画面（中间的帧丢掉），最多等 `timeout`；超时或已失败为 `None`。
    pub fn next(&self, timeout: Duration) -> Option<StreamFrame> {
        let deadline = Instant::now() + timeout;
        let mut g = self
            .latest
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if g.2 {
                return None;
            }
            if g.0 > self.taken.get() {
                self.taken.set(g.0);
                return g.1.take();
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            g = self
                .latest
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    pub fn failed(&self) -> bool {
        self.latest
            .slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .2
    }

    /// 日志用："画面流" | "逐帧"。
    pub fn mode(&self) -> &'static str {
        self.mode
    }

    /// 停止；`Drop` 时也会调用，只调一次。
    pub fn stop(self) {}
}

impl Drop for AreaStream {
    fn drop(&mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

/// 长截图的区域画面流（M23 规则 13）：先试原生画面流，失败或不支持时回退逐帧截屏（每 40 ms 一帧）
pub fn start_area_stream(opts: AreaOptions) -> Result<AreaStream, PlatformError> {
    let latest = Latest::new();
    match crate::start_native_stream(&opts, latest.clone()) {
        Ok(stop) => return Ok(AreaStream::new(latest, stop, "画面流")),
        Err(e) => log::warn!("long 画面流不可用，改用逐帧截屏：{e}"),
    }
    let (native, below) = (opts.native, opts.exclude_window);
    Ok(poll_stream(
        move || crate::capture_rect(native, below),
        Duration::from_millis(40),
    ))
}

/// 逐帧截屏回退：后台线程循环调用 `grab`，至少间隔 `tick`，放进槽位；`grab` 连续失败 5 次时 fail()
pub(crate) fn poll_stream(
    grab: impl Fn() -> Result<(u32, u32, Vec<u8>), PlatformError> + Send + 'static,
    tick: Duration,
) -> AreaStream {
    let latest = Latest::new();
    let quit = Arc::new(AtomicBool::new(false));
    let (l, q) = (latest.clone(), quit.clone());
    let h = thread::spawn(move || {
        let mut errs = 0;
        while !q.load(Ordering::Relaxed) {
            let t = Instant::now();
            match grab() {
                Ok((w, h, rgba)) => {
                    errs = 0;
                    l.push(StreamFrame { w, h, rgba });
                }
                Err(e) => {
                    errs += 1;
                    log::debug!("逐帧截屏失败（连续 {errs} 次）：{e}");
                    if errs >= 5 {
                        l.fail();
                        break;
                    }
                }
            }
            thread::sleep(tick.saturating_sub(t.elapsed()));
        }
    });
    let stop = Box::new(move || {
        quit.store(true, Ordering::Relaxed);
        // Drop 里不能 panic：忽略线程 join 错误
        let _ = h.join();
    });
    AreaStream::new(latest, stop, "逐帧")
}

/// BGRA（每行 `stride` 字节，可能有补齐）转成行紧密排列的 RGBA。屏幕画面不透明：alpha 置 255，
/// 预乘与否没有区别，即非预乘。
pub(crate) fn bgra_to_rgba(src: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in src.chunks(stride).take(h) {
        for p in row[..w * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::thread;

    fn frame(w: u32) -> StreamFrame {
        StreamFrame {
            w,
            h: 1,
            rgba: vec![0; 4 * w as usize],
        }
    }

    fn stream(latest: &Arc<Latest>) -> AreaStream {
        AreaStream::new(latest.clone(), Box::new(|| {}), "画面流")
    }

    #[test]
    fn next_returns_only_the_newest_frame() {
        let l = Latest::new();
        let s = stream(&l);
        for w in 1..=3 {
            l.push(frame(w));
        }
        assert_eq!(s.next(Duration::from_millis(10)).map(|f| f.w), Some(3));
        assert!(s.next(Duration::from_millis(10)).is_none());
    }

    #[test]
    fn next_times_out_without_new_frames() {
        let s = stream(&Latest::new());
        let t = Instant::now();
        assert!(s.next(Duration::from_millis(30)).is_none());
        let e = t.elapsed();
        assert!(
            e >= Duration::from_millis(30) && e < Duration::from_millis(200),
            "{e:?}"
        );
    }

    #[test]
    fn next_wakes_on_push_from_another_thread() {
        let l = Latest::new();
        let s = stream(&l);
        let l2 = l.clone();
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            l2.push(frame(7));
        });
        let t = Instant::now();
        assert_eq!(s.next(Duration::from_secs(1)).map(|f| f.w), Some(7));
        assert!(t.elapsed() < Duration::from_millis(200));
        h.join().unwrap();
    }

    #[test]
    fn fail_wakes_waiters_and_sets_failed() {
        let l = Latest::new();
        let s = stream(&l);
        let l2 = l.clone();
        let h = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            l2.fail();
        });
        let t = Instant::now();
        assert!(s.next(Duration::from_secs(1)).is_none());
        assert!(t.elapsed() < Duration::from_millis(200));
        assert!(s.failed());
        h.join().unwrap();
    }

    #[test]
    fn stop_runs_once() {
        let n = Arc::new(AtomicU32::new(0));
        let mk = || {
            let n = n.clone();
            AreaStream::new(
                Latest::new(),
                Box::new(move || {
                    n.fetch_add(1, Ordering::SeqCst);
                }),
                "逐帧",
            )
        };
        mk().stop();
        assert_eq!(n.load(Ordering::SeqCst), 1);
        drop(mk());
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn poll_stream_delivers_grabbed_frames() {
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        let alive = Arc::new(AtomicBool::new(false));
        let a2 = alive.clone();
        let s = poll_stream(
            move || {
                a2.store(true, Ordering::SeqCst);
                let w = n2.fetch_add(1, Ordering::SeqCst) + 1;
                Ok((w, 1, vec![0; 4 * w as usize]))
            },
            Duration::from_millis(10),
        );
        let f = s.next(Duration::from_millis(200)).expect("应有帧");
        assert_eq!((f.h, f.rgba.len()), (1, 4 * f.w as usize));
        assert_eq!(s.mode(), "逐帧");
        s.stop();
        // stop 已 join 线程：之后不再有 grab 调用
        let after = n.load(Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        assert_eq!(n.load(Ordering::SeqCst), after);
        assert!(alive.load(Ordering::SeqCst));
    }

    #[test]
    fn poll_stream_fails_after_repeated_errors() {
        let s = poll_stream(
            || Err(PlatformError::Capture("x".into())),
            Duration::from_millis(10),
        );
        let t = Instant::now();
        while !s.failed() && t.elapsed() < Duration::from_millis(200) {
            thread::sleep(Duration::from_millis(5));
        }
        assert!(s.failed());
    }

    #[test]
    fn bgra_to_rgba_swaps_channels_and_drops_row_padding() {
        // 2×2，每行补齐到 12 字节，最后一行不带补齐
        let src = [
            1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, //
            7, 8, 9, 254, 10, 11, 12, 255,
        ];
        assert_eq!(
            bgra_to_rgba(&src, 2, 2, 12),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }
}
