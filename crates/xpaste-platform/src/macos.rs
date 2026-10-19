use std::thread;
use std::time::Duration;

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSWorkspace};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};
use objc2_foundation::{NSString, NSURL};
use xpaste_core::{AppInfo, Flavor, Snapshot};

use crate::{PasteBlock, PlatformError};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// `kVK_ANSI_V`
const KEY_V: u16 = 9;

pub const OS: &str = "macos";

pub fn change_count() -> u64 {
    NSPasteboard::generalPasteboard().changeCount() as u64
}

/// 起常驻线程，每 300 ms 比较 `changeCount`，变化时回调新序号。
pub fn watch(on_change: impl Fn(u64) + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("xpaste-clipboard-watch".into())
        .spawn(move || {
            let mut last = autoreleasepool(|_| change_count());
            loop {
                thread::sleep(Duration::from_millis(300));
                autoreleasepool(|_| {
                    let now = change_count();
                    if now != last {
                        last = now;
                        on_change(now);
                    }
                });
            }
        });
    if let Err(e) = spawned {
        log::error!("剪切板监听线程启动失败：{e}");
    }
}

pub fn read_snapshot() -> Result<Option<Snapshot>, PlatformError> {
    autoreleasepool(|_| {
        let Some(items) = NSPasteboard::generalPasteboard().pasteboardItems() else {
            return Ok(None);
        };
        if items.is_empty() {
            return Ok(None);
        }
        let source = frontmost_app();
        let sensitive = items.iter().any(|item| {
            item.types()
                .iter()
                .any(|t| crate::decode::is_sensitive_uti(&t.to_string()))
        });
        if sensitive {
            return Ok(Some(Snapshot {
                sensitive: true,
                source,
                ..Default::default()
            }));
        }
        // SAFETY: AppKit 导出的常量，进程内一直有效
        let text_type = unsafe { NSPasteboardTypeString };
        let mut snapshot = Snapshot {
            source,
            ..Default::default()
        };
        if let Some((i, text)) = items
            .iter()
            .enumerate()
            .find_map(|(i, item)| item.stringForType(text_type).map(|s| (i, s.to_string())))
        {
            snapshot.flavors.push(Flavor {
                item: i as u32,
                format: text_type.to_string(),
                data: text.as_bytes().to_vec(),
            });
            snapshot.text = Some(text);
        }
        Ok(Some(snapshot))
    })
}

pub fn frontmost_app() -> Option<AppInfo> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let name = app.localizedName().map(|s| s.to_string());
        let id = app
            .bundleIdentifier()
            .map(|s| s.to_string())
            .or(name.clone())?;
        Some(AppInfo {
            name: name.unwrap_or_else(|| id.clone()),
            id,
        })
    })
}

/// 清空剪切板后写入纯文本，返回写入后的 `changeCount`。
pub fn write_text(text: &str) -> Result<u64, PlatformError> {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        // SAFETY: AppKit 导出的常量，进程内一直有效
        let ok = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        if !ok {
            return Err(PlatformError::Clipboard("写入文本失败".into()));
        }
        Ok(change_count())
    })
}

/// macOS 只看辅助功能权限，忽略 `target`。
pub fn can_paste(_target: Option<isize>) -> PasteBlock {
    // SAFETY: 无参数的只读查询
    if unsafe { AXIsProcessTrusted() } != 0 {
        PasteBlock::None
    } else {
        PasteBlock::NeedAccessibility
    }
}

/// 向当前前台应用发 ⌘V。须在主线程调用。
pub fn paste(_target: Option<isize>) -> Result<(), PlatformError> {
    for down in [true, false] {
        let event = CGEvent::new_keyboard_event(None, KEY_V, down)
            .ok_or_else(|| PlatformError::Clipboard("创建按键事件失败".into()))?;
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }
    Ok(())
}

pub fn open_accessibility_settings() {
    autoreleasepool(|_| {
        let url = NSString::from_str(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        );
        match NSURL::URLWithString(&url) {
            Some(url) if NSWorkspace::sharedWorkspace().openURL(&url) => {}
            _ => log::warn!("打开辅助功能设置失败"),
        }
    })
}

/// macOS 粘贴发给当前前台应用，不需要记录窗口。
pub fn foreground_window() -> Option<isize> {
    None
}
