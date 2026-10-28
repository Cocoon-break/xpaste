//! 录屏会话（M24）：开始、停止、边框窗口 `rec-frame`、菜单栏计时与“停止录制”、录完写剪切板与系统通知。
//!
//! 线程：`rec_start` 在阻塞线程里跑 `start`（取屏幕、问麦克风权限、`start_recording` 都会阻塞）；
//! 录制中一个 `rec-tick` 线程每秒更新菜单栏标题、轮询中断；`stop` 把 `Recorder::stop`（阻塞到文件写完）
//! 与收尾放到后台线程；`stop_blocking` 在当前线程做完，退出前用。
//!
//! 主线程从不等别的线程（开始线程要切主线程建窗口、改菜单栏）：退出时别的线程上的开始或停止还没完，
//! 由 `hold_exit` 拦下这次退出，在等待线程里等它结束再退出。
//!
//! 锁：`state(app)` 的结果一律先 `let` 绑定再用。edition 2024 里 `if let` 判别式的临时锁活到分支结束，
//! 分支里再加锁（如 `wrap_up` 的 `done()`）会在同一线程上死锁。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use tauri::webview::PageLoadEvent;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_notification::NotificationExt;
use xpaste_platform::Recorder;
use xpaste_platform::record::{MicAccess, RecordOptions, RecordOutcome};

use crate::error::AppError;
use crate::events;
use crate::panel::on_main;
use crate::shot::{self, LRect};
use crate::state::AppState;
use crate::tray;

/// 录屏边框窗口，只建一次、永不销毁（同截图遮罩）
const FRAME: &str = "rec-frame";
/// 边框淡出时长（同 `src/rec-frame.css`），淡出后再隐藏窗口
const FADE: Duration = Duration::from_millis(200);
/// 退出时最多等另一个线程上进行中的开始或停止这么久，超时照常退出
const EXIT_WAIT: Duration = Duration::from_secs(10);

/// 录制阶段。开始录制期间（`begin` 之后、录屏器到手之前）也算 `Recording`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Recording,
    Stopping,
}

/// 交给收尾的东西：录屏器、输出路径、录完是否复制。
pub type Taken<R> = (R, PathBuf, bool);

/// 录制状态。`R` 在应用里是 `Recorder`，测试用 `()`。
pub struct RecState<R = Recorder> {
    phase: Phase,
    started: Option<Instant>,
    recorder: Option<R>,
    path: PathBuf,
    copy: bool,
    /// 边框位置（该屏内逻辑像素），边框页加载好后用 `rec_frame` 拉取
    pub frame: Option<LRect>,
}

impl<R> Default for RecState<R> {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            started: None,
            recorder: None,
            path: PathBuf::new(),
            copy: false,
            frame: None,
        }
    }
}

impl<R> RecState<R> {
    /// 开始一次录制；已在录或正在停止时返回 false（M24 规则 1）。
    pub fn begin(&mut self) -> bool {
        if self.phase != Phase::Idle {
            return false;
        }
        self.phase = Phase::Recording;
        true
    }

    /// 录屏器到手。开始期间已请求停止时原样交回，由调用方立即停止。
    pub fn attach(&mut self, r: R, path: PathBuf, copy: bool, now: Instant) -> Option<Taken<R>> {
        if self.phase != Phase::Recording {
            return Some((r, path, copy));
        }
        self.recorder = Some(r);
        self.path = path;
        self.copy = copy;
        self.started = Some(now);
        None
    }

    /// 转入停止并交出录屏器；只有第一次有值。录屏器还没到手时只记下停止请求（`attach` 会交回）。
    pub fn take_for_stop(&mut self) -> Option<Taken<R>> {
        if self.phase != Phase::Recording {
            return None;
        }
        self.phase = Phase::Stopping;
        self.started = None;
        let r = self.recorder.take()?;
        Some((r, std::mem::take(&mut self.path), self.copy))
    }

    /// 收尾完成（或开始失败），回到空闲。
    pub fn done(&mut self) {
        *self = Self::default();
    }

    pub fn is_recording(&self) -> bool {
        self.phase == Phase::Recording
    }

    /// 开始、录制、停止写文件期间都算忙。
    pub fn is_busy(&self) -> bool {
        self.phase != Phase::Idle
    }

    /// 本次录制开始的时刻；计时线程据此认出自己是否已过期。
    pub fn started(&self) -> Option<Instant> {
        self.started
    }

    pub fn recorder(&self) -> Option<&R> {
        self.recorder.as_ref()
    }
}

fn state(app: &AppHandle) -> MutexGuard<'_, RecState> {
    app.state::<AppState>()
        .inner()
        .rec
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

pub fn is_recording(app: &AppHandle) -> bool {
    state(app).is_recording()
}

pub fn is_busy(app: &AppHandle) -> bool {
    state(app).is_busy()
}

/// 当前边框位置；不在录制时为 `None`。
pub fn frame_rect(app: &AppHandle) -> Option<LRect> {
    state(app).frame
}

pub fn notify(app: &AppHandle, body: &str) {
    if let Err(e) = app
        .notification()
        .builder()
        .title("xPaste")
        .body(body)
        .show()
    {
        log::warn!("发送录屏通知失败：{e}");
    }
}

/// `rec_start`：遮罩倒计时结束后调用，阻塞线程里执行。只接受当前这一次截图会话发起的调用。
pub fn start(
    app: &AppHandle,
    generation: u64,
    index: usize,
    rect: LRect,
    system_audio: bool,
    microphone: bool,
) -> Result<(), AppError> {
    if cfg!(not(target_os = "macos")) {
        return Err(AppError::Invalid("当前平台不支持录屏".into()));
    }
    let finite = [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|n| n.is_finite());
    if !finite || rect.w <= 0.0 || rect.h <= 0.0 {
        return Err(AppError::Invalid("录屏区域无效".into()));
    }
    if !state(app).begin() {
        return Err(AppError::Invalid("正在录屏".into()));
    }
    match launch(app, generation, index, rect, system_audio, microphone) {
        Ok(None) => Ok(()),
        // 开始期间已按了停止：录屏器到手就停
        Ok(Some(taken)) => {
            wrap_up(app, taken, false);
            Ok(())
        }
        Err(e) => {
            state(app).done();
            Err(e)
        }
    }
}

/// `start` 的主体；返回 `Some` 表示开始期间已请求停止，录屏器交回给调用方停止。
fn launch(
    app: &AppHandle,
    generation: u64,
    index: usize,
    rect: LRect,
    system_audio: bool,
    microphone: bool,
) -> Result<Option<Taken<Recorder>>, AppError> {
    // 结束会话会清掉屏幕信息：先在同一次加锁里核对代数并取出
    let (display, scale, bounds) = {
        let s = shot::session(app);
        match (s.screen_target(index), s.screen_bounds(index)) {
            (Some((id, scale)), Some(b)) if s.is_current(generation) => (id, scale, b),
            _ => return Err(AppError::Invalid("截图已结束".into())),
        }
    };
    let st = app.state::<AppState>();
    let cur = st.settings().clone();
    // 遮罩上的两个声音开关就是设置里那两项（对设计的裁定 3）；不重新注册快捷键
    if cur.rec_system_audio != system_audio || cur.rec_microphone != microphone {
        let mut s = cur.clone();
        s.rec_system_audio = system_audio;
        s.rec_microphone = microphone;
        match crate::commands::settings::save_settings_inner(&st, s.clone()) {
            Ok(()) => events::emit_to(app, "panel", events::SETTINGS_CHANGED, s),
            Err(e) => log::warn!("录屏声音开关写回设置失败：{e}"),
        }
    }
    shot::end_if_current(app, generation);
    let microphone = microphone && microphone_allowed(app);
    // 开始期间已按停止或退出：还没开始录，直接放弃（权限框可能等了很久）
    let cancelled = || AppError::Invalid("录屏已停止".into());
    if !is_recording(app) {
        return Err(cancelled());
    }

    let dir = resolve_rec_dir(&cur.rec_dir, app.path().video_dir().ok(), &st.data_dir);
    // 遮罩已撤下，此后的失败都要通知，不能静默（M24 规则 9）；io::Error 的描述不带路径
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("rec 创建录屏文件夹失败：{e}");
        notify(app, "录屏失败");
        return Err(e.into());
    }
    let stamp = xpaste_core::capture::local_stamp(crate::clock::now_ms());
    // 写入器直接写最终路径；失败或太短时平台层删掉
    let path = dir.join(shot::unique_file_name("录屏", &stamp, "mp4", |n| {
        dir.join(n).symlink_metadata().is_ok()
    }));

    state(app).frame = Some(rect);
    let exclude = show_frame(app, bounds);
    events::emit_to(app, FRAME, events::REC_FRAME, Some(rect));
    if !is_recording(app) {
        frame_out(app);
        hide_frame_after_fade(app, Instant::now());
        return Err(cancelled());
    }
    let opts = RecordOptions {
        display,
        rect: (rect.x, rect.y, rect.w, rect.h),
        scale,
        system_audio,
        microphone,
        exclude_window: exclude,
        path: path.clone(),
    };
    let recorder = match xpaste_platform::start_recording(opts) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("rec 开始失败：{e}");
            frame_out(app);
            hide_frame_after_fade(app, Instant::now());
            notify(app, "录屏失败");
            return Err(AppError::Invalid("录屏失败".into()));
        }
    };
    // 录屏器到手立即登记，再碰要切主线程的菜单栏：此后退出或停止都能拿到它去写完文件
    let t0 = Instant::now();
    let handed_back = state(app).attach(recorder, path, cur.rec_copy, t0);
    if let Some(taken) = handed_back {
        return Ok(Some(taken));
    }
    tray::set_recording(app, Some(Duration::ZERO));
    tray::show_stop_item(app, true);
    // 设菜单栏的同时被停止了：收尾可能先清了，这里再清一次，不留下过期的计时与菜单项
    let current = state(app).started() == Some(t0);
    if !current {
        tray::set_recording(app, None);
        tray::show_stop_item(app, false);
        return Ok(None);
    }
    log::info!(
        "rec 开始 屏 {index} {}x{} 系统声音={system_audio} 麦克风={microphone}",
        rect.w,
        rect.h
    );
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || tick(&app, t0));
    Ok(None)
}

/// 麦克风权限：未决定时弹系统框（阻塞到用户选择）；没有权限时本次不录麦克风并提示。
fn microphone_allowed(app: &AppHandle) -> bool {
    let ok = match xpaste_platform::microphone_access() {
        MicAccess::Granted => true,
        MicAccess::Undetermined => xpaste_platform::request_microphone(),
        MicAccess::Denied => false,
    };
    if !ok {
        log::info!("rec 麦克风权限未开启，本次不录麦克风");
        notify(app, "麦克风权限未开启，本次录屏没有麦克风声音");
    }
    ok
}

/// 计时线程：每到整秒更新菜单栏标题；录屏器报告中断时按中断停止。本次录制结束（`started` 变了）就退出。
fn tick(app: &AppHandle, t0: Instant) {
    let current = |app: &AppHandle| {
        let started = state(app).started();
        started == Some(t0)
    };
    loop {
        // 睡眠时不持锁
        let next = Duration::from_secs(t0.elapsed().as_secs() + 1);
        std::thread::sleep(next.saturating_sub(t0.elapsed()));
        let failed = {
            let s = state(app);
            if s.started() != Some(t0) {
                return;
            }
            s.recorder().is_some_and(Recorder::failed)
        };
        if failed {
            log::warn!("rec 录制被系统中断");
            return stop_with(app, true);
        }
        tray::set_recording(app, Some(t0.elapsed()));
        // 设标题的同时被停止了：收尾可能先清了标题，这里再清一次，不留下过期的计时
        if !current(app) {
            tray::set_recording(app, None);
            return;
        }
    }
}

/// 停止录制（托盘菜单项、快捷键）。不阻塞：写文件与收尾在后台线程。重复调用只停一次。
pub fn stop(app: &AppHandle) {
    stop_with(app, false);
}

fn stop_with(app: &AppHandle, interrupted: bool) {
    let taken = state(app).take_for_stop();
    let Some(taken) = taken else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || wrap_up(&app, taken, interrupted));
}

/// 退出前调用（托盘“退出”）：录制中在当前线程停止并写完文件。不等别的线程（见模块注释与 `hold_exit`）。
pub fn stop_blocking(app: &AppHandle) {
    // 先绑定：判别式里的临时锁会活到分支结束，wrap_up 再加锁就死锁
    let taken = state(app).take_for_stop();
    if let Some(taken) = taken {
        wrap_up(app, taken, false);
    }
}

/// 已拦下过一次退出：等待线程超时后的 `exit` 不再拦
static EXIT_DEFERRED: AtomicBool = AtomicBool::new(false);

/// 忙（开始或停止进行中）且还没拦过时拦下这次退出。
fn defer_exit(busy: bool, deferred: &AtomicBool) -> bool {
    busy && !deferred.swap(true, Ordering::SeqCst)
}

/// `RunEvent::ExitRequested`（主线程）：先 `stop_blocking`；另一个线程上的开始或停止还没完时返回 true，
/// 调用方 `prevent_exit`，等待线程等它结束（最多 `EXIT_WAIT`）后再 `exit`。主线程不阻塞，开始线程的主线程调用照常执行。
pub fn hold_exit(app: &AppHandle) -> bool {
    stop_blocking(app);
    if !defer_exit(is_busy(app), &EXIT_DEFERRED) {
        return false;
    }
    let waiter = app.clone();
    let spawned = std::thread::Builder::new()
        .name("rec-exit".into())
        .spawn(move || {
            let t = Instant::now();
            // ponytail: 轮询等待，够用；要更及时再换 Condvar
            while is_busy(&waiter) && t.elapsed() < EXIT_WAIT {
                std::thread::sleep(Duration::from_millis(50));
            }
            if is_busy(&waiter) {
                log::warn!("rec 退出时等待超时，照常退出");
            }
            waiter.exit(0);
        });
    match spawned {
        Ok(_) => {
            log::info!("rec 退出前等待录屏开始或停止完成");
            true
        }
        Err(e) => {
            log::warn!("rec 启动退出等待线程失败：{e}");
            false
        }
    }
}

/// 停止并收尾：写完文件、撤边框、清菜单栏、按设置写剪切板、发通知。
fn wrap_up(app: &AppHandle, (recorder, path, copy): Taken<Recorder>, interrupted: bool) {
    let result = recorder.stop();
    // 文件已写完就回到空闲：退出时的等待不依赖下面那些要切到主线程的步骤
    state(app).done();
    frame_out(app);
    let faded = Instant::now();
    tray::set_recording(app, None);
    tray::show_stop_item(app, false);
    let notice = match result {
        Ok(outcome @ RecordOutcome::Saved { duration_ms, bytes }) => {
            log::info!("rec 结束 {duration_ms}ms {bytes} 字节");
            let copied = copy && copy_file(&path);
            outcome_notice(&outcome, copied, interrupted)
        }
        Ok(RecordOutcome::Empty) => {
            log::info!("rec 太短未保存");
            outcome_notice(&RecordOutcome::Empty, false, interrupted)
        }
        Err(e) => {
            log::warn!("rec 停止失败：{e}");
            "录屏失败".into()
        }
    };
    notify(app, &notice);
    hide_frame_after_fade(app, faded);
}

/// 文件引用写剪切板，不标记为自身写入：监听器照常把它作为视频记录入库（M01 规则 5 的例外，同截图复制）。
fn copy_file(path: &Path) -> bool {
    let flavors = xpaste_platform::file_flavors(path);
    if flavors.is_empty() {
        log::warn!("录屏写剪切板失败：生成文件引用失败");
        return false;
    }
    match xpaste_platform::write(&flavors) {
        Ok(_) => true,
        Err(e) => {
            log::warn!("录屏写剪切板失败：{e}");
            false
        }
    }
}

/// 摆好并显示边框窗口（铺满该屏、鼠标穿透、不抢焦点），返回它的窗口编号供录制排除。
fn show_frame(app: &AppHandle, (origin, size): ((f64, f64), (f64, f64))) -> Option<u32> {
    let w = match app.get_webview_window(FRAME) {
        Some(w) => w,
        None => match build_frame(app) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("创建录屏边框失败：{e}");
                return None;
            }
        },
    };
    let (tx, rx) = mpsc::channel();
    on_main(app, move |app| {
        let shown = (|| -> tauri::Result<Option<u32>> {
            shot::place(&w, origin, size)?;
            w.set_ignore_cursor_events(true)?;
            #[cfg(target_os = "macos")]
            {
                use tauri_nspanel::ManagerExt;
                match app.get_webview_panel(FRAME) {
                    Ok(p) => p.order_front_regardless(),
                    Err(_) => w.show()?,
                }
                // 上屏之后才有窗口编号
                Ok(xpaste_platform::window_number(w.ns_window()?))
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = app;
                w.show()?;
                Ok(None)
            }
        })();
        let _ = tx.send(shown.unwrap_or_else(|e| {
            log::warn!("显示录屏边框失败：{e}");
            None
        }));
    });
    // 主线程正忙着退出（stop_blocking 在等）时不能一直等它
    let n = rx.recv_timeout(Duration::from_secs(2)).ok().flatten();
    if n.is_none() {
        log::warn!("rec 取不到边框窗口编号，边框会被录进去");
    }
    n
}

fn build_frame(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    let (tx, rx) = mpsc::channel();
    let w = WebviewWindowBuilder::new(app, FRAME, WebviewUrl::App("rec-frame.html".into()))
        .title("xpaste-rec-frame")
        .theme(crate::panel::current_theme(app))
        .visible(false)
        .focused(false)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .always_on_top(true)
        .on_page_load(move |_, p| {
            if p.event() == PageLoadEvent::Finished {
                let _ = tx.send(());
            }
        })
        .build()?;
    #[cfg(target_os = "macos")]
    {
        let w = w.clone();
        on_main(app, move |_| {
            if let Err(e) = shot::to_overlay_panel(&w) {
                log::warn!("录屏边框转换 NSPanel 失败：{e}");
            }
        });
    }
    // 页面加载完再推送位置；推送仍可能早于页面注册好监听，页面注册后会再拉一次（`rec_frame`）
    let _ = rx.recv_timeout(Duration::from_secs(5));
    Ok(w)
}

/// 边框淡出。
fn frame_out(app: &AppHandle) {
    events::emit_to(app, FRAME, events::REC_FRAME, None::<LRect>);
}

/// 淡出播完后隐藏边框窗口。
fn hide_frame_after_fade(app: &AppHandle, since: Instant) {
    std::thread::sleep(FADE.saturating_sub(since.elapsed()));
    on_main(app, |app| {
        if let Some(w) = app.get_webview_window(FRAME)
            && let Err(e) = w.hide()
        {
            log::warn!("隐藏录屏边框失败：{e}");
        }
    });
}

/// `mm:ss`，一小时及以上 `h:mm:ss`。
fn clock(d: Duration) -> String {
    let s = d.as_secs();
    match s / 3600 {
        0 => format!("{:02}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

/// 菜单栏标题：`● mm:ss`，一小时及以上 `● h:mm:ss`。
pub fn tray_title(elapsed: Duration) -> String {
    format!("● {}", clock(elapsed))
}

/// 录完的系统通知正文。
pub fn outcome_notice(outcome: &RecordOutcome, copied: bool, interrupted: bool) -> String {
    let RecordOutcome::Saved { duration_ms, .. } = *outcome else {
        return "录屏太短，没有保存".into();
    };
    let t = clock(Duration::from_millis(duration_ms));
    if interrupted {
        format!("录屏中断，已保存录到的部分 · {t}")
    } else if copied {
        format!("录屏已保存并复制 · {t}")
    } else {
        format!("录屏已保存 · {t}")
    }
}

/// 设置里录屏文件夹为空时的默认：`<影片>/xPaste`，取不到影片目录时 `<数据目录>/recordings`。
pub fn default_rec_dir(movies: Option<PathBuf>, data_dir: &Path) -> PathBuf {
    movies.map_or_else(|| data_dir.join("recordings"), |m| m.join("xPaste"))
}

/// 设置里的录屏文件夹：空或不是绝对路径时用默认文件夹（同 `shot::resolve_shot_dir`）。
pub fn resolve_rec_dir(setting: &str, movies: Option<PathBuf>, data_dir: &Path) -> PathBuf {
    let p = Path::new(setting);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if !setting.is_empty() {
        log::warn!("录屏文件夹不是绝对路径，改用默认文件夹");
    }
    default_rec_dir(movies, data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shot::unique_file_name;

    #[test]
    fn tray_title_mm_ss_then_h_mm_ss() {
        assert_eq!(tray_title(Duration::ZERO), "● 00:00");
        assert_eq!(tray_title(Duration::from_secs(65)), "● 01:05");
        assert_eq!(tray_title(Duration::from_millis(3_725_900)), "● 1:02:05");
    }

    #[test]
    fn outcome_notice_texts() {
        let saved = |ms| RecordOutcome::Saved {
            duration_ms: ms,
            bytes: 1,
        };
        assert_eq!(
            outcome_notice(&saved(12_400), true, false),
            "录屏已保存并复制 · 00:12"
        );
        assert_eq!(
            outcome_notice(&saved(12_400), false, false),
            "录屏已保存 · 00:12"
        );
        assert_eq!(
            outcome_notice(&RecordOutcome::Empty, true, false),
            "录屏太短，没有保存"
        );
        assert_eq!(
            outcome_notice(&RecordOutcome::Empty, false, true),
            "录屏太短，没有保存"
        );
        assert_eq!(
            outcome_notice(&saved(65_000), true, true),
            "录屏中断，已保存录到的部分 · 01:05"
        );
    }

    #[test]
    fn rec_dir_defaults_and_falls_back() {
        let d = Path::new("/d");
        assert_eq!(
            default_rec_dir(Some("/m".into()), d),
            PathBuf::from("/m/xPaste")
        );
        assert_eq!(default_rec_dir(None, d), PathBuf::from("/d/recordings"));
        // Windows 上 `/mine` 不是绝对路径
        let mine = if cfg!(windows) { r"C:\mine" } else { "/mine" };
        assert_eq!(resolve_rec_dir(mine, None, d), PathBuf::from(mine));
        assert_eq!(resolve_rec_dir("", None, d), PathBuf::from("/d/recordings"));
        assert_eq!(
            resolve_rec_dir("recs", Some("/m".into()), d),
            PathBuf::from("/m/xPaste")
        );
    }

    #[test]
    fn rec_file_name_appends_counter() {
        let stamp = "2026-10-28 15.04";
        assert_eq!(
            unique_file_name("录屏", stamp, "mp4", |_| false),
            "录屏 2026-10-28 15.04.mp4"
        );
        assert_eq!(
            unique_file_name("录屏", stamp, "mp4", |n| n == "录屏 2026-10-28 15.04.mp4"),
            "录屏 2026-10-28 15.04 2.mp4"
        );
    }

    #[test]
    fn stop_is_idempotent() {
        let mut s = RecState::<()>::default();
        assert!(!s.is_busy() && !s.is_recording());
        assert!(s.begin());
        assert!(!s.begin(), "录制中不能再开始");
        assert!(
            s.attach((), "/a.mp4".into(), true, Instant::now())
                .is_none()
        );
        assert!(s.is_recording());
        let (_, path, copy) = s.take_for_stop().expect("第一次停止拿到录屏器");
        assert_eq!((path, copy), (PathBuf::from("/a.mp4"), true));
        assert!(s.take_for_stop().is_none(), "不能重复停止");
        assert!(!s.is_recording() && s.is_busy());
        assert!(!s.begin(), "停止期间不能开始");
        s.done();
        assert!(!s.is_busy());
        assert!(s.begin(), "收尾后可以再录");
    }

    #[test]
    fn exit_is_deferred_once_and_only_while_busy() {
        use std::sync::atomic::AtomicBool;
        let deferred = AtomicBool::new(false);
        assert!(!defer_exit(false, &deferred), "空闲时直接退出");
        assert!(
            defer_exit(true, &deferred),
            "开始或停止进行中：拦下第一次退出"
        );
        assert!(!defer_exit(true, &deferred), "等待线程超时后的退出不再拦");
    }

    #[test]
    fn stop_during_start_hands_recorder_back() {
        let mut s = RecState::<()>::default();
        assert!(s.begin());
        // 录屏器还没到手时按停止：记下请求，不重复停止；开始的一方据 is_recording 提前放弃
        assert!(s.take_for_stop().is_none());
        assert!(!s.is_recording());
        assert!(s.take_for_stop().is_none());
        assert!(!s.begin());
        // 录屏器到手后交回，由开始的一方立即停止
        let back = s.attach((), "/b.mp4".into(), false, Instant::now());
        assert_eq!(back.map(|t| t.1), Some(PathBuf::from("/b.mp4")));
        s.done();
        assert!(s.begin());
    }
}
