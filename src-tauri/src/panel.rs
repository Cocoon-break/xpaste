use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, WebviewWindow};

use crate::state::AppState;

const LABEL: &str = "panel";

/// 逻辑像素的矩形区域。
#[derive(Clone, Copy, Debug)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 窗口左上角：水平居中、顶边在工作区 1/6 处，并钳制在工作区内。
pub fn place_in_work_area(work: Rect, (w, h): (f64, f64)) -> (f64, f64) {
    // 先 min 后 max（不用 f64::clamp）：窗口比工作区大时贴左上而不是 panic
    let x = (work.x + (work.w - w) / 2.0)
        .min(work.x + work.w - w)
        .max(work.x);
    let y = (work.y + work.h / 6.0).min(work.y + work.h - h).max(work.y);
    (x, y)
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    let w = app.get_webview_window(LABEL);
    if w.is_none() {
        log::warn!("找不到面板窗口");
    }
    w
}

/// 鼠标所在显示器；找不到时用主显示器。
fn cursor_monitor(app: &AppHandle) -> tauri::Result<Option<Monitor>> {
    let c = app.cursor_position()?;
    let hit = app.available_monitors()?.into_iter().find(|m| {
        let (p, s) = (m.position(), m.size());
        c.x >= p.x as f64
            && c.x < p.x as f64 + s.width as f64
            && c.y >= p.y as f64
            && c.y < p.y as f64 + s.height as f64
    });
    match hit {
        Some(m) => Ok(Some(m)),
        None => app.primary_monitor(),
    }
}

fn try_show(app: &AppHandle, w: &WebviewWindow, t0: Option<u64>) -> tauri::Result<()> {
    let size = app
        .state::<AppState>()
        .settings
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .panel_size;
    if let Some(m) = cursor_monitor(app)? {
        let (sf, wa) = (m.scale_factor(), m.work_area());
        let work = Rect {
            x: wa.position.x as f64 / sf,
            y: wa.position.y as f64 / sf,
            w: wa.size.width as f64 / sf,
            h: wa.size.height as f64 / sf,
        };
        let (x, y) = place_in_work_area(work, size);
        w.set_size(LogicalSize::new(size.0, size.1))?;
        w.set_position(LogicalPosition::new(x, y))?;
    }
    w.show()?;
    w.set_focus()?;
    w.emit("panel-shown", serde_json::json!({ "t0": t0 }))
}

pub fn show(app: &AppHandle, t0: Option<u64>) {
    if let Some(w) = window(app)
        && let Err(e) = try_show(app, &w, t0)
    {
        log::warn!("显示面板失败：{e}");
    }
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = window(app)
        && let Err(e) = w.hide()
    {
        log::warn!("隐藏面板失败：{e}");
    }
}

/// 以窗口实际可见性为准切换，不另存状态。
pub fn toggle(app: &AppHandle, t0: Option<u64>) {
    let Some(w) = window(app) else { return };
    match w.is_visible() {
        Ok(true) => hide(app),
        Ok(false) => show(app, t0),
        Err(e) => log::warn!("读取面板可见性失败：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SIZE: (f64, f64) = (960.0, 640.0);

    #[test]
    fn centered_horizontally_top_at_one_sixth() {
        let work = Rect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert_eq!(place_in_work_area(work, SIZE), (480.0, 180.0));
    }

    #[test]
    fn respects_monitor_origin() {
        let work = Rect {
            x: 1920.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert_eq!(place_in_work_area(work, SIZE), (2400.0, 180.0));
    }

    #[test]
    fn clamps_into_small_work_area() {
        let work = Rect {
            x: 0.0,
            y: 25.0,
            w: 1280.0,
            h: 695.0,
        }; // 1280 × 720 减去 25 px 菜单栏
        let (x, y) = place_in_work_area(work, SIZE);
        assert_eq!(x, 160.0);
        assert_eq!(y, 80.0); // 25 + 695 − 640，贴底而不是出屏
    }
}
