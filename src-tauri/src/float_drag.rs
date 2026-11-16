//! 浮层（传送带、搅拌机）的拖动摆放：页面在把手上按下时调 `start`，之后指针线程每次采样调 `tick`，
//! 窗口跟着光标走、整个窗口不出光标所在屏的工作区；松手时记下窗口左上角离工作区右下角的距离（逻辑像素，存 kv），
//! 下次打开在光标所在屏上照这个距离摆，没记过就用默认位置。
//!
//! 坐标同两个窗口模块的 `FRAME`：`pointer_position` 的坐标空间（macOS 是点，系数 1；Windows 是物理像素，系数为所在屏缩放）。

use std::sync::{Mutex, PoisonError};

use tauri::{AppHandle, Manager};

use crate::panel::{Rect, on_main};
use crate::state::AppState;

/// 窗口左上角和换成逻辑像素要除的系数（`pointer_position` 坐标空间）
pub type Frame = Mutex<((f64, f64), f64)>;

/// 一次拖动：按下时的光标、窗口左上角（`pointer_position` 坐标空间）和窗口逻辑大小；`last` 上次投出去的目标
pub struct Drag {
    cursor: (f64, f64),
    origin: (f64, f64),
    size: (f64, f64),
    last: Option<(f64, f64)>,
}

pub type Dragging = Mutex<Option<Drag>>;

/// 窗口左上角限在工作区里；窗口比工作区还大时贴着左上
pub fn clamp(origin: (f64, f64), size: (f64, f64), work: Rect) -> (f64, f64) {
    (
        origin.0.min(work.x + work.w - size.0).max(work.x),
        origin.1.min(work.y + work.h - size.1).max(work.y),
    )
}

/// 记下的距离换成窗口矩形（逻辑像素，限在工作区里）；没记过用 `default`
pub fn placed(work: Rect, default: Rect, offset: Option<(f64, f64)>) -> Rect {
    let Some((dx, dy)) = offset else {
        return default;
    };
    let size = (default.w, default.h);
    let (x, y) = clamp((work.x + work.w - dx, work.y + work.h - dy), size, work);
    Rect { x, y, ..default }
}

/// 拖的是哪一端
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Left,
    Right,
}

/// 拖一端调宽：`dx` 是光标自按下起的横向位移；返回新的左上角和宽。
/// 居中（没拖过位置）时两端对称伸缩，否则按住的那端动、另一端不动；宽限在 `[min, max]`。
pub fn resized(
    origin: (f64, f64),
    w0: f64,
    dx: f64,
    side: Side,
    centered: bool,
    min: f64,
    max: f64,
) -> ((f64, f64), f64) {
    let grow = if side == Side::Right { dx } else { -dx };
    let w = (w0 + grow * if centered { 2.0 } else { 1.0 })
        .min(max)
        .max(min);
    let x = match (centered, side) {
        (true, _) => origin.0 + (w0 - w) / 2.0,
        (false, Side::Left) => origin.0 + w0 - w,
        (false, Side::Right) => origin.0,
    };
    ((x, origin.1), w)
}

/// 窗口左上角离工作区右下角的距离
fn offset_of(origin: (f64, f64), work: Rect) -> (f64, f64) {
    (work.x + work.w - origin.0, work.y + work.h - origin.1)
}

fn parse(v: &str) -> Option<(f64, f64)> {
    let (a, b) = v.split_once(',')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

fn key(label: &str) -> String {
    format!("{label}_offset")
}

/// 记下的距离；没记过或读不出为 None
pub fn saved(app: &AppHandle, label: &str) -> Option<(f64, f64)> {
    let v = app.state::<AppState>().store().kv_get(&key(label)).ok()??;
    parse(&v)
}

/// 光标所在屏的工作区（逻辑像素）和换算系数。主线程调用。
pub fn cursor_work(app: &AppHandle) -> tauri::Result<Option<(Rect, f64)>> {
    let Some(m) = crate::panel::cursor_monitor(app)? else {
        return Ok(None);
    };
    let (sf, wa) = (m.scale_factor(), m.work_area());
    let work = Rect {
        x: wa.position.x as f64 / sf,
        y: wa.position.y as f64 / sf,
        w: wa.size.width as f64 / sf,
        h: wa.size.height as f64 / sf,
    };
    Ok(Some((
        work,
        if cfg!(target_os = "macos") { 1.0 } else { sf },
    )))
}

/// 页面在把手上按下：从此刻的光标和窗口位置开始拖
pub fn start(dragging: &Dragging, frame: &Frame, size: (f64, f64)) {
    let Some(cursor) = xpaste_platform::pointer_position() else {
        return;
    };
    let (origin, _) = *frame.lock().unwrap_or_else(PoisonError::into_inner);
    *dragging.lock().unwrap_or_else(PoisonError::into_inner) = Some(Drag {
        cursor,
        origin,
        size,
        last: None,
    });
}

/// 指针线程每次采样调用：按着键时把窗口挪到光标带到的地方（投到主线程）；松开时结束、记下位置。返回是否正拖着
pub fn tick(
    app: &AppHandle,
    label: &'static str,
    dragging: &'static Dragging,
    frame: &'static Frame,
    cursor: (f64, f64),
    button: bool,
) -> bool {
    let mut g = dragging.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(d) = g.as_mut() else {
        return false;
    };
    if !button {
        let moved = d.last.is_some();
        *g = None;
        drop(g);
        if moved {
            on_main(app, move |app| save(app, label, frame));
        }
        return false;
    }
    let target = (
        d.origin.0 + cursor.0 - d.cursor.0,
        d.origin.1 + cursor.1 - d.cursor.1,
    );
    if d.last == Some(target) {
        return true;
    }
    d.last = Some(target);
    let size = d.size;
    drop(g);
    on_main(app, move |app| move_to(app, label, frame, target, size));
    true
}

/// 挪到 `target`（`pointer_position` 坐标空间），限在光标所在屏的工作区里。主线程调用。
fn move_to(app: &AppHandle, label: &str, frame: &Frame, target: (f64, f64), size: (f64, f64)) {
    let Some(w) = app.get_webview_window(label) else {
        return;
    };
    let moved = cursor_work(app).and_then(|cw| {
        let Some((work, k)) = cw else { return Ok(()) };
        let ps = Rect {
            x: work.x * k,
            y: work.y * k,
            w: work.w * k,
            h: work.h * k,
        };
        let size = (size.0 * k, size.1 * k);
        let origin = clamp(target, size, ps);
        *frame.lock().unwrap_or_else(PoisonError::into_inner) = (origin, k);
        crate::shot::place(&w, origin, size)
    });
    if let Err(e) = moved {
        log::warn!("{label} 拖动失败：{e}");
    }
}

/// 拖完：记下离光标所在屏工作区右下角的距离。主线程调用。
fn save(app: &AppHandle, label: &str, frame: &Frame) {
    let Ok(Some((work, _))) = cursor_work(app) else {
        return;
    };
    save_in(app, label, frame, work);
}

/// 同 `save`，但按给定的工作区算（调长度时用按下时所在屏的，不跟光标换屏）
pub fn save_in(app: &AppHandle, label: &str, frame: &Frame, work: Rect) {
    let ((x, y), k) = *frame.lock().unwrap_or_else(PoisonError::into_inner);
    let (dx, dy) = offset_of((x / k, y / k), work);
    let v = format!("{dx:.0},{dy:.0}");
    if let Err(e) = app.state::<AppState>().store().kv_set(&key(label), &v) {
        log::warn!("{label} 记下位置失败：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 25.0,
        w: 1440.0,
        h: 835.0,
    };

    #[test]
    fn clamp_keeps_window_inside() {
        assert_eq!(clamp((-50.0, 0.0), (380.0, 600.0), WORK), (0.0, 25.0));
        assert_eq!(
            clamp((2000.0, 900.0), (380.0, 600.0), WORK),
            (1060.0, 260.0)
        );
        assert_eq!(clamp((100.0, 100.0), (380.0, 600.0), WORK), (100.0, 100.0));
    }

    #[test]
    fn placed_round_trips_offset() {
        let d = Rect {
            x: 1044.0,
            y: 244.0,
            w: 380.0,
            h: 600.0,
        };
        assert_eq!(placed(WORK, d, None).x, 1044.0);
        let off = offset_of((300.0, 120.0), WORK);
        let r = placed(WORK, d, Some(off));
        assert_eq!((r.x, r.y, r.w, r.h), (300.0, 120.0, 380.0, 600.0));
        // 换到更小的屏：限在工作区里
        let small = Rect {
            w: 600.0,
            h: 500.0,
            ..WORK
        };
        let r = placed(small, d, Some((2000.0, 50.0)));
        assert_eq!((r.x, r.y), (0.0, 25.0));
    }

    #[test]
    fn resize_centered_and_anchored() {
        // 居中：右边往外拖 50，宽 +100、左边往左 50
        assert_eq!(
            resized((100.0, 0.0), 600.0, 50.0, Side::Right, true, 360.0, 1400.0),
            ((50.0, 0.0), 700.0)
        );
        // 拖过位置：右边往外拖 50，左边不动
        assert_eq!(
            resized((100.0, 0.0), 600.0, 50.0, Side::Right, false, 360.0, 1400.0),
            ((100.0, 0.0), 650.0)
        );
        // 左边往里拖 400：宽到下限 360，右边不动
        assert_eq!(
            resized((100.0, 0.0), 600.0, 400.0, Side::Left, false, 360.0, 1400.0),
            ((340.0, 0.0), 360.0)
        );
    }

    #[test]
    fn resize_clamps_max_and_centered_left() {
        // 居中、左边往外拖 50：宽 +100，左边往左 50
        assert_eq!(
            resized((100.0, 0.0), 600.0, -50.0, Side::Left, true, 360.0, 1400.0),
            ((50.0, 0.0), 700.0)
        );
        // 超过上限：宽到 max；居中时左边只让出 (max-w0)/2
        assert_eq!(
            resized((100.0, 0.0), 600.0, 900.0, Side::Right, true, 360.0, 800.0),
            ((0.0, 0.0), 800.0)
        );
        assert_eq!(
            resized((100.0, 0.0), 600.0, -900.0, Side::Left, false, 360.0, 800.0),
            ((-100.0, 0.0), 800.0)
        );
    }

    #[test]
    fn placed_on_negative_screen() {
        let work = Rect {
            x: -1920.0,
            y: -62.0,
            w: 1920.0,
            h: 1080.0,
        };
        let d = Rect {
            x: -1000.0,
            y: 0.0,
            w: 380.0,
            h: 600.0,
        };
        let r = placed(work, d, Some((396.0, 616.0)));
        assert_eq!((r.x, r.y), (-396.0, 402.0));
        // 距离大到超出：贴工作区左上
        let r = placed(work, d, Some((5000.0, 5000.0)));
        assert_eq!((r.x, r.y), (-1920.0, -62.0));
    }

    #[test]
    fn parse_offset() {
        assert_eq!(parse("396,616"), Some((396.0, 616.0)));
        assert_eq!(parse("x"), None);
    }
}
