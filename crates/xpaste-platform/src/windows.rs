use std::cell::OnceCell;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits,
    GetObjectW, HBITMAP, ReleaseDC,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
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
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::Shell::SHDefExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyIcon, DispatchMessageW, GetClassNameW,
    GetForegroundWindow, GetIconInfo, GetMessageW, GetWindowThreadProcessId, HICON, HWND_MESSAGE,
    ICONINFO, MSG, RegisterClassW, SetForegroundWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLIPBOARDUPDATE, WNDCLASSW,
};
use windows::core::{Error, PCWSTR, PWSTR, w};
use xpaste_core::capture::MAX_FLAVOR_BYTES;
use xpaste_core::{AppInfo, Flavor, Snapshot};

use crate::decode::normalize_newlines;
use crate::winfmt::{
    bgra_to_png, bitmap_from_flavors, cf_html_fragment, file_description_query, formats_to_read,
    is_shell_tray_class, parse_hdrop, standard_format_id, standard_format_name, write_bytes,
};
use crate::{PasteBlock, PlatformError};

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
    let mut pid = 0u32;
    // SAFETY: pid 指向本地变量；进程句柄用完即关
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == 0 {
            return None;
        }
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
        let id = String::from_utf16_lossy(&buf[..len as usize]);
        let name = file_description(&id).unwrap_or_else(|| {
            Path::new(&id)
                .file_stem()
                .map_or_else(|| id.clone(), |s| s.to_string_lossy().into_owned())
        });
        Some(AppInfo { id, name })
    }
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
            .and_then(|()| icon_bitmap_to_png(info.hbmColor));
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

/// 图标彩色位图读成自上而下的 32 位 BGRA 再转 PNG。位图由调用方释放。
fn icon_bitmap_to_png(color: HBITMAP) -> Option<Vec<u8>> {
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
    if lines != h as i32 {
        return None;
    }
    bgra_to_png(w, h, &bgra)
}

/// 清空剪切板后写入 `CF_UNICODETEXT`，返回写入后的序号。
pub fn write_text(text: &str) -> Result<u64, PlatformError> {
    let bytes: Vec<u8> = text
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    let guard = ClipboardGuard::open()?;
    // SAFETY: 剪切板已打开
    unsafe { EmptyClipboard() }
        .map_err(|e| PlatformError::Clipboard(format!("清空剪切板失败：{e}")))?;
    set_global(&guard, u32::from(CF_UNICODETEXT.0), &bytes)?;
    drop(guard);
    Ok(change_count())
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

/// 把 `target` 切回前台（最多等 100 ms），再发 Ctrl+V。
pub fn paste(target: Option<isize>) -> Result<(), PlatformError> {
    if let Some(hwnd) = target.map(|h| HWND(h as *mut _)) {
        // SAFETY: 只读查询与前台切换，hwnd 失效时调用失败而不会崩溃
        unsafe {
            let _ = SetForegroundWindow(hwnd);
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

/// 粘贴目标窗口：前台窗口无效、属于本进程或是任务栏 / 托盘溢出窗口时返回 `None`，
/// 应用层随之只复制不粘贴（从托盘打开面板就是这种情况）。
pub fn foreground_window() -> Option<isize> {
    // SAFETY: 只读查询；pid 与类名缓冲区都是本地变量
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_invalid() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid as *mut u32));
        if pid == GetCurrentProcessId() {
            return None;
        }
        let mut class = [0u16; 256];
        let len = GetClassNameW(hwnd, &mut class);
        if len > 0 && is_shell_tray_class(&String::from_utf16_lossy(&class[..len as usize])) {
            return None;
        }
        Some(hwnd.0 as isize)
    }
}
