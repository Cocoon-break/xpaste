use std::collections::HashSet;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use objc2::AllocAnyThread;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard, NSPasteboardItem,
    NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypeString, NSWorkspace,
};
use objc2_core_graphics::{CGEvent, CGEventFlags, CGEventTapLocation};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL, ns_string,
};
use xpaste_core::{AppInfo, Flavor, Snapshot};

use crate::{PasteBlock, PlatformError};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const NSDictionary<NSString, NSNumber>) -> u8;
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

/// `skip_sensitive` 为真时，敏感内容只返回标记、不读取内容；为假时照常读取，仍标记 `sensitive`。
pub fn read_snapshot(skip_sensitive: bool) -> Result<Option<Snapshot>, PlatformError> {
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
        if sensitive && skip_sensitive {
            return Ok(Some(Snapshot {
                sensitive: true,
                source,
                ..Default::default()
            }));
        }
        let mut snapshot = Snapshot {
            sensitive,
            source,
            ..Default::default()
        };
        let mut seen = HashSet::new();
        for (i, item) in items.iter().enumerate() {
            let types = item.types();
            let names: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            for (uti, format) in types.iter().zip(names.iter().cloned()) {
                // 先按类型名过滤：dataForType 会触发来源应用的延迟渲染
                if crate::decode::should_skip_uti(&format, &names)
                    || !seen.insert((i, format.clone()))
                {
                    continue;
                }
                if let Some(data) = item.dataForType(&uti) {
                    snapshot.flavors.push(Flavor {
                        item: i as u32,
                        format,
                        data: data.to_vec(),
                    });
                }
            }
        }
        let first_string = |uti: &NSString| {
            items
                .iter()
                .find_map(|item| item.stringForType(uti).map(|s| s.to_string()))
        };
        // SAFETY: AppKit 导出的常量，进程内一直有效
        let (text_type, html_type, file_type) = unsafe {
            (
                NSPasteboardTypeString,
                NSPasteboardTypeHTML,
                NSPasteboardTypeFileURL,
            )
        };
        snapshot.text = first_string(text_type).map(|t| crate::decode::normalize_newlines(&t));
        snapshot.html = first_string(html_type);
        snapshot.url_title = first_string(ns_string!("public.url-name"));
        for item in items.iter() {
            let path = item
                .stringForType(file_type)
                .and_then(|s| NSURL::URLWithString(&s))
                .filter(|u| u.isFileURL())
                // 文件引用 URL（file:///.file/id=…）先转成路径 URL
                .and_then(|u| u.filePathURL())
                .and_then(|u| u.path())
                .map(|p| PathBuf::from(p.to_string()));
            if let Some(p) = path
                && !snapshot.file_paths.contains(&p)
            {
                snapshot.file_paths.push(p);
            }
        }
        if snapshot.file_paths.is_empty() {
            snapshot.bitmap = read_bitmap(&snapshot.flavors);
        }
        Ok(Some(snapshot))
    })
}

/// 已读出的 flavor 中第一个 PNG 原样返回；否则第一个 TIFF 转成 PNG。
fn read_bitmap(flavors: &[Flavor]) -> Option<Vec<u8>> {
    let first = |uti: &str| flavors.iter().find(|f| f.format == uti);
    if let Some(png) = first("public.png") {
        return Some(png.data.clone());
    }
    let tiff = first("public.tiff")?;
    let png = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&tiff.data))
        .and_then(|rep| to_png(&rep));
    if png.is_none() {
        log::debug!("TIFF 转 PNG 失败 bytes={}", tiff.data.len());
    }
    png
}

fn to_png(rep: &NSBitmapImageRep) -> Option<Vec<u8>> {
    // SAFETY: 空字典，类型参数无关
    unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .map(|d| d.to_vec())
}

/// 按 `item` 分组（保持首次出现顺序）写回剪切板，返回写入后的 `changeCount`。
pub fn write(flavors: &[Flavor]) -> Result<u64, PlatformError> {
    if flavors.iter().all(|f| f.format.is_empty()) {
        return Err(PlatformError::Clipboard("没有可写入的格式".into()));
    }
    autoreleasepool(|_| {
        let mut groups: Vec<(u32, Retained<NSPasteboardItem>)> = Vec::new();
        for f in flavors.iter().filter(|f| !f.format.is_empty()) {
            let idx = match groups.iter().position(|(i, _)| *i == f.item) {
                Some(idx) => idx,
                None => {
                    groups.push((f.item, NSPasteboardItem::new()));
                    groups.len() - 1
                }
            };
            groups[idx]
                .1
                .setData_forType(&NSData::with_bytes(&f.data), &NSString::from_str(&f.format));
        }
        let items: Vec<_> = groups
            .into_iter()
            .map(|(_, item)| ProtocolObject::from_retained(item))
            .collect();
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        if !pb.writeObjects(&NSArray::from_retained_slice(&items)) {
            return Err(PlatformError::Clipboard("writeObjects 失败".into()));
        }
        Ok(change_count())
    })
}

/// 应用图标转 PNG：取 64 pt 对应的那档图像（Retina 屏上为 128×128 px），不重新绘制。
pub fn app_icon(app_id: &str) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let ws = NSWorkspace::sharedWorkspace();
        let path = ws
            .URLForApplicationWithBundleIdentifier(&NSString::from_str(app_id))?
            .path()?;
        let image = ws.iconForFile(&path);
        let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0));
        // SAFETY: rect 是有效的栈上变量；不传 context 与 hints
        let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
        to_png(&NSBitmapImageRep::initWithCGImage(
            NSBitmapImageRep::alloc(),
            &cg,
        ))
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

/// 未授权时弹出系统辅助功能授权对话框并把本应用加入列表；已授权时无操作。须在主线程调用。
pub fn request_accessibility() {
    autoreleasepool(|_| {
        // `kAXTrustedCheckOptionPrompt` 的值
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let yes = NSNumber::new_bool(true);
        let options = NSDictionary::from_slices(&[&*key], &[&*yes]);
        // SAFETY: NSDictionary 与 CFDictionaryRef 免费桥接，options 在调用期间存活
        unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options)) };
    })
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
