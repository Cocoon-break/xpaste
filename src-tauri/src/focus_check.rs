//! 开发版自检：依次执行面板与晾衣绳之间换键盘的各种组合，每步之后核对窗口状态。
//! 这类问题（面板留在屏上却没有键盘，Esc、点外面都收不掉）只在真实的窗口系统里出现，单元测试测不到。
//!
//! 用法：`XPASTE_FOCUS_CHECK=<文件路径> make dev-isolated`，应用就绪后建出这个文件（`touch`）就跑一轮，
//! 约 20 秒，期间面板和晾衣绳会自己出现、消失，不要碰键盘鼠标。结果写进日志，`FOCUS` 开头，
//! 最后一行是 `FOCUS 完成：N 项，失败 M 项`。只编进开发构建。

use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::line::{self, How};
use crate::panel::{self, Trigger};

/// 面板、晾衣绳此刻的状态
#[derive(Clone, Copy, Debug)]
struct Snap {
    panel: bool,
    panel_key: bool,
    line: bool,
    /// 只出现在失败信息里
    #[allow(dead_code)]
    line_key: bool,
}

enum Step {
    PanelShow,
    PanelHide,
    /// 晾衣绳放下
    Line(How),
    /// 晾衣绳拿键盘（等于用户点了便签）
    LineKey,
    /// 晾衣绳收起（走页面的收起流程）
    LineHide,
    Wait(u64),
    /// 核对：面板是否在屏上；在屏上时必须拿着键盘
    Panel(bool),
    /// 核对：晾衣绳是否在屏上
    LineUp(bool),
}
use Step::*;

fn scenarios() -> Vec<(&'static str, Vec<Step>)> {
    vec![
        (
            "面板唤起再收起",
            vec![
                PanelShow,
                Wait(500),
                Panel(true),
                PanelHide,
                Wait(500),
                Panel(false),
            ],
        ),
        (
            "晾衣绳放下（没拿键盘）时唤起面板，晾衣绳收起后面板还能用",
            vec![
                Line(How::Mouse),
                Wait(300),
                PanelShow,
                Wait(500),
                Panel(true),
                LineHide,
                Wait(1500),
                LineUp(false),
                Panel(true),
            ],
        ),
        (
            "晾衣绳拿着键盘时唤起面板，晾衣绳收起后面板还能用",
            vec![
                Line(How::Mouse),
                Wait(300),
                LineKey,
                Wait(400),
                PanelShow,
                Wait(500),
                Panel(true),
                LineHide,
                Wait(1500),
                LineUp(false),
                Panel(true),
            ],
        ),
        (
            "键盘打开的晾衣绳上唤起面板，晾衣绳自己收起，面板能用",
            vec![
                Line(How::Kbd),
                Wait(500),
                PanelShow,
                Wait(1800),
                LineUp(false),
                Panel(true),
            ],
        ),
        (
            "面板开着时点晾衣绳，面板按失焦收起",
            vec![
                PanelShow,
                Wait(400),
                Line(How::Mouse),
                Wait(300),
                Panel(true),
                LineKey,
                Wait(800),
                // 不核对晾衣绳还在：自检时真实鼠标不在绳子上，页面会按规则把它收起
                Panel(false),
            ],
        ),
        (
            "点过晾衣绳后再唤起面板，面板能用、能收起",
            vec![
                PanelShow,
                Wait(400),
                Line(How::Mouse),
                Wait(300),
                LineKey,
                Wait(800),
                Panel(false),
                PanelShow,
                Wait(500),
                Panel(true),
                PanelHide,
                Wait(500),
                Panel(false),
            ],
        ),
        (
            "面板开着时晾衣绳亮一下再自己收起，面板不受影响",
            vec![
                PanelShow,
                Wait(400),
                Line(How::Flash),
                Wait(500),
                Panel(true),
                Wait(3500),
                Panel(true),
            ],
        ),
        (
            "面板开着时提醒放下晾衣绳，面板不受影响",
            vec![
                PanelShow,
                Wait(400),
                Line(How::Remind),
                Wait(600),
                Panel(true),
                LineHide,
                Wait(1500),
                Panel(true),
            ],
        ),
        (
            "晾衣绳拿着键盘、面板开着，面板收起后再唤起",
            vec![
                Line(How::Mouse),
                Wait(300),
                LineKey,
                Wait(400),
                PanelShow,
                Wait(500),
                Panel(true),
                PanelHide,
                Wait(500),
                Panel(false),
                PanelShow,
                Wait(500),
                Panel(true),
            ],
        ),
        (
            "连续快速唤起、收起",
            vec![
                PanelShow,
                Wait(60),
                PanelHide,
                Wait(60),
                PanelShow,
                Wait(60),
                PanelHide,
                Wait(60),
                PanelShow,
                Wait(700),
                Panel(true),
                PanelHide,
                Wait(600),
                Panel(false),
            ],
        ),
    ]
}

/// 设了 `XPASTE_FOCUS_CHECK` 时等那个文件出现就跑一轮。
pub fn start(app: &AppHandle) {
    let Some(go) = std::env::var_os("XPASTE_FOCUS_CHECK") else {
        return;
    };
    let go = std::path::PathBuf::from(go);
    let app = app.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if std::fs::remove_file(&go).is_ok() {
                run(&app);
            }
        }
    });
}

fn main<T: Send + 'static>(
    app: &AppHandle,
    f: impl FnOnce(&AppHandle) -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = mpsc::channel();
    panel::on_main(app, move |app| {
        let _ = tx.send(f(app));
    });
    rx.recv_timeout(Duration::from_secs(5)).ok()
}

fn snap(app: &AppHandle) -> Option<Snap> {
    main(app, |app| {
        let line = app.get_webview_window(line::LABEL);
        Snap {
            panel: panel::visible(app),
            panel_key: panel::holds_key(app),
            line: line
                .as_ref()
                .is_some_and(|w| w.is_visible().unwrap_or(false)),
            line_key: line
                .as_ref()
                .is_some_and(|w| w.is_focused().unwrap_or(false)),
        }
    })
}

fn reset(app: &AppHandle) {
    panel::hide(app);
    main(app, line::hidden);
    std::thread::sleep(Duration::from_millis(700));
}

fn run(app: &AppHandle) {
    if line::pinned() || !line::active() {
        log::warn!("FOCUS 跳过：晾衣绳关着或设成了一直显示，先在设置里改回来");
        return;
    }
    let all = scenarios();
    log::info!("FOCUS 开始：{} 项", all.len());
    let mut failed = 0;
    for (name, steps) in &all {
        reset(app);
        let saves = panel::WATCH_HITS.load(Ordering::SeqCst);
        let mut errs: Vec<String> = Vec::new();
        for (i, step) in steps.iter().enumerate() {
            match step {
                PanelShow => panel::show(app, None, Trigger::Hotkey),
                PanelHide => panel::hide(app),
                Line(how) => line::show(app, *how),
                LineKey => line::keyboard(app, true),
                LineHide => line::hide(app),
                Wait(ms) => std::thread::sleep(Duration::from_millis(*ms)),
                Panel(want) => match snap(app) {
                    None => errs.push(format!("第 {} 步：主线程 5 秒没响应", i + 1)),
                    Some(s) if s.panel != *want => errs.push(format!(
                        "第 {} 步：面板应{}，实际 {s:?}",
                        i + 1,
                        if *want { "在屏上" } else { "已收起" }
                    )),
                    Some(s) if s.panel && !s.panel_key => {
                        errs.push(format!("第 {} 步：面板在屏上却没有键盘 {s:?}", i + 1))
                    }
                    Some(_) => {}
                },
                LineUp(want) => match snap(app) {
                    Some(s) if s.line != *want => errs.push(format!(
                        "第 {} 步：晾衣绳应{}，实际 {s:?}",
                        i + 1,
                        if *want { "在屏上" } else { "已收起" }
                    )),
                    _ => {}
                },
            }
        }
        // 靠看门兜住的不算干净：面板会晚 0.6 秒才收，而且是收起、不是留着能用
        if panel::WATCH_HITS.load(Ordering::SeqCst) != saves {
            errs.push("靠看门才收起的".into());
        }
        if errs.is_empty() {
            log::info!("FOCUS 通过：{name}");
        } else {
            failed += 1;
            log::warn!("FOCUS 失败：{name}：{}", errs.join("；"));
        }
    }
    reset(app);
    log::info!("FOCUS 完成：{} 项，失败 {failed} 项", all.len());
}
