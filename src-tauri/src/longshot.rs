//! 长截图（M23 规则 13）：截图选好区域后按 L，遮罩撤下，选区四角显示边框（录屏边框窗口 `rec-frame`）与提示；
//! 用户自己滚动区域里的内容，后台线程从这块区域的画面流（`xpaste_platform::start_area_stream`）取帧、拼接
//! （`xpaste_core::stitch`）。回车或再按截图快捷键完成，
//! 拼好的图按普通写入写剪切板（同截图复制，进历史），系统通知结果；Esc 取消。
//!
//! 选区旁有拼接结果的预览（最下面最新接上的部分），边框页收到进度后用 `long_preview` 拉取，看接缝对不对。
//!
//! 有辅助功能权限（Windows 不需要）时可以自动滚动：设置“长截图自动滚动”开着时按 L 直接开始，关着（默认）时手动滚、
//! 按空格开始本次自动滚动。在选区中心发滚动事件，等画面停下再按实际位移决定下一步
//! （`xpaste_core::autoscroll`），已测出比例后把预期位移交给拼接缩小查找范围。用户自己滚动、按键或点击时让出，
//! 停手 1 秒后从头起步；空格暂停与继续；到底自动完成（同回车）；从没动过时转手动并提示不响应。
//! 完成后边框一淡出就在提示条显示“正在生成长图…”，编码、写剪切板后换成结果。
//!
//! 期间临时把回车、Esc（能自动滚动时还有空格）注册成全局快捷键（键盘已还给原应用，否则收不到），结束时注销。
//! 画面排除边框窗口：macOS 由画面流排除它的窗口编号，Windows 上它带 `WDA_EXCLUDEFROMCAPTURE`（见 `rec::show_frame`）。
//!
//! 开发构建里设了 `XPASTE_LONG_DUMP=<目录>` 时，每帧另存为 `NNNNN.png`（第一帧为 00000），用来重放排查接不上。

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use xpaste_core::autoscroll::{Act, Pacer, PacerCfg};
use xpaste_core::stitch::{Step, Stitch};
use xpaste_platform::{AreaOptions, AreaStream, InputWatch, WinRect};

use crate::error::AppError;
use crate::events;
use crate::rec;
use crate::shot::{self, LRect};
use crate::state::AppState;

/// 等下一帧最多这么久：画面不变时画面流不出新帧，等不到就回头看回车、Esc（按下后 0.2 秒内生效）
const WAIT: Duration = Duration::from_millis(100);
/// 比对时忽略区域右侧这么宽（逻辑像素）：滚动条随滚动移动，整列对不上
const SCROLLBAR: f64 = 24.0;
/// 每帧底部这么高（逻辑像素）先不接，滚上来再接：选中整个窗口时底部有圆角，列表底部常有渐隐
const GUARD: f64 = 40.0;
/// 第一帧最多等这么久画面稳定（见 `settled`）
const SETTLE: Duration = Duration::from_millis(600);
/// 连续这么久都没接上，提示改为“这里接不上”，告诉用户回车可以保留已拼好的部分
const STUCK: Duration = Duration::from_secs(3);
const ENTER: &str = "Enter";
const ESC: &str = "Escape";
const SPACE: &str = "Space";
/// 自动滚动时等下一帧最多这么久：要及时看出画面停下（M23 规则 13）
const AUTO_WAIT: Duration = Duration::from_millis(20);
/// 画面这么久没再变，算这一步停下了
const QUIET: Duration = Duration::from_millis(120);
/// 发出一步后画面一直没变，这么久后才算“没动”：应用处理滚动要一点时间
const NO_MOVE: Duration = Duration::from_millis(300);
/// 发出一步后画面一直在变（自动播放的视频、轮播），最多等这么久也算停下，否则节奏永远等不到结果
const STEP_MAX: Duration = Duration::from_secs(1);
/// 用户这么久内有过输入算接手；停手满这么久后恢复〔建议值，真机按手感调〕
const TAKEOVER: Duration = Duration::from_secs(1);
/// 自己的快捷键（空格、回车、Esc）也会被当成用户输入：按下后这么久内的输入不算接手
const OWN_KEY: Duration = Duration::from_millis(300);

const RUN: u8 = 0;
const DONE: u8 = 1;
const CANCEL: u8 = 2;

struct Run {
    cmd: Arc<AtomicU8>,
    rect: LRect,
    /// 截屏线程拿到第一帧后放进来；预览在命令线程里读，截屏线程拼接时短暂加锁
    st: Arc<Mutex<Option<Stitch>>>,
    keys: Arc<Mutex<Keys>>,
}

/// 快捷键回调交给截屏线程的：空格按了奇数次没处理（切换暂停；手动滚时开始自动滚动），以及最近一次长截图快捷键的时刻
struct Keys {
    space: bool,
    at: Option<Instant>,
}

static RUNNING: Mutex<Option<Run>> = Mutex::new(None);

fn running() -> std::sync::MutexGuard<'static, Option<Run>> {
    RUNNING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 发给边框页（`rec-frame`）的进度：已拼接的高度（物理像素）与这一帧的情况
#[derive(Serialize, Clone)]
pub struct Progress {
    height: usize,
    /// `run` 正常；`lost` 这一帧没对上（滚得太快）；`stuck` 连续 `STUCK` 都没对上（画面一直在变，或这个应用的滚动
    /// 落在小数像素上，见已知限制），自动滚动时为连续 3 次恢复不了；`full` 已到长度上限。
    /// 自动滚动（M23 规则 13）：`auto` 滚动中；`paused` 空格暂停；`pointer` 指针移出了选区（只在滚动跟着指针走的
    /// 平台上）；`manual` 设置里没开自动滚动，手动滚、空格开始自动滚动；`noperm` 没有辅助功能权限，手动滚；`noresponse` 窗口不响应自动滚动，手动滚；`autostuck` 自动滚动
    /// 连续 3 次恢复不了、已暂停（空格重试）。用户接手时显示手动的 `run`、`lost`、`stuck`
    state: &'static str,
}

pub fn active() -> bool {
    running().is_some()
}

/// 边框位置（边框页刚建好时拉取）；不在长截图时为 `None`。
pub fn frame_rect() -> Option<LRect> {
    running().as_ref().map(|r| r.rect)
}

/// `shot_long_start`：阻塞线程里执行。只接受当前这一次截图会话、由第 `index` 块屏的遮罩发起的调用。
pub fn start(app: &AppHandle, generation: u64, index: usize, rect: LRect) -> Result<(), AppError> {
    let finite = [rect.x, rect.y, rect.w, rect.h]
        .iter()
        .all(|n| n.is_finite());
    if !finite || rect.w < 1.0 || rect.h < 1.0 {
        return Err(AppError::Invalid("长截图区域无效".into()));
    }
    // 结束会话会清掉屏幕信息：先在同一次加锁里核对代数并取出
    let (origin, scale, display, bounds) = {
        let s = shot::session(app);
        match (
            s.screen(index),
            s.screen_target(index),
            s.screen_bounds(index),
        ) {
            (Some((o, k)), Some((id, _)), Some(b)) if s.is_current(generation) => (o, k, id, b),
            _ => return Err(AppError::Invalid("截图已结束".into())),
        }
    };
    let cmd = Arc::new(AtomicU8::new(RUN));
    let st = Arc::new(Mutex::new(None));
    // 开始的时刻也记成快捷键时刻：按 L（或点工具栏）本身不算用户接手（macOS 的输入时刻早于长截图开始）
    let keys = Arc::new(Mutex::new(Keys {
        space: false,
        at: Some(Instant::now()),
    }));
    {
        let mut r = running();
        if r.is_some() || rec::is_busy(app) {
            return Err(AppError::Invalid("正在长截图或录屏".into()));
        }
        *r = Some(Run {
            cmd: cmd.clone(),
            rect,
            st: st.clone(),
            keys: keys.clone(),
        });
    }
    // 撤下遮罩、把键盘还给原应用；随后边框上屏（主线程上排在隐藏遮罩之后）
    shot::end_if_current(app, generation);
    let below = rec::show_frame(app, bounds);
    events::emit_to(app, rec::FRAME, events::REC_FRAME, Some(rect));
    // 能自动滚动（有权限）时，设置开着才直接自动滚；关着时手动滚，按空格开始本次自动滚动（M15、M23 规则 13）
    let perm = xpaste_platform::can_auto_scroll();
    let auto = perm && app.state::<AppState>().settings().long_auto_scroll;
    let gs = app.global_shortcut();
    // 空格只在能自动滚动时有用；没权限时不占，用户可能用它翻页
    for k in [ENTER, ESC, SPACE]
        .into_iter()
        .filter(|k| perm || *k != SPACE)
    {
        if let Err(e) = gs.register(k) {
            log::warn!("long 注册 {k} 失败：{e}");
        }
    }
    let (o, size) = shot::pin_bounds(origin, scale, rect, shot::POINTS);
    let opts = AreaOptions {
        display,
        rect: (rect.x, rect.y, rect.w, rect.h),
        scale,
        exclude_window: below,
        native: WinRect {
            x: o.0,
            y: o.1,
            w: size.0,
            h: size.1,
        },
    };
    log::info!(
        "long 开始 屏 {index} {}x{} {}",
        rect.w,
        rect.h,
        match (auto, perm) {
            (true, _) => "自动滚动",
            (false, true) => "手动（空格自动滚动）",
            (false, false) => "手动（没权限）",
        }
    );
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || run(&app, &cmd, &st, &keys, auto, perm, opts));
    Ok(())
}

/// `long_preview`：拼接结果缩到宽 `w`、最多高 `h`（物理像素）的预览，前 8 字节为小端 `u32` 宽、高，其后是 RGBA。
/// 不在长截图或还没截到第一帧时为 `None`。
pub fn preview(w: u32, h: u32) -> Option<Vec<u8>> {
    let st = running().as_ref()?.st.clone();
    let g = st.lock().unwrap_or_else(PoisonError::into_inner);
    // 预览最大 400 × 4000：边框页按屏幕尺寸要，这里只防异常参数
    let (pw, ph, px) = g
        .as_ref()?
        .preview(w.min(400) as usize, h.min(4000) as usize);
    let mut bytes = Vec::with_capacity(8 + px.len());
    bytes.extend_from_slice(&(pw as u32).to_le_bytes());
    bytes.extend_from_slice(&(ph as u32).to_le_bytes());
    bytes.extend_from_slice(&px);
    Some(bytes)
}

/// 完成（`done`：回车、截图快捷键）或取消（Esc）。拼图与收尾在截屏线程里做。
pub fn stop(done: bool) {
    if let Some(r) = running().as_ref() {
        r.cmd
            .store(if done { DONE } else { CANCEL }, Ordering::SeqCst);
        log::info!("long {}", if done { "完成" } else { "取消" });
    }
}

/// 全局快捷键回调里先问这里：长截图期间的回车、Esc、空格归长截图。返回是否已处理。
pub fn key(pressed: &Shortcut) -> bool {
    let Some(keys) = running().as_ref().map(|r| r.keys.clone()) else {
        return false;
    };
    let is = |k: &str| k.parse::<Shortcut>().is_ok_and(|s| s == *pressed);
    let mut k = keys.lock().unwrap_or_else(PoisonError::into_inner);
    if is(SPACE) {
        k.space = !k.space;
    } else if is(ENTER) || is(ESC) {
        stop(is(ENTER));
    } else {
        return false;
    }
    // 这次按键也会被 `InputWatch` 看到，记下时刻，不算用户接手（M23 规则 13）
    k.at = Some(Instant::now());
    true
}

/// 自动滚动发出、还没报给节奏的一步（M23 规则 13）
struct Flight {
    /// 发出时长图上的位置；当时画面没定位到（没接上）时为 None
    from: Option<usize>,
    /// 往回滚的恢复步
    back: bool,
    /// 交给拼接的预期位置（比例已测出、往下滚、发出时定位到时才有）
    near: Option<usize>,
    sent: Instant,
}

/// 自动滚动的状态（M23 规则 13）
struct Auto {
    pacer: Pacer,
    watch: InputWatch,
    /// 选区的原生坐标：滚动事件发在中心，指针在不在里面
    rect: WinRect,
    flight: Option<Flight>,
    /// 暂停的原因：`paused` 空格，`autostuck` 一直接不上；空格继续
    hold: Option<&'static str>,
    /// 给边框页的状态：`auto`、`paused`、`autostuck`、`pointer`
    state: &'static str,
    /// 用户正在接手：显示拼接自己的 `lost`、`stuck` 提示
    user: bool,
}

/// 截屏线程：画面流的第一帧作为开头，之后来一帧拼一帧，直到完成或取消（M23 规则 13）。
/// `auto`：一开始就自动滚动；`perm`：能自动滚动（有权限），手动滚时按空格开始自动滚动。
fn run(
    app: &AppHandle,
    cmd: &AtomicU8,
    shared: &Mutex<Option<Stitch>>,
    keys: &Mutex<Keys>,
    auto: bool,
    perm: bool,
    opts: AreaOptions,
) {
    let scale = opts.scale;
    let rect = opts.native;
    let stream = match xpaste_platform::start_area_stream(opts) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("long 画面流失败：{e}");
            end(app);
            return rec::frame_result(app, "长截图失败", false);
        }
    };
    let (w, h, first) = match settled(first_grab(&stream)) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("long 截屏失败：{e}");
            end(app);
            stream.stop();
            return rec::frame_result(app, "长截图失败", false);
        }
    };
    let dump = dump_frames();
    let (w, h) = (w as usize, h as usize);
    let px = |v: f64| (v * scale).round() as usize;
    let stitch = || shared.lock().unwrap_or_else(PoisonError::into_inner);
    *stitch() = Some(Stitch::new(w, h, &first, px(SCROLLBAR), px(GUARD)));
    if let Some((tx, _)) = &dump {
        let _ = tx.send((w as u32, h as u32, first));
    }
    let height = || stitch().as_ref().map_or(0, Stitch::height);
    let progress = |height, state| {
        events::emit_to(
            app,
            rec::FRAME,
            events::LONG_SHOT,
            Progress { height, state },
        );
    };
    let (mut sent, mut state) = (None, "run");
    // 从哪一刻起一直没接上
    let mut lost_since: Option<Instant> = None;
    // 排查用：拼了几帧、几帧没接上、拼接累计耗时
    let (mut frames, mut lost, mut busy) = (0u32, 0u32, Duration::ZERO);
    let new_auto = || Auto {
        pacer: Pacer::new(PacerCfg {
            frame_h: h,
            px_per_unit: xpaste_platform::PX_PER_SCROLL_UNIT * scale,
        }),
        watch: xpaste_platform::watch_user_input(),
        rect,
        flight: None,
        hold: None,
        state: "auto",
        user: false,
    };
    let mut auto = auto.then(new_auto);
    // 没在自动滚动时，拼接状态 `run` 显示成什么：`run`、`manual`（设置里没开，空格开始自动滚动）、`noperm`、
    // `noresponse`
    let mut manual = match (auto.is_some(), perm) {
        (true, _) => "run",
        (false, true) => "manual",
        (false, false) => "noperm",
    };
    // 日志用：自动滚动发了几步、学到的比例、怎么结束的
    let (mut steps, mut ratio) = (0u32, None::<f64>);
    let mut ended = match manual {
        "manual" => "没开自动",
        "noperm" => "没权限",
        _ => "手动",
    };
    // 画面最近一次变化（接上、没接上、换了位置）的时刻
    let mut changed = Instant::now();
    loop {
        let c = cmd.load(Ordering::SeqCst);
        if c == CANCEL {
            end(app);
            stream.stop();
            join_dump(dump);
            return rec::hide_frame_after_fade(app, Instant::now());
        }
        // 按回车时再多等一会儿取最后一帧：最后滚进来的内容不能漏
        let wait = if c == DONE {
            Duration::from_millis(150)
        } else if auto.as_ref().is_some_and(|a| a.hold.is_none()) {
            AUTO_WAIT
        } else {
            WAIT
        };
        match stream.next(wait) {
            // 尺寸变了（如分辨率切换）：这一帧跳过
            Some(f) if f.w as usize == w && f.h as usize == h => {
                frames += 1;
                if let Some((tx, _)) = &dump {
                    let _ = tx.send((f.w, f.h, f.rgba.clone()));
                }
                // 处理耗时不算存图的拷贝
                let t = Instant::now();
                let near = auto
                    .as_ref()
                    .and_then(|a| a.flight.as_ref())
                    .and_then(|f| f.near);
                let (step, moved) = stitch().as_mut().map_or((Step::Same, false), |st| {
                    let (from, was) = (st.position(), st.located());
                    let step = st.push_near(f.rgba, near);
                    (step, st.position() != from || st.located() != was)
                });
                // 画面停下的判断看这里：接上了新内容、没接上、定位到了别处或重新定位到都算变化
                if step != Step::Same || moved {
                    changed = t;
                }
                state = match step {
                    // 滚回接上的位置：画面与上次接上的一样，“没接上”的提示要撤掉
                    Step::Same | Step::Added(_) => "run",
                    Step::Lost => {
                        lost += 1;
                        let since = *lost_since.get_or_insert(t);
                        if since.elapsed() >= STUCK {
                            "stuck"
                        } else {
                            "lost"
                        }
                    }
                    Step::Full => "full",
                };
                if state != "lost" && state != "stuck" {
                    lost_since = None;
                }
                busy += t.elapsed();
            }
            Some(_) => {}
            // 画面流中途断了：按完成收尾，已拼好的部分照样复制
            None if stream.failed() => {
                log::warn!("long 画面流中断，保留已拼好的部分");
                break;
            }
            // 画面没变，画面流不出新帧：不算没接上，只看没接上是否已满 `STUCK`
            None => state = idle_state(state, lost_since, Instant::now()),
        }
        if c == DONE {
            break;
        }
        // 到上限：不再自动滚（手动滚时也不能再开始），等用户回车
        if state == "full" && (auto.take().is_some() || manual == "manual") {
            (manual, ended) = ("run", "到上限");
            release_space(app);
        }
        // 手动滚时按了空格：开始本次自动滚动，之后同设置开着时一样（M23 规则 13）。空格这一下已记成快捷键时刻，
        // 不算接手；标记在这里取走，`drive` 不会再把它当成暂停
        if manual == "manual"
            && std::mem::take(&mut keys.lock().unwrap_or_else(PoisonError::into_inner).space)
        {
            log::info!("long 空格开始自动滚动");
            auto = Some(new_auto());
            (manual, ended) = ("run", "手动");
        }
        let mut to_manual = false;
        if let Some(a) = auto.as_mut() {
            // 当前画面在长图上的位置；最近一帧没接上时为 None
            let pos = stitch()
                .as_ref()
                .and_then(|st| st.located().then(|| st.position()));
            let next = drive(a, keys, pos, changed);
            if next.ratio.is_some() {
                ratio = next.ratio;
            }
            match next.act {
                // 等帧期间按了回车或 Esc：不再往用户的应用里多滚一步，下一轮收尾
                Some(Act::Scroll { .. }) if cmd.load(Ordering::SeqCst) != RUN => {}
                Some(Act::Scroll { units, expect_px }) => {
                    let (x, y) = (a.rect.x + a.rect.w / 2.0, a.rect.y + a.rect.h / 2.0);
                    if let Err(e) = xpaste_platform::scroll_at(x, y, units) {
                        log::warn!("long 发滚动失败，转手动：{e}");
                        to_manual = true;
                    } else {
                        steps += 1;
                        a.flight = Some(Flight {
                            from: pos,
                            back: units < 0,
                            // 比例测出来之后、往下滚、当前位置可信时才给预期；往回滚没有预期
                            near: pos
                                .filter(|_| a.pacer.measured() && units > 0)
                                .map(|p| p + expect_px),
                            sent: Instant::now(),
                        });
                    }
                }
                // 到底：同回车（下一轮再多等一会儿取最后一帧）；Esc 已按下时不改成完成
                Some(Act::Finish) => {
                    ended = "到底";
                    let _ = cmd.compare_exchange(RUN, DONE, Ordering::SeqCst, Ordering::SeqCst);
                }
                Some(Act::NoResponse) => to_manual = true,
                Some(Act::Stuck) => a.hold = Some("autostuck"),
                None => {}
            }
        }
        if to_manual {
            log::info!("long 窗口不响应自动滚动，转手动");
            auto = None;
            (manual, ended) = ("noresponse", "不响应");
            release_space(app);
        }
        let shown = match (&auto, state) {
            (_, "full") => "full",
            // 用户接手：按手动拼接显示（没暂停时不说“自动滚动中”）
            (Some(a), s @ ("lost" | "stuck")) if a.user => s,
            (Some(a), "run") if a.user && a.hold.is_none() => "run",
            (Some(a), _) => a.state,
            (None, "run") => manual,
            (None, s) => s,
        };
        let cur = (height(), shown);
        let now = Instant::now();
        if should_emit(sent, now, cur) {
            sent = Some((now, cur));
            progress(cur.0, cur.1);
        }
    }
    // 先收边框和快捷键，再停画面流（macOS 上停止最多等 10 秒）；Windows 上顺带卸掉输入钩子
    let _ = auto.take();
    let st = stitch().take();
    end(app);
    // 停画面流、编码与写剪切板要一会儿（开发构建 1–4 秒）：边框一淡出就提示在生成，完成后换成结果（M23 规则 13）
    if st.is_some() {
        rec::frame_busy(app, "正在生成长图…");
    }
    let mode = stream.mode();
    stream.stop();
    join_dump(dump);
    let Some(st) = st else {
        return;
    };
    let (w, h, rgba) = st.finish();
    let t = Instant::now();
    let (notice, ok) = match encode(w, h, rgba).and_then(|png| {
        let n = png.len();
        shot::copy_png(png).map(|()| n)
    }) {
        Ok(bytes) => {
            log::info!(
                "long 完成 {w}x{h} {bytes} 字节 编码与写剪切板 {}ms；{mode} {frames} 帧 平均处理 {}ms，没接上 {lost} 帧；\
                 自动滚动 {steps} 步 比例 {} {ended}",
                t.elapsed().as_millis(),
                (busy / frames.max(1)).as_millis(),
                ratio.map_or("-".into(), |r| format!("{r:.2}"))
            );
            // 超过 M01 的大小上限，监听器不入库
            let text = if bytes > xpaste_core::capture::MAX_BITMAP_BYTES {
                format!("长截图已复制，超过 50 MB，没有加入历史 · {w} × {h}")
            } else {
                format!("长截图已复制，已加入历史 · {w} × {h}")
            };
            (text, true)
        }
        Err(e) => {
            log::warn!("long 输出失败：{e:?}");
            ("长截图失败".into(), false)
        }
    };
    rec::frame_result(app, &notice, ok);
}

/// `drive` 的结果：要做的动作，以及报过一步之后学到的比例（日志用）
struct Next {
    act: Option<Act>,
    ratio: Option<f64>,
}

/// 自动滚动的一拍（每取一次帧后调用）：处理空格，看 `auto_gate` 要不要发下一步，要发时把在途一步的位移报给节奏。
/// `pos` 为长图上的当前位置（最近一帧没接上时为 None），`changed` 为画面最近一次变化的时刻。
/// 发事件由调用方做（M23 规则 13）。
fn drive(a: &mut Auto, keys: &Mutex<Keys>, pos: Option<usize>, changed: Instant) -> Next {
    let (space, key_at) = {
        let mut k = keys.lock().unwrap_or_else(PoisonError::into_inner);
        (std::mem::take(&mut k.space), k.at)
    };
    if space {
        // 暂停时丢掉在途的一步；继续时从头起步
        a.hold = if a.hold.is_some() {
            None
        } else {
            Some("paused")
        };
        a.flight = None;
    }
    let r = a.rect;
    let pointer_in = !xpaste_platform::SCROLL_FOLLOWS_POINTER
        || xpaste_platform::pointer_position()
            .is_none_or(|(x, y)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h);
    let now = Instant::now();
    let gate = auto_gate(
        a.hold.is_some(),
        a.watch.age(),
        key_at.map(|t| now.saturating_duration_since(t)),
        pointer_in,
        quiet(now, changed, a.flight.as_ref().map(|f| f.sent)),
    );
    a.user = gate == Gate::User;
    a.state = a.hold.unwrap_or(if gate == Gate::Pointer {
        "pointer"
    } else {
        "auto"
    });
    let mut next = Next {
        act: None,
        ratio: None,
    };
    match gate {
        // 用户接手：在途的一步作废（位移里混着用户滚的），停手后从头起步
        Gate::User => a.flight = None,
        Gate::Wait | Gate::Pointer => {}
        Gate::Go => {
            next.act = Some(match a.flight.take() {
                Some(f) => {
                    let act = a.pacer.settled(report(&f, pos));
                    next.ratio = a.pacer.measured().then(|| a.pacer.px_per_unit());
                    act
                }
                None => a.pacer.start(),
            });
        }
    }
    next
}

/// 在途的一步停下后报给节奏的位移（M23 规则 13）。往下滚：发出与停下时都定位到才有位移，否则报没接上，
/// 节奏往回滚恢复。往回滚的恢复步：停下时定位到就算接上（节奏不看大小），发出时多半正因没接上而没定位到。
fn report(f: &Flight, pos: Option<usize>) -> Option<i64> {
    if f.back {
        pos.map(|y| f.from.map_or(0, |x| y as i64 - x as i64))
    } else {
        f.from.zip(pos).map(|(x, y)| y as i64 - x as i64)
    }
}

/// 要不要发下一步自动滚动（M23 规则 13）
#[derive(Debug, PartialEq, Eq)]
enum Gate {
    Go,
    Wait,
    /// 用户 1 秒内滚过、按过键或点过：让出，丢掉在途的一步
    User,
    /// 滚动跟着指针走而指针不在选区：等指针回来
    Pointer,
}

/// `user_age`：距用户最近一次输入（`InputWatch::age`，None 为没有过）；`key_age`：距最近一次长截图快捷键；
/// `pointer_in`：指针在选区里（滚动不跟指针的平台上恒为真）；`quiet`：画面已停下（见 `quiet`）。
fn auto_gate(
    paused: bool,
    user_age: Option<Duration>,
    key_age: Option<Duration>,
    pointer_in: bool,
    quiet: bool,
) -> Gate {
    // 输入时刻不晚于快捷键时刻 + OWN_KEY 的，是自己的快捷键（系统先看到按键，再分发给快捷键回调）
    let user = user_age.is_some_and(|a| a < TAKEOVER && key_age.is_none_or(|k| a + OWN_KEY < k));
    if user {
        Gate::User
    } else if paused {
        Gate::Wait
    } else if !pointer_in {
        Gate::Pointer
    } else if !quiet {
        Gate::Wait
    } else {
        Gate::Go
    }
}

/// 画面停下了没有：在途的一步（`sent` 发出）之后变过，距最近一次变化满 `QUIET`；之后一直没变，距发出满
/// `NO_MOVE`；一直在变时距发出满 `STEP_MAX` 也算；没有在途的一步时，距最近一次变化满 `QUIET`（M23 规则 13）。
fn quiet(now: Instant, changed: Instant, sent: Option<Instant>) -> bool {
    match sent {
        Some(s) if now.saturating_duration_since(s) >= STEP_MAX => true,
        Some(s) if changed < s => now.saturating_duration_since(s) >= NO_MOVE,
        _ => now.saturating_duration_since(changed) >= QUIET,
    }
}

/// 空闲（画面流没有新帧）时的状态：没接上已满 `STUCK` 时改为 `stuck`（M23 规则 13 按时间算，停住不滚也要提示），
/// 其余不变；空闲不算一次没接上，也不清 `lost_since`。
fn idle_state(state: &'static str, lost_since: Option<Instant>, now: Instant) -> &'static str {
    match lost_since {
        Some(since) if state == "lost" && now.saturating_duration_since(since) >= STUCK => "stuck",
        _ => state,
    }
}

/// 进度什么时候发给边框页：状态变了立即发；高度变了至少隔 100 ms 发一次（画面流一秒最多 60 帧，不必每帧都发）；
/// 什么都没变时每秒补发一次（边框页刚建好时可能错过第一条）。`last` 为上次发的时刻与内容。
fn should_emit(last: Option<(Instant, (usize, &str))>, now: Instant, cur: (usize, &str)) -> bool {
    let Some((at, (height, state))) = last else {
        return true;
    };
    let gap = now.saturating_duration_since(at);
    state != cur.1
        || (height != cur.0 && gap >= Duration::from_millis(100))
        || gap >= Duration::from_secs(1)
}

/// `settled` 用的取帧：第一帧最多等 0.5 秒；之后 `WAIT` 内没有新帧说明画面没变，返回上一帧
/// （画面流只在画面变了时出帧，否则静止的画面永远等不到“连续两帧相同”）。
fn first_grab(stream: &AreaStream) -> impl Fn() -> Result<(u32, u32, Vec<u8>), &'static str> + '_ {
    let last = RefCell::new(None::<(u32, u32, Vec<u8>)>);
    move || {
        let wait = if last.borrow().is_some() {
            WAIT
        } else {
            Duration::from_millis(500)
        };
        match stream.next(wait) {
            Some(f) => {
                let f = (f.w, f.h, f.rgba);
                *last.borrow_mut() = Some(f.clone());
                Ok(f)
            }
            None if stream.failed() => Err("画面流中断"),
            None => last.borrow().clone().ok_or("画面流没有画面"),
        }
    }
}

type Dump = (
    mpsc::Sender<(u32, u32, Vec<u8>)>,
    std::thread::JoinHandle<()>,
);

/// 开发构建、设了 `XPASTE_LONG_DUMP=<目录>` 时，后台线程把每帧存成 `NNNNN.png`；正式构建永远是 `None`。
// 通道不限长：写盘慢于 60 帧/秒时帧在内存里排队；只在开发构建里，可以接受
fn dump_frames() -> Option<Dump> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let dir = std::path::PathBuf::from(std::env::var_os("XPASTE_LONG_DUMP")?);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("long 存帧目录创建失败：{e}");
        return None;
    }
    let (tx, rx) = mpsc::channel::<(u32, u32, Vec<u8>)>();
    let h = std::thread::spawn(move || {
        for (i, (w, h, rgba)) in rx.into_iter().enumerate() {
            let saved = image::RgbaImage::from_raw(w, h, rgba)
                .map(|img| img.save(dir.join(format!("{i:05}.png"))));
            if let Some(Err(e)) = saved {
                log::debug!("long 存第 {i} 帧失败：{e}");
            }
        }
    });
    log::info!("long 逐帧存图已开启");
    Some((tx, h))
}

/// 丢掉发送端，等写盘线程把排队的帧写完
fn join_dump(dump: Option<Dump>) {
    if let Some((tx, h)) = dump {
        drop(tx);
        let _ = h.join();
    }
}

/// 长图的第一帧。遮罩刚撤下，窗口服务器还在把它淡出：这时截到的是遮罩里的冻结画面，看着一样、每个像素的颜色
/// 都差一点，之后每一帧都和它对不上，怎么滚都“没接上”。所以等到连续两帧完全相同（画面稳定）再用；
/// 用户一开始就在滚、一直不稳定时，最多等 `SETTLE`，用最后一帧。
fn settled<E>(grab: impl Fn() -> Result<(u32, u32, Vec<u8>), E>) -> Result<(u32, u32, Vec<u8>), E> {
    let t = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    let mut f = grab()?;
    while t.elapsed() < SETTLE {
        // 取帧本身会等下一帧，不用再睡
        let next = grab()?;
        if next == f {
            break;
        }
        f = next;
    }
    Ok(f)
}

/// 快速压缩档：长图很大，默认档在 release 下也要近 2 秒（2818 × 3872），快速档不到 0.1 秒，文件大 2%–55%
fn encode(w: usize, h: usize, rgba: Vec<u8>) -> Result<Vec<u8>, AppError> {
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    if rgba.len() != w * h * 4 {
        return Err(AppError::Invalid("长截图尺寸无效".into()));
    }
    let mut png = Vec::new();
    PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::Adaptive)
        .write_image(&rgba, w as u32, h as u32, image::ExtendedColorType::Rgba8)
        .map_err(|e| AppError::Invalid(format!("长截图编码失败：{e}")))?;
    Ok(png)
}

/// 不再自动滚动（转手动、到上限）：空格还给应用（可能用它翻页）
fn release_space(app: &AppHandle) {
    if let Err(e) = app.global_shortcut().unregister(SPACE) {
        log::debug!("long 注销 {SPACE} 失败：{e}");
    }
}

/// 撤下边框（淡出，窗口留着显示结果；取消时由调用方隐藏）、注销回车、Esc 与空格、回到空闲。
fn end(app: &AppHandle) {
    rec::frame_out(app);
    let gs = app.global_shortcut();
    for k in [ENTER, ESC, SPACE] {
        if let Err(e) = gs.unregister(k) {
            log::debug!("long 注销 {k} 失败：{e}");
        }
    }
    *running() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn first_frame_waits_until_the_overlay_is_gone() {
        // 前两帧还带着淡出中的遮罩，之后画面稳定
        let frames = [1u8, 2, 3, 3, 3];
        let i = Cell::new(0);
        let grab = || {
            let k = frames[i.get().min(frames.len() - 1)];
            i.set(i.get() + 1);
            Ok::<_, ()>((1, 1, vec![k; 4]))
        };
        assert_eq!(settled(grab), Ok((1, 1, vec![3; 4])));
        // 一直在变：最多等 SETTLE，用最后一帧
        let n = Cell::new(0u8);
        let moving = || {
            n.set(n.get().wrapping_add(1));
            Ok::<_, ()>((1, 1, vec![n.get(); 4]))
        };
        let t = Instant::now();
        assert!(settled(moving).is_ok());
        assert!(t.elapsed() < SETTLE + Duration::from_millis(200));
    }

    #[test]
    fn progress_throttle() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let last = Some((t0, (100, "run")));
        // 还没发过：发
        assert!(should_emit(None, t0, (0, "run")));
        // 状态变了：立即发
        assert!(should_emit(last, ms(1), (100, "lost")));
        // 高度变了：至少隔 100 ms
        assert!(!should_emit(last, ms(50), (200, "run")));
        assert!(should_emit(last, ms(100), (200, "run")));
        // 什么都没变：每 1 秒补发一次
        assert!(!should_emit(last, ms(999), (100, "run")));
        assert!(should_emit(last, ms(1000), (100, "run")));
    }

    #[test]
    fn auto_gate_rules() {
        let s = Duration::from_secs_f64;
        // 只移动指针（没有输入，或输入已超过 1 秒）：照常发
        assert_eq!(auto_gate(false, None, None, true, true), Gate::Go);
        assert_eq!(auto_gate(false, Some(s(1.5)), None, true, true), Gate::Go);
        // 用户 1 秒内滚过、按过键：让出
        assert_eq!(auto_gate(false, Some(s(0.5)), None, true, true), Gate::User);
        // 用户接手优先于其余各条
        assert_eq!(
            auto_gate(true, Some(s(0.5)), None, false, false),
            Gate::User
        );
        // 自己的空格（及回车、Esc）：输入时刻不晚于快捷键时刻 + 300 ms，不算接手
        assert_eq!(
            auto_gate(false, Some(s(0.5)), Some(s(0.5)), true, true),
            Gate::Go
        );
        assert_eq!(
            auto_gate(false, Some(s(0.3)), Some(s(0.5)), true, true),
            Gate::Go
        );
        // 快捷键之后又有输入（晚于快捷键 300 ms 以上）：是用户
        assert_eq!(
            auto_gate(false, Some(s(0.1)), Some(s(0.5)), true, true),
            Gate::User
        );
        // 暂停
        assert_eq!(auto_gate(true, None, None, true, true), Gate::Wait);
        // 指针不在选区（调用方只在滚动跟着指针走的平台上报 false）
        assert_eq!(auto_gate(false, None, None, false, true), Gate::Pointer);
        // 画面还没停下
        assert_eq!(auto_gate(false, None, None, true, false), Gate::Wait);
    }

    #[test]
    fn report_to_pacer() {
        let f = |from, back| Flight {
            from,
            back,
            near: None,
            sent: Instant::now(),
        };
        // 往下滚：接上报位移，没接上报 None
        assert_eq!(report(&f(Some(100), false), Some(400)), Some(300));
        assert_eq!(report(&f(Some(100), false), None), None);
        assert_eq!(report(&f(None, false), Some(400)), None);
        // 往回滚：发出时没定位到，停下时重新定位到也算接上
        assert!(report(&f(None, true), Some(250)).is_some());
        assert_eq!(report(&f(Some(400), true), Some(250)), Some(-150));
        // 往回滚仍没接上
        assert_eq!(report(&f(None, true), None), None);
    }

    #[test]
    fn quiet_waits_for_the_picture_to_stop() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        // 发出后有过变化：距最近一次变化满 QUIET
        assert!(!quiet(ms(200), ms(100), Some(t0)));
        assert!(quiet(ms(100) + QUIET, ms(100), Some(t0)));
        // 发出后一直没变：距发出满 NO_MOVE（应用还没来得及动，不能算没动）
        assert!(!quiet(ms(200), t0, Some(ms(10))));
        assert!(quiet(ms(10) + NO_MOVE, t0, Some(ms(10))));
        // 一直在变（视频、轮播）：距发出满 STEP_MAX 也算停下
        assert!(!quiet(ms(999), ms(990), Some(t0)));
        assert!(quiet(t0 + STEP_MAX, t0 + STEP_MAX, Some(t0)));
        // 没有在途的一步：距最近一次变化满 QUIET
        assert!(!quiet(ms(50), t0, None));
        assert!(quiet(t0 + QUIET, t0, None));
    }

    #[test]
    fn idle_turns_lost_into_stuck_after_stuck_time() {
        let t0 = Instant::now();
        // 没接上后停住不滚，画面流不出帧：满 STUCK 改为 stuck
        assert_eq!(idle_state("lost", Some(t0), t0 + STUCK), "stuck");
        assert_eq!(
            idle_state("lost", Some(t0), t0 + Duration::from_secs(1)),
            "lost"
        );
        // 其余状态空闲时不变
        assert_eq!(idle_state("run", None, t0 + STUCK), "run");
        assert_eq!(idle_state("full", None, t0 + STUCK), "full");
    }
}
