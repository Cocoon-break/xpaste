//! 清理：启动时清掉残留的软删除并按保留时长清理，之后每小时一次；删除 5 秒后彻底清除。

use std::time::Duration;

use tauri::{AppHandle, Manager};

use crate::clock::now_ms;
use crate::events::emit_removed;
use crate::state::AppState;

const PRUNE_EVERY: Duration = Duration::from_secs(3600);
const UNDO_WINDOW: Duration = Duration::from_secs(5);

pub fn start(app: AppHandle) {
    let purged = app.state::<AppState>().store().purge_all_soft_deleted();
    match purged {
        Ok(ids) => log::info!("启动清除软删除记录 {} 条", ids.len()),
        Err(e) => log::warn!("清除软删除记录失败：{e}"),
    }
    prune(&app);
    spawn("xpaste-housekeeping", move || {
        loop {
            std::thread::sleep(PRUNE_EVERY);
            prune(&app);
        }
    });
}

/// 撤销窗口过后，若记录仍是这次软删除则彻底删除。
pub fn schedule_purge(app: &AppHandle, id: i64, deleted_at: i64) {
    let app = app.clone();
    spawn("xpaste-purge", move || {
        std::thread::sleep(UNDO_WINDOW);
        let r = app
            .state::<AppState>()
            .store()
            .purge_if_deleted_at(id, deleted_at);
        if let Err(e) = r {
            log::warn!("清除记录 {id} 失败：{e}");
        }
    });
}

fn prune(app: &AppHandle) {
    let state = app.state::<AppState>();
    let days = state.settings().retention_days;
    let removed = state.store().prune_retention(days, now_ms());
    match removed {
        Ok(ids) => {
            if !ids.is_empty() {
                log::info!("保留时长清理 {} 条", ids.len());
            }
            emit_removed(app, &ids);
        }
        Err(e) => log::warn!("保留时长清理失败：{e}"),
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        log::warn!("线程 {name} 启动失败：{e}");
    }
}
