use std::cell::OnceCell;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, PROPERTYKEY, RECT,
    RPC_E_CHANGED_MODE, SIZE, WPARAM,
};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
    CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors, GdiFlush, GetDC,
    GetDIBits, GetMonitorInfoW, GetObjectW, HBITMAP, HDC, HMONITOR, MONITORINFO, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::EnhancedStorage::{
    PKEY_Media_Duration, PKEY_Video_Compression, PKEY_Video_FourCC, PKEY_Video_FrameHeight,
    PKEY_Video_FrameRate, PKEY_Video_FrameWidth, PKEY_Video_Orientation,
};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantClear, PropVariantToStringAlloc, PropVariantToUInt32,
    PropVariantToUInt64,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoIncrementMTAUsage,
    CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats,
    GetClipboardData, GetClipboardFormatNameW, GetClipboardOwner, GetClipboardSequenceNumber,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, OpenProcess, OpenProcessToken,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};
use windows::Win32::System::Variant::VT_EMPTY;
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_CONTROL, VK_LBUTTON, VK_RBUTTON,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName,
};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, IShellLinkW, SHCreateItemFromParsingName, SHDefExtractIconW,
    SIIGBF_THUMBNAILONLY, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW, EVENT_SYSTEM_FOREGROUND,
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetForegroundWindow, GetIconInfo, GetMessageW,
    GetWindowLongW, GetWindowTextLengthW, GetWindowThreadProcessId, HICON, HWND_MESSAGE, ICONINFO,
    InternalGetWindowText, IsIconic, IsWindow, IsWindowVisible, MONITORINFOF_PRIMARY, MSG,
    RegisterClassW, SetForegroundWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WM_CLIPBOARDUPDATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, Error, HSTRING, Interface, PCWSTR, PWSTR, w};
use xpaste_core::capture::MAX_FLAVOR_BYTES;
use xpaste_core::{AppInfo, Flavor, Snapshot};

use crate::decode::normalize_newlines;
use crate::record::{MicAccess, RecordOptions, RecordOutcome};
use crate::winfmt::{
    Foreground, VideoProps, bgra_to_opaque_rgba, bgra_to_png, bgra_to_png_within,
    bitmap_from_flavors, cf_html_fragment, file_description_query, foreground_target,
    formats_to_read, is_desktop_class, is_shell_tray_class, join_ocr_words, parse_hdrop,
    png_to_dib, standard_format_id, standard_format_name, video_meta_from_props, write_bytes,
};
use crate::{
    Frame, OcrError, OcrText, PIN_TITLE_PREFIX, PackStatus, PasteBlock, PlatformError,
    SysTranslateError, WinRect,
};

pub const OS: &str = "windows";

thread_local! {
    /// 只在监听线程里设置一次，供窗口过程回调。
    static ON_CHANGE: OnceCell<Box<dyn Fn(u64)>> = const { OnceCell::new() };
}

pub fn change_count() -> u64 {
    // SAFETY: 无参数、无副作用
    unsafe { GetClipboardSequenceNumber() as u64 }
}

/// 起常驻线程：message-only 窗口 + `AddClipboardFormatListener`，收到 `WM_CLIPBOARDUPDATE` 时回调。
pub fn watch(on_change: impl Fn(u64) + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("xpaste-clipboard-watch".into())
        .spawn(move || {
            ON_CHANGE.with(|c| {
                let _ = c.set(Box::new(on_change));
            });
            if let Err(e) = listen() {
                log::error!("剪切板监听启动失败：{e}");
            }
        });
    if let Err(e) = spawned {
        log::error!("剪切板监听线程启动失败：{e}");
    }
}

fn listen() -> windows::core::Result<()> {
    let class = w!("XPasteClipboardWatch");
    // SAFETY: 参数都是本函数内的有效值；窗口与消息循环属于当前线程
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(PCWSTR::null())?.into();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(Error::from_thread());
        }
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance),
            None,
        )?;
        AddClipboardFormatListener(hwnd)?;
        // 前台切换钩子借用本线程的消息循环（WINEVENT_OUTOFCONTEXT 的回调由它分发），不另起线程
        if let Some(h) = foreground_window() {
            LAST_EXTERNAL.store(h, Ordering::Relaxed);
        }
        if SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
        .is_invalid()
        {
            log::warn!("前台切换钩子安装失败");
        }
        let mut msg = MSG::default();
        // GetMessageW 出错返回 -1，只有 > 0 才继续
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        let seq = change_count();
        ON_CHANGE.with(|c| {
            if let Some(f) = c.get() {
                f(seq);
            }
        });
        return LRESULT(0);
    }
    // SAFETY: 原样转发给系统默认窗口过程
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// 打开剪切板期间持有；离开作用域时一定 `CloseClipboard`。
struct ClipboardGuard;

impl ClipboardGuard {
    fn open() -> Result<Self, PlatformError> {
        for attempt in 0..5 {
            // SAFETY: 不关联窗口打开剪切板，成功后由 Drop 关闭
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(Self);
            }
            if attempt < 4 {
                thread::sleep(Duration::from_millis(20));
            }
        }
        Err(PlatformError::Clipboard("剪切板被其他程序占用".into()))
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 只有 open 成功才会构造本结构
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// 读取某格式的全局内存块。调用方必须持有 `ClipboardGuard`。
fn with_global<R>(_guard: &ClipboardGuard, format: u32, f: impl FnOnce(&[u8]) -> R) -> Option<R> {
    // SAFETY: 剪切板已打开；锁定期间内存有效，读完立即解锁
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        if handle.is_invalid() {
            return None;
        }
        let hglobal = HGLOBAL(handle.0);
        let ptr = GlobalLock(hglobal) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let out = f(std::slice::from_raw_parts(ptr, GlobalSize(hglobal)));
        // 锁计数归零时也返回 Err（错误码 NO_ERROR），忽略
        let _ = GlobalUnlock(hglobal);
        Some(out)
    }
}

/// `skip_sensitive` 为真时，敏感内容只返回标记、不读取内容；为假时照常读取，仍标记 `sensitive`。
///
/// 读取全部格式（`winfmt::formats_to_read` 筛过的）会触发来源应用的延迟渲染；
/// 卡顿明显时改为白名单（设计 §15），观测点是超过 100 ms 时的 info 日志。
pub fn read_snapshot(skip_sensitive: bool) -> Result<Option<Snapshot>, PlatformError> {
    let started = Instant::now();
    let source = frontmost_app();
    let guard = ClipboardGuard::open()?;
    // SAFETY: 参数为静态宽字符串
    let (exclude, history) = unsafe {
        (
            RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
            RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
        )
    };
    let mut formats = Vec::new();
    let mut format = 0;
    loop {
        // SAFETY: 剪切板已打开
        format = unsafe { EnumClipboardFormats(format) };
        if format == 0 {
            break;
        }
        formats.push(format);
    }
    if formats.is_empty() {
        return Ok(None);
    }
    let sensitive = formats.contains(&exclude)
        || (formats.contains(&history)
            && with_global(&guard, history, crate::decode::dword_is_zero) == Some(true));
    if sensitive && skip_sensitive {
        return Ok(Some(Snapshot {
            sensitive: true,
            source,
            ..Default::default()
        }));
    }
    let named: Vec<(u32, String)> = formats
        .iter()
        .filter_map(|&id| Some((id, format_name(id)?)))
        .collect();
    let mut flavors = Vec::new();
    for (id, name) in formats_to_read(&named) {
        let data = with_global(&guard, id, |b| {
            if b.len() > MAX_FLAVOR_BYTES {
                log::debug!("跳过过大的格式 {name} {} 字节", b.len());
                None
            } else if id == u32::from(CF_UNICODETEXT.0) {
                Some(crate::decode::utf16le_cstr(b).1)
            } else {
                Some(b.to_vec())
            }
        });
        if let Some(Some(data)) = data {
            flavors.push(Flavor {
                item: 0,
                format: name,
                data,
            });
        }
    }
    drop(guard);
    let first = |name: &str| flavors.iter().find(|f| f.format == name);
    let text = first("CF_UNICODETEXT")
        .map(|f| normalize_newlines(&crate::decode::utf16le_cstr(&f.data).0));
    let html = first("HTML Format").and_then(|f| cf_html_fragment(&f.data));
    let mut file_paths = Vec::new();
    for p in first("CF_HDROP")
        .map(|f| parse_hdrop(&f.data))
        .unwrap_or_default()
    {
        if !file_paths.contains(&p) {
            file_paths.push(p);
        }
    }
    let bitmap = file_paths
        .is_empty()
        .then(|| bitmap_from_flavors(&flavors))
        .flatten();
    let ms = started.elapsed().as_millis();
    if ms > 100 {
        let total: usize = flavors.iter().map(|f| f.data.len()).sum();
        log::info!("读取剪切板 formats={} bytes={total} ms={ms}", flavors.len());
    }
    Ok(Some(Snapshot {
        flavors,
        text,
        html,
        file_paths,
        bitmap,
        sensitive,
        source,
        ..Default::default()
    }))
}

/// 格式 id 对应的名字：预定义格式查表，注册格式问系统；私有 / GDI 对象区间取不到名字，返回 `None`。
fn format_name(id: u32) -> Option<String> {
    if let Some(name) = standard_format_name(id) {
        return Some(name.into());
    }
    let mut buf = [0u16; 256];
    // SAFETY: buf 是本地缓冲区，长度随切片传入
    let len = unsafe { GetClipboardFormatNameW(id, &mut buf) };
    (len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// 剪切板所有者窗口所属进程；取不到时用前台窗口。
pub fn frontmost_app() -> Option<AppInfo> {
    // SAFETY: 只读查询
    let owner = unsafe { GetClipboardOwner() }
        .ok()
        .filter(|h| !h.is_invalid());
    owner
        .and_then(app_of_window)
        .or_else(|| app_of_window(unsafe { GetForegroundWindow() }))
}

fn app_of_window(hwnd: HWND) -> Option<AppInfo> {
    if hwnd.is_invalid() {
        return None;
    }
    let id = exe_of_pid(window_pid(hwnd)?)?;
    Some(AppInfo {
        name: app_name(&id),
        id,
    })
}

fn window_pid(hwnd: HWND) -> Option<u32> {
    let mut pid = 0u32;
    // SAFETY: pid 指向本地变量；hwnd 失效时调用失败、pid 保持 0
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32)) };
    (pid != 0).then_some(pid)
}

/// 进程的 exe 完整路径。
fn exe_of_pid(pid: u32) -> Option<String> {
    // SAFETY: 进程句柄用完即关；buf 是本地缓冲区，len 为其容量
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// 应用名称（`frontmost_app().name` 与 `installed_apps` 共用）：`FileDescription`，没有时用不带扩展名的文件名。
fn app_name(exe: &str) -> String {
    file_description(exe).unwrap_or_else(|| {
        Path::new(exe)
            .file_stem()
            .map_or_else(|| exe.to_owned(), |s| s.to_string_lossy().into_owned())
    })
}

/// exe 版本资源里的 `FileDescription`（按第一个语言 / 代码页取）；没有或为空时返回 `None`。
fn file_description(path: &str) -> Option<String> {
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let path = PCWSTR(wide.as_ptr());
    // SAFETY: path 以 NUL 结尾，调用期间有效；VerQueryValueW 返回的指针指向 data 内部，
    // 只在 data 存活期间按其返回的长度读取
    unsafe {
        let size = GetFileVersionInfoSizeW(path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(path, None, size, data.as_mut_ptr().cast()).ok()?;
        let query = |sub: PCWSTR| {
            let mut ptr = std::ptr::null_mut();
            let mut len = 0u32;
            (VerQueryValueW(data.as_ptr().cast(), sub, &mut ptr, &mut len).as_bool()
                && !ptr.is_null())
            .then_some((ptr, len as usize))
        };
        let translation = query(w!(r"\VarFileInfo\Translation")).map_or(&[][..], |(p, len)| {
            std::slice::from_raw_parts(p as *const u8, len)
        });
        let sub: Vec<u16> = file_description_query(translation)
            .encode_utf16()
            .chain([0])
            .collect();
        // 字符串值的长度按 WCHAR 计，可能含结尾 NUL
        let (p, len) = query(PCWSTR(sub.as_ptr()))?;
        let chars = std::slice::from_raw_parts(p as *const u16, len);
        let end = chars.iter().position(|&c| c == 0).unwrap_or(len);
        let name = String::from_utf16_lossy(&chars[..end]).trim().to_owned();
        (!name.is_empty()).then_some(name)
    }
}

/// 清空剪切板后按 `format` 名逐个写回，返回写入后的序号；`item` 在 Windows 上忽略
/// （多文件本来就在一个 `CF_HDROP` 里）。只写 `read_snapshot` 存下的 HGLOBAL 格式，GDI 句柄格式读时已滤掉。
///
/// `OpenClipboard(None)` 后 `EmptyClipboard` 会把剪切板所有者设为空，文档说此时 `SetClipboardData`
/// 可能失败；若 Windows 上往返验收（`阶段6-W1`）失败，改为用监听线程的 message-only 窗口作所有者。
/// 一个格式都没写成时返回错误，但剪切板已经清空（与 macOS 的 `clearContents` 相同），原有内容不会恢复。
pub fn write(flavors: &[Flavor]) -> Result<u64, PlatformError> {
    if flavors.iter().all(|f| f.format.is_empty()) {
        return Err(PlatformError::Clipboard("没有可写入的格式".into()));
    }
    let guard = ClipboardGuard::open()?;
    // SAFETY: 剪切板已打开
    unsafe { EmptyClipboard() }
        .map_err(|e| PlatformError::Clipboard(format!("清空剪切板失败：{e}")))?;
    let mut written = 0;
    for f in flavors.iter().filter(|f| !f.format.is_empty()) {
        let id = standard_format_id(&f.format).unwrap_or_else(|| {
            let wide: Vec<u16> = f.format.encode_utf16().chain([0]).collect();
            // SAFETY: wide 以 NUL 结尾，调用期间有效
            unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
        });
        if id == 0 {
            log::debug!("跳过无法注册的格式 {}", f.format);
            continue;
        }
        match set_global(&guard, id, &write_bytes(id, &f.data)) {
            Ok(()) => written += 1,
            Err(e) => log::debug!("写入格式 {} 失败：{e}", f.format),
        }
    }
    drop(guard);
    if written == 0 {
        return Err(PlatformError::Clipboard("写入剪切板失败".into()));
    }
    Ok(change_count())
}

/// 把 `bytes` 拷进新分配的全局内存块并 `SetClipboardData`。调用方必须持有 `ClipboardGuard` 且已 `EmptyClipboard`。
/// 补 NUL、空数据补 1 字节由调用方经 `winfmt::write_bytes` 处理；这里的 `max(1)` 只是兜底，0 字节的块锁不住。
fn set_global(_guard: &ClipboardGuard, id: u32, bytes: &[u8]) -> Result<(), PlatformError> {
    let err = |what: &str, e: Error| PlatformError::Clipboard(format!("{what}：{e}"));
    // SAFETY: 剪切板已打开；内存块不小于拷贝长度；SetClipboardData 成功后所有权归系统，失败时自行释放
    unsafe {
        let hglobal = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len().max(1))
            .map_err(|e| err("分配内存失败", e))?;
        let ptr = GlobalLock(hglobal) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(PlatformError::Clipboard("锁定内存失败".into()));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(hglobal);
        if let Err(e) = SetClipboardData(id, Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(err("写入剪切板失败", e));
        }
    }
    Ok(())
}

/// `app_id` 是 exe 路径：取 64 px 图标转 PNG；单色图标（无彩色位图）返回 `None`。
/// ponytail: alpha 全零的老图标按不透明处理，有反馈再按 hbmMask 抠图
pub fn app_icon(app_id: &str) -> Option<Vec<u8>> {
    let wide: Vec<u16> = app_id.encode_utf16().chain([0]).collect();
    let mut icon = HICON::default();
    // SAFETY: wide 以 NUL 结尾；icon 指向本地变量，成功后由下面 DestroyIcon 释放
    let hr = unsafe { SHDefExtractIconW(PCWSTR(wide.as_ptr()), 0, 0, Some(&mut icon), None, 64) };
    if hr.is_err() || icon.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    // SAFETY: icon 有效；GetIconInfo 成功时创建的两张位图由本函数 DeleteObject，图标最后 DestroyIcon
    unsafe {
        let png = GetIconInfo(icon, &mut info)
            .ok()
            .and_then(|()| bitmap_bgra(info.hbmColor))
            .and_then(|(w, h, px)| bgra_to_png(w, h, &px));
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        let _ = DestroyIcon(icon);
        png
    }
}

/// 位图读成自上而下的 32 位 BGRA，返回（宽，高，像素）。位图由调用方释放。
fn bitmap_bgra(color: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    if color.is_invalid() {
        return None;
    }
    let mut bm = BITMAP::default();
    // SAFETY: bm 是本地变量，大小随参数传入
    let got = unsafe {
        GetObjectW(
            color.into(),
            size_of::<BITMAP>() as i32,
            Some((&mut bm as *mut BITMAP).cast()),
        )
    };
    let (w, h) = (
        u32::try_from(bm.bmWidth).ok()?,
        u32::try_from(bm.bmHeight).ok()?,
    );
    if got == 0 || w == 0 || h == 0 {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bm.bmWidth,
            biHeight: -bm.bmHeight, // 负高度：自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bgra = vec![0u8; w as usize * h as usize * 4];
    // SAFETY: 屏幕 DC 用完即释放；bgra 正好容纳 h 行 32 位像素（32 位行天然 4 字节对齐）；
    // 32 位 BI_RGB 不写颜色表，BITMAPINFO 里的一项 bmiColors 足够
    let lines = unsafe {
        let dc = GetDC(None);
        let n = GetDIBits(
            dc,
            color,
            0,
            h,
            Some(bgra.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, dc);
        n
    };
    (lines == h as i32).then_some((w, h, bgra))
}

/// 进程是否以提升权限运行；读不到令牌时返回 `None`。
fn is_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    let mut elevation = TOKEN_ELEVATION::default();
    let mut len = 0u32;
    // SAFETY: 输出参数都指向本地变量；令牌句柄用完即关
    unsafe {
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        let _ = CloseHandle(token);
        ok.ok()?;
    }
    Some(elevation.TokenIsElevated != 0)
}

/// 目标窗口进程权限高于本进程时 `Elevated`（读不到目标令牌也按提升处理）。
pub fn can_paste(target: Option<isize>) -> PasteBlock {
    let Some(hwnd) = target.map(|h| HWND(h as *mut _)) else {
        return PasteBlock::None;
    };
    // SAFETY: 伪句柄，无需关闭
    if is_elevated(unsafe { GetCurrentProcess() }) == Some(true) {
        return PasteBlock::None;
    }
    let mut pid = 0u32;
    // SAFETY: pid 指向本地变量；进程句柄用完即关
    let target_elevated = unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(process) if pid != 0 => {
                let e = is_elevated(process);
                let _ = CloseHandle(process);
                e
            }
            Ok(process) => {
                let _ = CloseHandle(process);
                None
            }
            Err(_) => None,
        }
    };
    if target_elevated == Some(false) {
        PasteBlock::None
    } else {
        PasteBlock::Elevated
    }
}

fn key(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                ..Default::default()
            },
        },
    }
}

/// 把窗口切到前台；本进程须是前台进程（系统的前台锁规则），hwnd 失效时什么也不做。
pub fn activate_window(hwnd: isize) {
    // SAFETY: hwnd 失效时调用失败而不会崩溃
    unsafe {
        let _ = SetForegroundWindow(HWND(hwnd as *mut _));
    }
}

/// 把 `target` 切回前台（最多等 100 ms），再发 Ctrl+V。
pub fn paste(target: Option<isize>) -> Result<(), PlatformError> {
    if let Some(h) = target {
        activate_window(h);
        let hwnd = HWND(h as *mut _);
        // SAFETY: 只读查询
        unsafe {
            let mut waited = 0;
            while GetForegroundWindow() != hwnd {
                if waited >= 100 {
                    return Err(PlatformError::Clipboard("前台窗口切换超时".into()));
                }
                thread::sleep(Duration::from_millis(10));
                waited += 10;
            }
        }
    }
    let v = VIRTUAL_KEY(u16::from(b'V'));
    let inputs = [
        key(VK_CONTROL, false),
        key(v, false),
        key(v, true),
        key(VK_CONTROL, true),
    ];
    // SAFETY: inputs 是本地数组，cbsize 为单个 INPUT 大小
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(PlatformError::Clipboard(format!(
            "模拟按键只发出 {sent}/{} 个：{}",
            inputs.len(),
            Error::from_thread()
        )));
    }
    Ok(())
}

pub fn request_accessibility() {}

pub fn open_accessibility_settings() {}

/// 最后一个成为前台的外部窗口（不含本进程与任务栏），0 为还没有。由剪切板监听线程上的前台切换钩子更新。
static LAST_EXTERNAL: AtomicIsize = AtomicIsize::new(0);

/// `EVENT_SYSTEM_FOREGROUND` 回调（`WINEVENT_SKIPOWNPROCESS`：本进程窗口成为前台时不来）。
unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    if let Foreground::Other(h) = classify_foreground(hwnd) {
        LAST_EXTERNAL.store(h, Ordering::Relaxed);
    }
}

fn classify_foreground(hwnd: HWND) -> Foreground {
    if hwnd.is_invalid() {
        return Foreground::None;
    }
    // SAFETY: 只读查询；pid 与类名缓冲区都是本地变量
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == GetCurrentProcessId() {
            return Foreground::Own;
        }
        let mut class = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class);
        if len > 0 && is_shell_tray_class(&String::from_utf16_lossy(&class[..len as usize])) {
            return Foreground::Tray;
        }
    }
    Foreground::Other(hwnd.0 as isize)
}

/// 粘贴目标窗口：前台窗口无效或是任务栏 / 托盘溢出窗口时返回 `None`，应用层随之只复制不粘贴
/// （从托盘打开面板就是这种情况）；本进程窗口（如刚点过的贴图）在前台时取最后一个外部前台窗口（`foreground_target`）。
pub fn foreground_window() -> Option<isize> {
    // SAFETY: 只读查询
    let fg = classify_foreground(unsafe { GetForegroundWindow() });
    let last = LAST_EXTERNAL.load(Ordering::Relaxed);
    // SAFETY: 只读查询；句柄失效时返回 false
    let alive = last != 0 && unsafe { IsWindow(Some(HWND(last as *mut _))) }.as_bool();
    foreground_target(fg, if alive { last } else { 0 })
}

/// 当前线程的 COM 初始化（MTA）。线程已是 STA（`RPC_E_CHANGED_MODE`）时 COM 照样可用，但不归本对象配对；
/// 只有本次 `CoInitializeEx` 成功（含 `S_FALSE`）才在离开作用域时 `CoUninitialize`。
/// 必须先于任何 COM 对象声明，让 COM 对象先释放。
struct ComGuard(bool);

impl ComGuard {
    fn init() -> Option<Self> {
        // 进程级保住 MTA：否则最后一个 MTA 线程 CoUninitialize 时整个 COM 被拆掉，windows crate 缓存的
        // WinRT 工厂（如 OcrEngine）成了悬垂指针，下一次调用访问违例。cookie 故意不归还，随进程结束
        static MTA: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        MTA.get_or_init(|| {
            // SAFETY: 无参数；失败时退回旧行为（只是不保活）
            if let Err(e) = unsafe { CoIncrementMTAUsage() } {
                log::debug!("CoIncrementMTAUsage 失败：{e:?}");
            }
        });
        // SAFETY: 保留参数传 None
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr.is_ok() {
            Some(Self(true))
        } else if hr == RPC_E_CHANGED_MODE {
            Some(Self(false))
        } else {
            log::debug!("COM 初始化失败：{hr:?}");
            None
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: 与本线程上成功的 CoInitializeEx 配对
            unsafe { CoUninitialize() }
        }
    }
}

/// 以 NUL 结尾的宽字符路径（按 OsStr 原样转换，不经 UTF-8）。
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// 读一个属性并交给 `f` 转换；属性不存在或为 `VT_EMPTY` 时返回 `None`。PROPVARIANT 用完即清。
fn prop<T>(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    f: impl FnOnce(&PROPVARIANT) -> Option<T>,
) -> Option<T> {
    // SAFETY: GetValue 成功后 PROPVARIANT 归本函数所有，读完 PropVariantClear；
    // vt 是联合体里所有变体共有的头部字段
    unsafe {
        let mut v = store.GetValue(key).ok()?;
        let out = (v.Anonymous.Anonymous.vt != VT_EMPTY)
            .then(|| f(&v))
            .flatten();
        let _ = PropVariantClear(&mut v);
        out
    }
}

fn prop_u32(store: &IPropertyStore, key: &PROPERTYKEY) -> Option<u32> {
    // SAFETY: v 由 prop 保证有效
    prop(store, key, |v| unsafe { PropVariantToUInt32(v) }.ok())
}

/// 属性库读时长、尺寸、方向、帧率与编码，换算见 `winfmt::video_meta_from_props`。
/// 打不开或没有视频尺寸时返回 None。
pub fn video_meta(path: &Path) -> Option<xpaste_core::VideoMeta> {
    let _com = ComGuard::init()?;
    let wide = wide_path(path);
    // SAFETY: wide 以 NUL 结尾，调用期间有效
    let store: IPropertyStore =
        unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(wide.as_ptr()), None, GPS_DEFAULT) }
            .ok()?;
    // SAFETY: v 由 prop 保证有效；PropVariantToStringAlloc 分配的字符串读完 CoTaskMemFree
    let compression = prop(&store, &PKEY_Video_Compression, |v| unsafe {
        let p = PropVariantToStringAlloc(v).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    });
    video_meta_from_props(&VideoProps {
        // SAFETY: v 由 prop 保证有效
        duration_100ns: prop(&store, &PKEY_Media_Duration, |v| {
            unsafe { PropVariantToUInt64(v) }.ok()
        }),
        w: prop_u32(&store, &PKEY_Video_FrameWidth),
        h: prop_u32(&store, &PKEY_Video_FrameHeight),
        frame_rate_milli: prop_u32(&store, &PKEY_Video_FrameRate),
        orientation: prop_u32(&store, &PKEY_Video_Orientation),
        compression: compression.as_deref(),
        fourcc: prop_u32(&store, &PKEY_Video_FourCC),
    })
}

/// 封面：系统缩略图（`SIIGBF_THUMBNAILONLY`，取帧位置由系统决定），长边不超过 `max_px`，PNG。
/// 没有缩略图处理程序的格式（系统不认识的编码）返回 None。
pub fn video_poster(path: &Path, max_px: u32) -> Option<Vec<u8>> {
    let _com = ComGuard::init()?;
    let wide = wide_path(path);
    let side = i32::try_from(max_px).ok()?;
    // SAFETY: wide 以 NUL 结尾，调用期间有效
    let factory: IShellItemImageFactory =
        unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;
    // SAFETY: GetImage 成功返回的位图归本函数所有，读完 DeleteObject
    unsafe {
        let bitmap = factory
            .GetImage(SIZE { cx: side, cy: side }, SIIGBF_THUMBNAILONLY)
            .ok()?;
        let png = bitmap_bgra(bitmap).and_then(|(w, h, px)| bgra_to_png_within(w, h, &px, max_px));
        let _ = DeleteObject(bitmap.into());
        png
    }
}

/// 依次试用户语言、简体中文、美式英语；系统没有对应 OCR 语言包时 None。
fn ocr_engine() -> Option<OcrEngine> {
    OcrEngine::TryCreateFromUserProfileLanguages()
        .ok()
        .or_else(|| {
            ["zh-Hans-CN", "en-US"].into_iter().find_map(|tag| {
                let lang = Language::CreateLanguage(&HSTRING::from(tag)).ok()?;
                OcrEngine::TryCreateFromLanguage(&lang).ok()
            })
        })
}

pub fn ocr_available() -> bool {
    let _com = ComGuard::init();
    ocr_engine().is_some()
}

/// `Windows.Media.Ocr`。阻塞（`RecognizeAsync` 在本线程 `join`），只在工作线程调用。
/// WinRT 不给行置信度，固定 1.0。行由 `Words()` 按 `join_ocr_words` 拼接：`Text()` 在中文词之间也插空格。
/// `png` 为图片文件字节（按内容识别格式）；长边另受引擎上限 `MaxImageDimension` 限制。
pub fn ocr_png_bytes(png: &[u8], max_side_px: u32) -> Result<Option<OcrText>, OcrError> {
    // COM 失败时后续 WinRT 调用自会报错，这里不单独处理
    let _com = ComGuard::init();
    let max = OcrEngine::MaxImageDimension().map_or(max_side_px, |m| m.min(max_side_px));
    let mut img = crate::load_rgba(png, max)?;
    let Some(engine) = ocr_engine() else {
        return Ok(None);
    };
    let err = |e: Error| OcrError::Engine(e.message());
    let (w, h) = img.dimensions();
    for px in img.pixels_mut() {
        px.0.swap(0, 2); // RGBA → BGRA
    }
    let writer = DataWriter::new().map_err(err)?;
    writer.WriteBytes(img.as_raw()).map_err(err)?;
    let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
        &writer.DetachBuffer().map_err(err)?,
        BitmapPixelFormat::Bgra8,
        w as i32,
        h as i32,
    )
    .map_err(err)?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .map_err(err)?
        .join()
        .map_err(err)?;
    let lines = result
        .Lines()
        .map_err(err)?
        .into_iter()
        .map(|l| {
            let words = l
                .Words()?
                .into_iter()
                .map(|w| w.Text().map(|t| t.to_string()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((join_ocr_words(&words), 1.0))
        })
        .collect::<Result<Vec<_>, Error>>()
        .map_err(err)?;
    Ok(Some(crate::ocr_text(lines)))
}

/// 系统翻译只在 macOS 26 及以上提供（M26 规则 7）。
pub fn translate_supported() -> bool {
    false
}

pub fn translate_pack(_target: &str) -> PackStatus {
    PackStatus::Unsupported
}

pub fn translate_text(_text: &str, _target: &str) -> Result<String, SysTranslateError> {
    Err(SysTranslateError::Unsupported)
}

/// 系统翻译只在 macOS 上有。
///
/// # Safety
/// 同 macOS：`window` 为空或是有效的窗口指针（这里不用它）。
pub unsafe fn translate_prepare(
    _source: &str,
    _target: &str,
    _window: *mut std::ffi::c_void,
) -> Result<PackStatus, SysTranslateError> {
    Err(SysTranslateError::Unsupported)
}

pub fn language_name(_code: &str) -> Option<String> {
    None
}

/// 当前线程降到低于正常优先级（后台识别用）。尽力而为。
pub fn lower_thread_priority() {
    // SAFETY: 当前线程伪句柄，只改本线程
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) };
}

/// 选择器候选：有可见（未被 DWM 隐藏）、带标题顶层窗口的进程（本进程除外），加上开始菜单快捷方式指向的 exe，
/// 名称与 `frontmost_app().name` 同口径。可重复、无序，由前端去重。
pub fn installed_apps() -> Vec<String> {
    let mut pids: Vec<u32> = Vec::new();
    // SAFETY: 回调只在 EnumWindows 返回前经 lparam 访问 pids
    let _ = unsafe {
        EnumWindows(
            Some(push_window_pid),
            LPARAM(&mut pids as *mut Vec<u32> as isize),
        )
    };
    pids.sort_unstable();
    pids.dedup();
    let linked = ComGuard::init().map_or_else(Vec::new, |_com| start_menu_targets());
    // 同一个 exe 只读一次版本资源
    let mut exes: Vec<String> = pids
        .into_iter()
        .filter_map(exe_of_pid)
        .chain(linked)
        .collect();
    exes.sort_unstable();
    exes.dedup();
    exes.iter()
        .map(|exe| app_name(exe))
        .filter(|n| !n.trim().is_empty())
        .collect()
}

extern "system" fn push_window_pid(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 是 installed_apps 传入的 &mut Vec<u32>，EnumWindows 返回前一直有效
    unsafe {
        // 先排除本进程：对本进程 UI 线程的窗口取标题会跨线程发 WM_GETTEXTLENGTH，没有超时；
        // 再排除被 DWM 隐藏（cloaked）的窗口，如其他虚拟桌面上的、挂起的 UWP 窗口
        if IsWindowVisible(hwnd).as_bool()
            && let Some(pid) = window_pid(hwnd)
            && pid != GetCurrentProcessId()
            && !is_cloaked(hwnd)
            && GetWindowTextLengthW(hwnd) > 0
        {
            (*(lparam.0 as *mut Vec<u32>)).push(pid);
        }
    }
    true.into()
}

/// 被 DWM 隐藏（cloaked）：其他虚拟桌面上的、挂起的 UWP 窗口等。
fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: cloaked 是本地变量，大小随参数传入
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
}

/// 截图写剪切板用的格式：`PNG` 加 `CF_DIB`（不认 PNG 的老程序也能粘贴）。
pub fn png_flavors(png: Vec<u8>) -> Vec<Flavor> {
    let flavor = |format: &str, data| Flavor {
        item: 0,
        format: format.into(),
        data,
    };
    let dib = png_to_dib(&png);
    let mut out = vec![flavor("PNG", png)];
    out.extend(dib.map(|d| flavor("CF_DIB", d)));
    out
}

/// 逐块显示器截整屏（主屏在前）。坐标为虚拟桌面物理像素，`scale` 为 DPI / 96。阻塞。
///
/// 前提：进程是 Per-Monitor-v2 DPI 感知（tao 在事件循环建立后设置）。否则显示器矩形与 `BitBlt`
/// 按缩放后的逻辑坐标计算，而 `window_rects` 的 DWM 边框仍是物理像素，两者对不上。
pub fn freeze_displays() -> Result<Vec<Frame>, PlatformError> {
    let mut monitors: Vec<(HMONITOR, RECT)> = Vec::new();
    // SAFETY: 回调只在 EnumDisplayMonitors 返回前经 lparam 访问 monitors
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(push_monitor),
            LPARAM(&mut monitors as *mut Vec<(HMONITOR, RECT)> as isize),
        )
    }
    .ok()
    .map_err(|e| PlatformError::Capture(format!("枚举显示器失败：{e}")))?;
    monitors.sort_by_key(|&(m, _)| !is_primary(m));
    // SAFETY: 屏幕 DC 用完即释放
    let screen = unsafe { GetDC(None) };
    if screen.is_invalid() {
        return Err(PlatformError::Capture("取屏幕 DC 失败".into()));
    }
    let frames = monitors
        .iter()
        .zip(0u32..)
        .map(|(&(m, r), id)| freeze_monitor(screen, id, m, r))
        .collect();
    // SAFETY: 上面取得的屏幕 DC
    unsafe { ReleaseDC(None, screen) };
    frames
}

extern "system" fn push_monitor(m: HMONITOR, _dc: HDC, rect: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam 是 freeze_displays 传入的 &mut Vec，rect 由系统给出，都在回调期间有效
    unsafe { (*(lparam.0 as *mut Vec<(HMONITOR, RECT)>)).push((m, *rect)) };
    true.into()
}

fn is_primary(m: HMONITOR) -> bool {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: info 是本地变量，cbSize 已填
    unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() && info.dwFlags & MONITORINFOF_PRIMARY != 0
}

/// 一块屏 `BitBlt` 到自上而下的 32 位 DIB section，再转 RGBA。
/// `CAPTUREBLT` 把分层窗口（贴图）一起截进来。
fn freeze_monitor(screen: HDC, id: u32, m: HMONITOR, r: RECT) -> Result<Frame, PlatformError> {
    let err = |what: &str| PlatformError::Capture(what.into());
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let (pw, ph) = match (u32::try_from(w), u32::try_from(h)) {
        (Ok(pw), Ok(ph)) if pw > 0 && ph > 0 => (pw, ph),
        _ => return Err(err("显示器尺寸无效")),
    };
    let (mut dpi, mut dpi_y) = (96u32, 96u32);
    // SAFETY: 输出参数指向本地变量；失败时保持 96（100%）
    let _ = unsafe { GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y) };
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w,
            biHeight: -h, // 负高度：自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 内存 DC、DIB section 都在本块内创建并释放；BitBlt 完成并 GdiFlush 后 bits 指向
    // pw * ph * 4 字节的像素，在 DeleteObject 之前读完
    let rgba = unsafe {
        let mem = CreateCompatibleDC(Some(screen));
        if mem.is_invalid() {
            return Err(err("创建内存 DC 失败"));
        }
        let rgba = match CreateDIBSection(Some(screen), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(bmp) => {
                let old = SelectObject(mem, bmp.into());
                let blt = BitBlt(
                    mem,
                    0,
                    0,
                    w,
                    h,
                    Some(screen),
                    r.left,
                    r.top,
                    SRCCOPY | CAPTUREBLT,
                );
                let _ = GdiFlush();
                let rgba = blt.ok().and_then(|()| {
                    let len = pw as usize * ph as usize * 4;
                    let px = std::slice::from_raw_parts(bits as *const u8, len);
                    bgra_to_opaque_rgba(px, pw, ph)
                });
                SelectObject(mem, old);
                let _ = DeleteObject(bmp.into());
                rgba
            }
            Err(_) => None,
        };
        let _ = DeleteDC(mem);
        rgba
    }
    .ok_or_else(|| err("截屏失败"))?;
    Ok(Frame {
        id,
        origin: (f64::from(r.left), f64::from(r.top)),
        size: (f64::from(w), f64::from(h)),
        scale: f64::from(dpi) / 96.0,
        px: (pw, ph),
        rgba,
    })
}

/// 屏上窗口外框（DWM 扩展边框，不含阴影），由前到后：别的进程取可见、未最小化、非 cloaked、非工具窗口、非桌面、非鼠标穿透的分层窗口；
/// 本进程只取贴图窗口。前提同 `freeze_displays`（Per-Monitor-v2 DPI 感知）。
pub fn window_rects() -> Vec<WinRect> {
    let mut out: Vec<WinRect> = Vec::new();
    // SAFETY: 回调只在 EnumWindows 返回前经 lparam 访问 out
    let _ = unsafe {
        EnumWindows(
            Some(push_window_rect),
            LPARAM(&mut out as *mut Vec<WinRect> as isize),
        )
    };
    out
}

extern "system" fn push_window_rect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    if let Some(r) = window_rect(hwnd) {
        // SAFETY: lparam 是 window_rects 传入的 &mut Vec，EnumWindows 返回前一直有效
        unsafe { (*(lparam.0 as *mut Vec<WinRect>)).push(r) };
    }
    true.into()
}

fn window_rect(hwnd: HWND) -> Option<WinRect> {
    // SAFETY: 只读查询；buf、rect 是本地变量，大小随参数传入
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return None;
        }
        if window_pid(hwnd)? == GetCurrentProcessId() {
            // InternalGetWindowText 直接读标题，不向本进程 UI 线程发 WM_GETTEXT
            let mut buf = [0u16; 32];
            let n = InternalGetWindowText(hwnd, &mut buf).max(0) as usize;
            if !String::from_utf16_lossy(&buf[..n]).starts_with(PIN_TITLE_PREFIX) {
                return None;
            }
        } else {
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            // 鼠标穿透的分层窗口（屏幕标注、悬浮层等）对应 macOS 的 alpha 为 0：看不见，不吸附
            let see_through = WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0;
            let mut class = [0u16; 256];
            // GetClassNameW 只读窗口类，不向窗口发消息
            let len = GetClassNameW(hwnd, &mut class).max(0) as usize;
            if ex & WS_EX_TOOLWINDOW.0 != 0
                || ex & see_through == see_through
                || is_desktop_class(&String::from_utf16_lossy(&class[..len]))
                || is_cloaked(hwnd)
            {
                return None;
            }
        }
        let mut rect = RECT::default();
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
        .ok()?;
        let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
        (w > 0 && h > 0).then(|| WinRect {
            x: f64::from(rect.left),
            y: f64::from(rect.top),
            w: f64::from(w),
            h: f64::from(h),
        })
    }
}

/// Windows 截屏不需要授权。
pub fn screen_capture_granted() -> bool {
    true
}

pub fn request_screen_capture() {}

/// 不是 macOS。
pub fn macos_version() -> Option<u32> {
    None
}

/// 录屏（M24）只做了 macOS；以下是同名桩，让应用层两端都能编译。
pub struct Recorder;

impl Recorder {
    pub fn failed(&self) -> bool {
        false
    }

    pub fn stop(self) -> Result<RecordOutcome, PlatformError> {
        Err(PlatformError::Unsupported)
    }
}

pub fn start_recording(_opts: RecordOptions) -> Result<Recorder, PlatformError> {
    Err(PlatformError::Unsupported)
}

pub fn microphone_access() -> MicAccess {
    MicAccess::Denied
}

pub fn request_microphone() -> bool {
    false
}

pub fn window_number(_ns_window: *mut std::ffi::c_void) -> Option<u32> {
    None
}

pub fn file_flavors(_path: &Path) -> Vec<Flavor> {
    Vec::new()
}

/// 鼠标主键此刻是否按着。贴图据此判断拖动是否结束。
pub fn mouse_button_down() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    // GetAsyncKeyState 查的是物理键：左右键互换时主键是物理右键
    // SAFETY: 纯查询；最高位为 1 表示按着
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
            VK_RBUTTON
        } else {
            VK_LBUTTON
        };
        GetAsyncKeyState(vk.0 as i32) < 0
    }
}

/// 开始菜单（全体用户与当前用户）里 `.lnk` 指向的、存在的 `.exe`。调用方已初始化 COM。
fn start_menu_targets() -> Vec<String> {
    let mut links = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Some(root) = std::env::var_os(var) {
            collect_links(
                &PathBuf::from(root).join(r"Microsoft\Windows\Start Menu\Programs"),
                &mut links,
            );
        }
    }
    links.iter().filter_map(|l| link_target(l)).collect()
}

/// 递归收集 `.lnk`；不跟随符号链接与联接点，避免循环。
fn collect_links(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            collect_links(&path, out);
        } else if path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("lnk"))
        {
            out.push(path);
        }
    }
}

/// 快捷方式的目标；不是存在的 `.exe`（帮助文档、网址、安装程序公告的快捷方式等）时返回 None。
fn link_target(lnk: &Path) -> Option<String> {
    let wide = wide_path(lnk);
    let mut buf = [0u16; 1024];
    // SAFETY: wide 以 NUL 结尾、buf 是本地缓冲区，调用期间有效；不要 WIN32_FIND_DATAW，传 null；
    // 两个 COM 对象在本函数返回前释放
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>()
            .ok()?
            .Load(PCWSTR(wide.as_ptr()), STGM_READ)
            .ok()?;
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).ok()?;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let target = String::from_utf16_lossy(&buf[..end]);
    let path = Path::new(&target);
    (path
        .extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("exe"))
        && path.is_file())
    .then_some(target)
}
