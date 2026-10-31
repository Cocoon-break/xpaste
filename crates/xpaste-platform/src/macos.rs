use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AllocAnyThread, ClassType, msg_send};
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSBitmapImageFileType, NSBitmapImageRep, NSPasteboard,
    NSPasteboardItem, NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypeString,
    NSWorkspace,
};
use objc2_av_foundation::{AVAssetImageGenerator, AVAssetTrack, AVMediaTypeVideo, AVURLAsset};
use objc2_core_foundation::CFData;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGBlendMode, CGColorRenderingIntent, CGColorSpace,
    CGContext, CGDataProvider, CGDirectDisplayID, CGDisplayBounds, CGDisplayMirrorsDisplay,
    CGError, CGEvent, CGEventFlags, CGEventTapLocation, CGGetActiveDisplayList, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption,
    kCGColorSpaceSRGB, kCGNullWindowID,
};
use objc2_core_media::{CMFormatDescription, CMTime};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSFileManager, NSIndexSet, NSLocale, NSNumber,
    NSOperatingSystemVersion, NSPoint, NSProcessInfo, NSRect, NSSize, NSString, NSURL, ns_string,
};
use objc2_vision::{
    VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
};
use xpaste_core::{AppInfo, Flavor, Snapshot, VideoMeta};

#[path = "macos_record.rs"]
mod macos_record;
pub use macos_record::{
    Recorder, file_flavors, microphone_access, request_microphone, start_recording, window_number,
};

use crate::{
    Frame, OcrError, OcrText, PIN_TITLE_PREFIX, PackStatus, PasteBlock, PlatformError,
    SysTranslateError, WinRect,
};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: *const NSDictionary<NSString, NSNumber>) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventSourceButtonState(state_id: i32, button: u32) -> u8;
}

/// 回调：`code` 0 成功（`out` 为译文），1 缺语言包（`out` 为源语言代码），2 不支持，3 其他失败。
/// 可能在任意线程上调用，`out` 只在回调期间有效。
type TrCallback = extern "C" fn(ctx: *mut c_void, code: i32, out: *const c_char);

// swift/translate.swift 编成的静态库，由 build.rs 链接（M26 规则 7）
unsafe extern "C" {
    fn xp_tr_pack(target: *const c_char) -> i32;
    fn xp_tr_translate(
        text: *const c_char,
        target: *const c_char,
        ctx: *mut c_void,
        cb: TrCallback,
    );
    fn xp_tr_prepare(
        source: *const c_char,
        target: *const c_char,
        window: *mut c_void,
        ctx: *mut c_void,
        cb: extern "C" fn(ctx: *mut c_void, code: i32),
    );
}

// libSystem，总是链接
unsafe extern "C" {
    fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
}

/// `VNRecognizeTextRequestRevision2`（SDK 里标了弃用，值固定）
const TEXT_REVISION_2: usize = 2;

/// `QOS_CLASS_UTILITY`
const QOS_CLASS_UTILITY: u32 = 0x11;

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
        } else {
            prefer_bitmap_over_unreadable_files(&mut snapshot);
        }
        Ok(Some(snapshot))
    })
}

/// 微信等沙盒应用复制图片时同时给位图和自己容器里的临时文件路径：路径没有权限读，且很快被删掉，
/// 按文件记就预览不了。文件都不可用且有位图时，丢掉路径按位图记（M02 规则 4 补充）。
fn prefer_bitmap_over_unreadable_files(snapshot: &mut Snapshot) {
    // 没有位图就不碰文件：打开 FIFO 会卡住监听线程，打开桌面等目录里的文件可能弹授权框
    let has_bitmap = snapshot
        .flavors
        .iter()
        .any(|f| f.format == "public.png" || f.format == "public.tiff");
    if !has_bitmap || snapshot.file_paths.iter().any(|p| usable_file(p)) {
        return;
    }
    if let Some(bitmap) = read_bitmap(&snapshot.flavors) {
        snapshot.file_paths.clear();
        // 粘贴时不写回已失效的文件引用，目标应用只拿到位图
        snapshot.flavors.retain(|f| f.format != "public.file-url");
        snapshot.bitmap = Some(bitmap);
    }
}

/// 其他应用沙盒容器（`~/Library/Containers/`、`~/Library/Group Containers/`）里的路径不去打开：
/// 读不了，打开还可能弹出“访问其他应用的数据”授权框；即使有完全磁盘访问权限，那也是对方的临时文件，随时会被删掉。
fn usable_file(p: &Path) -> bool {
    let in_container = std::env::var_os("HOME")
        .map(PathBuf::from)
        .is_some_and(|h| {
            p.starts_with(h.join("Library/Containers"))
                || p.starts_with(h.join("Library/Group Containers"))
        });
    !in_container && std::fs::File::open(p).is_ok()
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

/// 截图写剪切板用的格式：只写 `public.png`。
pub fn png_flavors(png: Vec<u8>) -> Vec<Flavor> {
    vec![Flavor {
        item: 0,
        format: "public.png".into(),
        data: png,
    }]
}

/// 逐块显示器截整屏（主屏在前，镜像屏跳过），像素转成 sRGB。阻塞；2880×1800 实测约 55–75 ms（其中色彩转换约一半），进程内首次约 135 ms。
/// 没有屏幕录制权限时不报错，只截到桌面壁纸和菜单栏，调用方须先查 `screen_capture_granted()`。
/// ponytail: CGDisplayCreateImage 在 macOS 15 SDK 已标记废弃；Apple 真正移除或截图出现异常时换 SCScreenshotManager，平台层签名不变
pub fn freeze_displays() -> Result<Vec<Frame>, PlatformError> {
    let mut ids = [0 as CGDirectDisplayID; 16];
    let mut n = 0u32;
    // SAFETY: 两个输出指针指向本地变量，ids 容量随参数传入
    let e = unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut n) };
    if e != CGError::Success {
        return Err(PlatformError::Capture(format!("取显示器列表失败：{}", e.0)));
    }
    // 第一项即主屏（CGGetActiveDisplayList 的约定）
    ids[..n as usize]
        .iter()
        .filter(|&&id| CGDisplayMirrorsDisplay(id) == 0)
        .map(|&id| freeze_display(id))
        .collect()
}

#[allow(deprecated)] // 见 freeze_displays 的 ponytail 注释
fn freeze_display(id: CGDirectDisplayID) -> Result<Frame, PlatformError> {
    let err = |what: &str| PlatformError::Capture(what.into());
    let bounds = CGDisplayBounds(id);
    let img = objc2_core_graphics::CGDisplayCreateImage(id).ok_or_else(|| err("截屏失败"))?;
    let (w, h) = (CGImage::width(Some(&img)), CGImage::height(Some(&img)));
    let (w32, h32) = match (u32::try_from(w), u32::try_from(h)) {
        (Ok(a), Ok(b)) if a > 0 && b > 0 => (a, b),
        _ => return Err(err("截屏尺寸无效")),
    };
    let rgba = srgb_rgba(&img, w, h).ok_or_else(|| err("像素转换失败"))?;
    let size = (bounds.size.width, bounds.size.height);
    Ok(Frame {
        id,
        origin: (bounds.origin.x, bounds.origin.y),
        size,
        scale: if size.0 > 0.0 { w as f64 / size.0 } else { 1.0 },
        px: (w32, h32),
        rgba,
    })
}

/// 把图像画进 sRGB、RGBA8 的位图上下文，返回非预乘、行紧密排列的像素。屏幕图带的是显示器的色彩描述
/// （如 P3），前端画布与 PNG 都按 sRGB 解读，不转换的话颜色发灰。任意位深（10 位、HDR）都由 Quartz 转成 8 位。
fn srgb_rgba(img: &CGImage, w: usize, h: usize) -> Option<Vec<u8>> {
    // SAFETY: CoreGraphics 导出的常量，进程内一直有效
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let mut buf = vec![0u8; w.checked_mul(h)?.checked_mul(4)?];
    // SAFETY: buf 正好容纳 h 行 w * 4 字节；上下文在本函数内释放，早于 buf 被移出
    let ctx = unsafe {
        CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            Some(&space),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }?;
    // 直接覆盖，不与底色混合
    CGContext::set_blend_mode(Some(&ctx), CGBlendMode::Copy);
    let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(w as f64, h as f64));
    CGContext::draw_image(Some(&ctx), rect, Some(img));
    drop(ctx);
    unpremultiply(&mut buf);
    Some(buf)
}

/// 预乘 RGBA 原地还原成非预乘。屏幕像素都不透明，只有 alpha 不是 255 的像素才计算。
fn unpremultiply(rgba: &mut [u8]) {
    for p in rgba.as_chunks_mut::<4>().0 {
        match p[3] {
            255 => {}
            0 => *p = [0; 4],
            a => {
                let a = u32::from(a);
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
    }
}

/// 屏上窗口外框，由前到后：别的应用只取 layer 0、不透明度大于 0 的普通窗口；本进程只取贴图窗口。
/// 不读其他应用的窗口标题（需要屏幕录制权限，也用不到）。
pub fn window_rects() -> Vec<WinRect> {
    let option =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(option, kCGNullWindowID) else {
        return Vec::new();
    };
    let me = i64::from(std::process::id());
    autoreleasepool(|_| {
        // SAFETY: CFArray 与 NSArray 免费桥接，元素都是 CFDictionary；list 在闭包内存活
        let list: &NSArray<NSDictionary<NSString, AnyObject>> = unsafe {
            &*(objc2_core_foundation::CFRetained::as_ptr(&list)
                .as_ptr()
                .cast())
        };
        list.iter()
            .filter(|w| {
                let num = |k: &str| {
                    w.objectForKey(&NSString::from_str(k))
                        .and_then(|v| v.downcast::<NSNumber>().ok())
                };
                if num("kCGWindowOwnerPID").map(|n| n.longLongValue()) == Some(me) {
                    w.objectForKey(ns_string!("kCGWindowName"))
                        .and_then(|v| v.downcast::<NSString>().ok())
                        .is_some_and(|t| t.to_string().starts_with(PIN_TITLE_PREFIX))
                } else {
                    num("kCGWindowLayer").is_some_and(|n| n.longLongValue() == 0)
                        && num("kCGWindowAlpha").is_some_and(|n| n.doubleValue() > 0.0)
                }
            })
            .filter_map(|w| {
                let b = w.objectForKey(ns_string!("kCGWindowBounds"))?;
                // SAFETY: kCGWindowBounds 的值是 CGRect 的字典表示（X、Y、Width、Height 为 NSNumber）
                let b: Retained<NSDictionary<NSString, AnyObject>> =
                    unsafe { Retained::cast_unchecked(b) };
                let f = |k: &str| {
                    b.objectForKey(&NSString::from_str(k))
                        .and_then(|v| v.downcast::<NSNumber>().ok())
                        .map(|n| n.doubleValue())
                };
                let r = WinRect {
                    x: f("X")?,
                    y: f("Y")?,
                    w: f("Width")?,
                    h: f("Height")?,
                };
                (r.w > 0.0 && r.h > 0.0).then_some(r)
            })
            .collect()
    })
}

/// 是否已有屏幕录制权限（只查询，不弹窗）。
pub fn screen_capture_granted() -> bool {
    CGPreflightScreenCaptureAccess()
}

/// 未授权时请求系统授权（无 TCC 记录时弹框），再打开系统设置的“屏幕录制”页，理由同 `request_accessibility`。
/// 须在主线程调用。
pub fn request_screen_capture() {
    if !CGRequestScreenCaptureAccess() {
        open_privacy_pane("Privacy_ScreenCapture");
    }
}

/// macOS 主版本号（如 15）。
pub fn macos_version() -> Option<u32> {
    u32::try_from(
        NSProcessInfo::processInfo()
            .operatingSystemVersion()
            .majorVersion,
    )
    .ok()
}

/// 鼠标主键此刻是否按着（硬件状态，系统拖动窗口期间也准）。贴图据此判断拖动是否结束。
pub fn mouse_button_down() -> bool {
    // kCGEventSourceStateCombinedSessionState = 0，kCGMouseButtonLeft = 0
    // SAFETY: 纯查询，无指针参数
    unsafe { CGEventSourceButtonState(0, 0) != 0 }
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

fn video_asset(path: &Path) -> Retained<AVURLAsset> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: 不传选项
    unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) }
}

/// 有限的秒数，否则 0。
fn seconds(t: CMTime) -> f64 {
    // SAFETY: 纯计算
    let s = unsafe { t.seconds() };
    if s.is_finite() { s } else { 0.0 }
}

/// 首条视频轨。同步接口会阻塞，只在补全线程调用。
#[allow(deprecated)] // 异步的 loadTracksWithMediaType 要回调，补全线程本来就可以阻塞
fn first_video_track(asset: &AVURLAsset) -> Option<Retained<AVAssetTrack>> {
    // SAFETY: AVFoundation 导出的常量，进程内一直有效
    let video = unsafe { AVMediaTypeVideo }?;
    // SAFETY: video 是有效的媒体类型
    unsafe { asset.tracksWithMediaType(video) }.firstObject()
}

/// 读时长、方向校正后的分辨率、帧率与编码。无视频轨或 AVFoundation 打不开（如 mkv）时返回 None。
pub fn video_meta(path: &Path) -> Option<VideoMeta> {
    autoreleasepool(|_| {
        let asset = video_asset(path);
        let track = first_video_track(&asset)?;
        // SAFETY: 以下都是只读属性
        let (size, t, fps, formats) = unsafe {
            (
                track.naturalSize(),
                track.preferredTransform(),
                track.nominalFrameRate(),
                track.formatDescriptions(),
            )
        };
        let (w, h) = oriented_size(size.width, size.height, t.a, t.b);
        let codec = formats.firstObject().map_or_else(String::new, |f| {
            let f: *const AnyObject = &*f;
            // SAFETY: formatDescriptions 的元素都是 CMFormatDescriptionRef（CF 对象与 Objective-C 对象免费桥接）
            let sub = unsafe { (*f.cast::<CMFormatDescription>()).media_sub_type() };
            crate::codec_label(&String::from_utf8_lossy(&sub.to_be_bytes()))
        });
        Some(VideoMeta {
            // SAFETY: 只读属性
            duration_sec: seconds(unsafe { asset.duration() }),
            w,
            h,
            fps: round_fps(fps),
            codec,
        })
    })
}

/// 封面：第 min(1 s, 时长 × 10%) 处的一帧，按拍摄方向转正，长边不超过 `max_px`，PNG。
pub fn video_poster(path: &Path, max_px: u32) -> Option<Vec<u8>> {
    autoreleasepool(|_| {
        let asset = video_asset(path);
        // SAFETY: 以下调用参数都是有效值；actual_time 传 null
        let cg = unsafe {
            let at = (seconds(asset.duration()) * 0.1).min(1.0);
            let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
            generator.setAppliesPreferredTrackTransform(true);
            generator.setMaximumSize(NSSize::new(f64::from(max_px), f64::from(max_px)));
            #[allow(deprecated)] // 异步接口要回调，补全线程本来就可以阻塞
            generator.copyCGImageAtTime_actualTime_error(
                CMTime::with_seconds(at, 600),
                std::ptr::null_mut(),
            )
        }
        .ok()?;
        to_png(&NSBitmapImageRep::initWithCGImage(
            NSBitmapImageRep::alloc(),
            &cg,
        ))
    })
}

/// 方向校正：`preferredTransform` 的 `|b| > |a|` 视为四分之一转（90° / 270°），交换宽高。
fn oriented_size(w: f64, h: f64, a: f64, b: f64) -> (u32, u32) {
    let (w, h) = if b.abs() > a.abs() { (h, w) } else { (w, h) };
    (w.abs().round() as u32, h.abs().round() as u32)
}

/// 可变帧率等情况下 `nominalFrameRate` 可能为 0 或非有限，记 0（界面不显示）；否则保留两位小数。
fn round_fps(f: f32) -> f64 {
    if !f.is_finite() || f <= 0.0 {
        return 0.0;
    }
    (f64::from(f) * 100.0).round() / 100.0
}

/// Vision 文字识别 macOS 10.15 起恒有。
pub fn ocr_available() -> bool {
    true
}

/// Vision `VNRecognizeTextRequest`（accurate、语言校正、简繁中英）。`png` 为图片文件字节（按内容识别格式）。
/// 阻塞，只在工作线程调用。
pub fn ocr_png_bytes(png: &[u8], max_side_px: u32) -> Result<Option<OcrText>, OcrError> {
    let img = crate::load_rgba(png, max_side_px)?;
    let engine = |what: &str| OcrError::Engine(what.into());
    autoreleasepool(|_| {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(img.as_raw())))
            .ok_or_else(|| engine("CGDataProvider 创建失败"))?;
        let space = CGColorSpace::new_device_rgb().ok_or_else(|| engine("色彩空间创建失败"))?;
        // SAFETY: 尺寸与 RGBA8 缓冲一致；decode 传 null
        let cg = unsafe {
            CGImage::new(
                w,
                h,
                8,
                32,
                w * 4,
                Some(&space),
                CGBitmapInfo(CGImageAlphaInfo::Last.0),
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .ok_or_else(|| engine("CGImage 创建失败"))?;
        // SAFETY: 空选项字典
        let handler = unsafe {
            VNImageRequestHandler::initWithCGImage_options(
                VNImageRequestHandler::alloc(),
                &cg,
                &NSDictionary::new(),
            )
        };
        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        // 固定第 2 版模型（macOS 11+，含简繁中英）：默认的第 3 版在 macOS 27.0.1（M1）上首次识别要 30 s 以上，
        // 之后同进程内每次都报 e5rt 错误；不支持第 2 版的旧系统用默认版本
        // SAFETY: 无参数的类方法，返回 NSIndexSet
        let revisions: Retained<NSIndexSet> =
            unsafe { msg_send![VNRecognizeTextRequest::class(), supportedRevisions] };
        if revisions.containsIndex(TEXT_REVISION_2) {
            // SAFETY: 系统声明支持的版本号
            unsafe { request.setRevision(TEXT_REVISION_2) };
        }
        // 只设本机支持的语言；查询失败或都不支持时用系统默认
        // SAFETY: 只读查询
        if let Ok(supported) = unsafe { request.supportedRecognitionLanguagesAndReturnError() } {
            let langs: Vec<_> = ["zh-Hans", "zh-Hant", "en-US"]
                .into_iter()
                .map(NSString::from_str)
                .filter(|l| supported.containsObject(l))
                .collect();
            if !langs.is_empty() {
                request.setRecognitionLanguages(&NSArray::from_retained_slice(&langs));
            }
        }
        let base: &VNRequest = &request;
        handler
            .performRequests_error(&NSArray::from_slice(&[base]))
            .map_err(|e| OcrError::Engine(e.localizedDescription().to_string()))?;
        let lines = request
            .results()
            .map(|obs| {
                obs.iter()
                    .filter_map(|o| o.topCandidates(1).firstObject())
                    .map(|t| (t.string().to_string(), t.confidence()))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(crate::ocr_text(lines)))
    })
}

/// 系统翻译（`TranslationSession(installedSource:target:)`）需要 macOS 26（M26 规则 7）。
pub fn translate_supported() -> bool {
    NSProcessInfo::processInfo().isOperatingSystemAtLeastVersion(NSOperatingSystemVersion {
        majorVersion: 26,
        minorVersion: 0,
        patchVersion: 0,
    })
}

/// 英语 ↔ `target` 的语言包状态；`target` 为 `en` 时查简体中文 ↔ 英语。阻塞（本机查询，很快）。
/// Translation 框架要靠主队列回调：只在工作线程调用，且主线程在跑事件循环；在主线程上调用会等 10 秒后返回 `Unsupported`。
pub fn translate_pack(target: &str) -> PackStatus {
    let Ok(target) = CString::new(target) else {
        return PackStatus::Unsupported;
    };
    if !translate_supported() {
        return PackStatus::Unsupported;
    }
    // SAFETY: 以 NUL 结尾的字符串，调用期间有效
    match unsafe { xp_tr_pack(target.as_ptr()) } {
        0 => PackStatus::Installed,
        1 => PackStatus::Download,
        _ => PackStatus::Unsupported,
    }
}

/// 系统翻译超时（M26 规则 10）
const TRANSLATE_TIMEOUT: Duration = Duration::from_secs(30);

type TrSender = mpsc::Sender<Result<String, SysTranslateError>>;

extern "C" fn on_translated(ctx: *mut c_void, code: i32, out: *const c_char) {
    // SAFETY: ctx 是 translate_text 里 Box::into_raw 的发送端，Swift 侧恰好回调一次，这里收回所有权
    let tx = unsafe { Box::from_raw(ctx.cast::<TrSender>()) };
    // SAFETY: out 非空时为回调期间有效的 C 字符串
    let s = || {
        unsafe { CStr::from_ptr(out) }
            .to_string_lossy()
            .into_owned()
    };
    let r = match code {
        0 if !out.is_null() => Ok(s()),
        1 if !out.is_null() => Err(SysTranslateError::NotInstalled { source: s() }),
        2 => Err(SysTranslateError::Unsupported),
        _ => Err(SysTranslateError::Failed),
    };
    // 已超时则接收端已丢弃，send 失败忽略
    let _ = tx.send(r);
}

/// 用系统翻译把 `text` 译成 `target`（源语言由系统识别）。阻塞，最多 30 秒。
/// 同 `translate_pack`，只在工作线程调用且主线程在跑事件循环，否则只会等到 `Timeout`。
pub fn translate_text(text: &str, target: &str) -> Result<String, SysTranslateError> {
    if !translate_supported() {
        return Err(SysTranslateError::OsTooOld);
    }
    let (Ok(text), Ok(target)) = (CString::new(text), CString::new(target)) else {
        return Err(SysTranslateError::Failed);
    };
    let (tx, rx) = mpsc::channel();
    let ctx = Box::into_raw(Box::new(tx)).cast::<c_void>();
    // SAFETY: 字符串在调用期间有效（Swift 侧同步复制）；ctx 由 on_translated 收回
    unsafe { xp_tr_translate(text.as_ptr(), target.as_ptr(), ctx, on_translated) };
    rx.recv_timeout(TRANSLATE_TIMEOUT)
        .unwrap_or(Err(SysTranslateError::Timeout))
}

extern "C" fn on_prepared(ctx: *mut c_void, code: i32) {
    // SAFETY: ctx 是 translate_prepare 里 Box::into_raw 的发送端，Swift 侧恰好回调一次，这里收回所有权
    let tx = unsafe { Box::from_raw(ctx.cast::<mpsc::Sender<i32>>()) };
    let _ = tx.send(code);
}

/// 弹出系统“下载翻译语言”确认，等用户选择及下载结束（设计 §3.1、§4）。`source` 为空串时同 `translate_pack` 的语言对。
/// `window` 是正在显示的 xPaste 窗口的 `NSWindow` 指针：确认框靠挂进它的 SwiftUI 视图弹出，隐藏窗口弹不出来。
/// 阻塞、不设超时（下载时长由系统决定）；视图在主队列上挂，所以**只在工作线程调用**，且主线程在跑事件循环。
/// 返回下载后的状态：装好了为 `Installed`，取消为 `Download`；确认框弹不出来为 `Err(Failed)`。
///
/// # Safety
/// `window` 为空或是有效的 `NSWindow` 指针，且在本函数返回前窗口不被释放。
pub unsafe fn translate_prepare(
    source: &str,
    target: &str,
    window: *mut c_void,
) -> Result<PackStatus, SysTranslateError> {
    if !translate_supported() {
        return Err(SysTranslateError::OsTooOld);
    }
    let (Ok(source), Ok(target)) = (CString::new(source), CString::new(target)) else {
        return Err(SysTranslateError::Failed);
    };
    let (tx, rx) = mpsc::channel::<i32>();
    let ctx = Box::into_raw(Box::new(tx)).cast::<c_void>();
    // SAFETY: 字符串在调用期间有效（Swift 侧同步复制）；window 见函数的 Safety；ctx 由 on_prepared 收回
    unsafe { xp_tr_prepare(source.as_ptr(), target.as_ptr(), window, ctx, on_prepared) };
    match rx.recv() {
        Ok(0) => Ok(PackStatus::Installed),
        Ok(1) => Ok(PackStatus::Download),
        _ => Err(SysTranslateError::Failed),
    }
}

/// 语言代码按系统语言的本地化名（如 `fr` → “法语”），认不出时为 None。
pub fn language_name(code: &str) -> Option<String> {
    autoreleasepool(|_| {
        NSLocale::currentLocale()
            .localizedStringForLanguageCode(&NSString::from_str(code))
            .map(|s| s.to_string())
    })
}

/// 当前线程降到 utility QoS（后台识别用）。尽力而为。
pub fn lower_thread_priority() {
    // SAFETY: 只改本线程的 QoS
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0) };
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

/// 选择器候选：前台类（Regular）运行中应用的 `localizedName`，加上常见应用目录下一层 `*.app` 的显示名
///（去掉 `.app`）。与 `frontmost_app().name` 同口径；可重复、无序，由前端去重。
pub fn installed_apps() -> Vec<String> {
    const DIRS: [&str; 5] = [
        "/Applications",
        "/Applications/Utilities",
        "/System/Applications",
        "/System/Applications/Utilities",
        "/System/Library/CoreServices/Applications",
    ];
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Applications"));
    let bundles: Vec<PathBuf> = DIRS
        .iter()
        .map(PathBuf::from)
        .chain(home)
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "app"))
        .collect();
    autoreleasepool(|_| {
        let running = NSWorkspace::sharedWorkspace().runningApplications();
        let fm = NSFileManager::defaultManager();
        running
            .iter()
            .filter(|a| a.activationPolicy() == NSApplicationActivationPolicy::Regular)
            .filter_map(|a| a.localizedName().map(|s| s.to_string()))
            .chain(bundles.iter().filter_map(|p| {
                let name = fm
                    .displayNameAtPath(&NSString::from_str(p.to_str()?))
                    .to_string();
                Some(name.strip_suffix(".app").map(str::to_owned).unwrap_or(name))
            }))
            .filter(|n| !n.trim().is_empty())
            .collect()
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

/// 未授权时先请求系统授权对话框（无 TCC 记录时弹出并把本应用加入列表），再打开系统设置的
/// 辅助功能页：拒绝过或在设置里撤销过时对话框通常不再弹出，只靠前者按钮会没反应。
/// 已授权时无操作。须在主线程调用。
pub fn request_accessibility() {
    let trusted = autoreleasepool(|_| {
        // `kAXTrustedCheckOptionPrompt` 的值
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let yes = NSNumber::new_bool(true);
        let options = NSDictionary::from_slices(&[&*key], &[&*yes]);
        // SAFETY: NSDictionary 与 CFDictionaryRef 免费桥接，options 在调用期间存活
        unsafe { AXIsProcessTrustedWithOptions(Retained::as_ptr(&options)) != 0 }
    });
    if !trusted {
        open_accessibility_settings();
    }
}

pub fn open_accessibility_settings() {
    open_privacy_pane("Privacy_Accessibility");
}

/// 打开系统设置“隐私与安全性”下的某一页。
fn open_privacy_pane(pane: &str) {
    autoreleasepool(|_| {
        let url = NSString::from_str(&format!(
            "x-apple.systempreferences:com.apple.preference.security?{pane}"
        ));
        match NSURL::URLWithString(&url) {
            Some(url) if NSWorkspace::sharedWorkspace().openURL(&url) => {}
            _ => log::warn!("打开系统设置 {pane} 失败"),
        }
    })
}

/// macOS 粘贴发给当前前台应用，不需要记录窗口。
pub fn foreground_window() -> Option<isize> {
    None
}

/// Windows 上把窗口切到前台；macOS 没有窗口句柄的概念（`foreground_window` 恒为 `None`），无操作。
pub fn activate_window(_hwnd: isize) {}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::kCGColorSpaceLinearSRGB;

    #[test]
    fn unreadable_file_paths_fall_back_to_bitmap() {
        let png = Flavor {
            item: 0,
            format: "public.png".into(),
            data: vec![1, 2, 3],
        };
        let gone = PathBuf::from("/nonexistent/wechat-temp.jpg");
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert!(!usable_file(
            &home.join("Library/Containers/com.tencent.xinWeChat/Data/a.jpg")
        ));
        assert!(!usable_file(&home.join(
            "Library/Group Containers/5A4RE8SF68.com.tencent.xinWeChat/a.jpg"
        )));
        let file_url = Flavor {
            item: 0,
            format: "public.file-url".into(),
            data: b"file:///nonexistent/wechat-temp.jpg".to_vec(),
        };
        let mut s = Snapshot {
            flavors: vec![png.clone(), file_url],
            file_paths: vec![gone.clone()],
            ..Default::default()
        };
        prefer_bitmap_over_unreadable_files(&mut s);
        assert!(s.file_paths.is_empty());
        assert_eq!(s.bitmap, Some(vec![1, 2, 3]));
        // 失效的文件引用不随记录写回剪切板
        assert_eq!(s.flavors, vec![png.clone()]);
        // 有一个能读就按文件记
        let readable = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let mut s = Snapshot {
            flavors: vec![png],
            file_paths: vec![gone.clone(), readable],
            ..Default::default()
        };
        prefer_bitmap_over_unreadable_files(&mut s);
        assert_eq!(s.file_paths.len(), 2);
        assert!(s.bitmap.is_none());
        // 没有位图时保留路径
        let mut s = Snapshot {
            file_paths: vec![gone],
            ..Default::default()
        };
        prefer_bitmap_over_unreadable_files(&mut s);
        assert_eq!(s.file_paths.len(), 1);
    }

    #[test]
    fn oriented_size_swaps_on_quarter_turn() {
        assert_eq!(oriented_size(1920.0, 1080.0, 1.0, 0.0), (1920, 1080));
        assert_eq!(oriented_size(1920.0, 1080.0, 0.0, 1.0), (1080, 1920)); // 竖拍 90°
        assert_eq!(oriented_size(1920.0, 1080.0, 0.0, -1.0), (1080, 1920)); // 270°
        assert_eq!(oriented_size(1920.0, 1080.0, -1.0, 0.0), (1920, 1080)); // 180°
    }

    #[test]
    fn png_flavors_is_public_png() {
        let f = png_flavors(vec![1, 2, 3]);
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].item, f[0].format.as_str()), (0, "public.png"));
        assert_eq!(f[0].data, [1, 2, 3]);
    }

    #[test]
    fn srgb_rgba_converts_colour_space_and_unpremultiplies() {
        // 线性 sRGB 的 50% 灰转到 sRGB 约为 188；半透明纯红线性与 sRGB 相同，还原预乘后不变
        let src = [128u8, 128, 128, 255, 255, 0, 0, 128];
        let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(&src))).unwrap();
        // SAFETY: AppKit 导出的常量，进程内一直有效
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceLinearSRGB })).unwrap();
        // SAFETY: 尺寸与 RGBA8 缓冲一致；decode 传 null
        let img = unsafe {
            CGImage::new(
                2,
                1,
                8,
                32,
                8,
                Some(&space),
                CGBitmapInfo(CGImageAlphaInfo::Last.0),
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .unwrap();
        let out = srgb_rgba(&img, 2, 1).unwrap();
        assert!((186..=190).contains(&out[0]), "{out:?}");
        assert_eq!((out[0], out[3]), (out[1], 255));
        assert!(
            out[4] >= 253 && out[5] <= 1 && out[6] <= 1 && out[7] == 128,
            "{out:?}"
        );
    }

    #[test]
    fn installed_apps_lists_display_names() {
        let apps = installed_apps();
        assert!(
            apps.iter().any(|n| n == "Safari"),
            "{} 项里没有 Safari",
            apps.len()
        );
        assert!(apps.iter().all(|n| !n.is_empty() && !n.ends_with(".app")));
    }

    #[test]
    fn round_fps_drops_non_finite() {
        assert_eq!(round_fps(29.970_03), 29.97);
        assert_eq!(round_fps(60.0), 60.0);
        assert_eq!(round_fps(f32::NAN), 0.0);
        assert_eq!(round_fps(0.0), 0.0);
    }

    #[test]
    #[ignore = "需要 XPASTE_SAMPLE_VIDEO（见执行说明的 ffmpeg 命令）"]
    fn reads_sample_video() {
        let p = std::path::PathBuf::from(std::env::var("XPASTE_SAMPLE_VIDEO").unwrap());
        let m = video_meta(&p).unwrap();
        assert!((m.duration_sec - 3.0).abs() < 0.1);
        assert_eq!(
            (m.w, m.h, m.fps, m.codec.as_str()),
            (640, 360, 30.0, "H.264")
        );
        let png = video_poster(&p, 320).unwrap();
        let wh = |o: usize| u32::from_be_bytes(png[o..o + 4].try_into().unwrap());
        assert_eq!((wh(16), wh(20)), (320, 180)); // PNG IHDR 宽高
        assert!(video_meta(std::path::Path::new("/definitely/missing.mp4")).is_none());
    }
}
