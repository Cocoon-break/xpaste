//! 补全线程：位图尺寸与缩略图、图片文件缩略图、视频元数据与封面、来源应用图标。失败只记日志，不带路径与内容。

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use image::{DynamicImage, ImageReader};
use serde_json::json;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};

use crate::events::{RECORD_UPDATED, emit_summary};
use crate::state::AppState;

const THUMB_MAX: u32 = 320;

pub enum Job {
    Bitmap { id: i64 },
    ImageFile { id: i64, path: PathBuf },
    Video { id: i64, path: PathBuf },
    AppIcon { app_id: String },
}

/// 图标文件名（不含扩展名）：sha256(app_id) 前 16 字节的十六进制。Windows 的 app_id 是路径，不能直接当文件名。
pub fn icon_file_name(app_id: &str) -> String {
    Sha256::digest(app_id.as_bytes())[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn icon_path(dir: &Path, app_id: &str) -> PathBuf {
    dir.join("icons")
        .join(format!("{}.png", icon_file_name(app_id)))
}

/// 线程只在 `AppState` 托管后才会收到任务（发送端存在 `AppState` 里）。
pub fn start(app: AppHandle) -> mpsc::Sender<Job> {
    let (tx, rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("xpaste-enrich".into())
        .spawn(move || {
            for job in rx {
                handle(&app, job);
            }
        });
    if let Err(e) = spawned {
        log::warn!("补全线程启动失败：{e}");
    }
    tx
}

fn handle(app: &AppHandle, job: Job) {
    let state = app.state::<AppState>();
    match job {
        Job::Bitmap { id } => {
            let Some(img) = decode(&state.data_dir.join("images").join(format!("{id}.png"))) else {
                return;
            };
            let meta = json!({ "w": img.width(), "h": img.height() });
            if let Err(e) = state.store().set_meta(id, &meta) {
                log::warn!("记录 {id} 写入尺寸失败：{e}");
            }
            thumb(app, &state.data_dir, id, &img);
        }
        Job::ImageFile { id, path } => {
            if let Some(img) = decode(&path) {
                thumb(app, &state.data_dir, id, &img);
            }
        }
        Job::Video { id, path } => {
            // AVFoundation 在锁外读，读完再写库
            let meta = match xpaste_platform::video_meta(&path) {
                Some(m) => match state.store().set_meta(id, &json!(m)) {
                    Ok(()) => true,
                    Err(e) => {
                        log::warn!("记录 {id} 写入视频元数据失败：{e}");
                        false
                    }
                },
                None => {
                    log::debug!("记录 {id} 视频元数据读取失败");
                    false
                }
            };
            let poster = match xpaste_platform::video_poster(&path, THUMB_MAX) {
                Some(png) => {
                    std::fs::write(state.data_dir.join("thumbs").join(format!("{id}.png")), png)
                        .inspect_err(|e| log::warn!("记录 {id} 写入缩略图失败：{e}"))
                        .is_ok()
                }
                None => {
                    log::debug!("记录 {id} 视频封面生成失败");
                    false
                }
            };
            if meta || poster {
                emit_summary(app, RECORD_UPDATED, id);
            }
        }
        Job::AppIcon { app_id } => {
            let Some(png) = xpaste_platform::app_icon(&app_id) else {
                log::debug!("未取到应用图标");
                return;
            };
            let path = icon_path(&state.data_dir, &app_id);
            if let Err(e) = std::fs::write(&path, png) {
                log::warn!("写入应用图标失败：{e}");
                return;
            }
            let ids = {
                let store = state.store();
                store
                    .set_app_icon(&app_id, &path)
                    .and_then(|()| store.ids_for_app(&app_id))
            };
            match ids {
                Ok(ids) => ids
                    .into_iter()
                    .for_each(|id| emit_summary(app, RECORD_UPDATED, id)),
                Err(e) => log::warn!("保存应用图标失败：{e}"),
            }
        }
    }
}

/// 按内容识别格式；HEIC、SVG 等不支持的格式返回 None。
fn decode(path: &Path) -> Option<DynamicImage> {
    let img = ImageReader::open(path)
        .map_err(image::ImageError::IoError)
        .and_then(|r| r.with_guessed_format().map_err(image::ImageError::IoError))
        .and_then(|r| r.decode());
    img.map_err(|e| log::debug!("图片解码失败：{e}")).ok()
}

/// 长边不超过 320，不放大。
fn thumb(app: &AppHandle, dir: &Path, id: i64, img: &DynamicImage) {
    let rgba = if img.width().max(img.height()) > THUMB_MAX {
        img.thumbnail(THUMB_MAX, THUMB_MAX).to_rgba8()
    } else {
        img.to_rgba8()
    };
    match rgba.save(dir.join("thumbs").join(format!("{id}.png"))) {
        Ok(()) => emit_summary(app, RECORD_UPDATED, id),
        Err(e) => log::warn!("记录 {id} 写入缩略图失败：{e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::icon_file_name;

    #[test]
    fn icon_file_name_is_stable_32_hex() {
        let n = icon_file_name("com.apple.finder");
        assert_eq!(n.len(), 32);
        assert!(
            n.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(n, icon_file_name("com.apple.finder"));
        assert_ne!(n, icon_file_name("com.apple.Safari"));
    }
}
