use std::cell::OnceCell;
use std::path::Path;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, EnumClipboardFormats,
    GetClipboardData, GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_CONTROL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, HWND_MESSAGE, MSG, RegisterClassW, SetForegroundWindow,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WNDCLASSW,
};
use windows::core::{Error, PCWSTR, PWSTR, w};
use xpaste_core::{AppInfo, Flavor, Snapshot};

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

pub fn read_snapshot() -> Result<Option<Snapshot>, PlatformError> {
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
    if sensitive {
        return Ok(Some(Snapshot {
            sensitive: true,
            source,
            ..Default::default()
        }));
    }
    let text = formats
        .contains(&u32::from(CF_UNICODETEXT.0))
        .then(|| {
            with_global(
                &guard,
                u32::from(CF_UNICODETEXT.0),
                crate::decode::utf16le_cstr,
            )
        })
        .flatten();
    drop(guard);
    let mut snapshot = Snapshot {
        source,
        ..Default::default()
    };
    if let Some((text, data)) = text {
        snapshot.flavors.push(Flavor {
            item: 0,
            format: "CF_UNICODETEXT".into(),
            data,
        });
        snapshot.text = Some(text);
    }
    Ok(Some(snapshot))
}

/// 剪切板所有者窗口所属进程；取不到时用前台窗口。
/// ponytail: name 用文件名，FileDescription 留到阶段 6。
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
        let name = Path::new(&id)
            .file_stem()
            .map_or_else(|| id.clone(), |s| s.to_string_lossy().into_owned());
        Some(AppInfo { id, name })
    }
}

/// 多格式写回在阶段 6 实现。
pub fn write(flavors: &[Flavor]) -> Result<u64, PlatformError> {
    let _ = flavors;
    Err(PlatformError::Unsupported)
}

/// 应用图标在阶段 6 实现。
pub fn app_icon(app_id: &str) -> Option<Vec<u8>> {
    let _ = app_id;
    None
}

/// 清空剪切板后写入 `CF_UNICODETEXT`，返回写入后的序号。
pub fn write_text(text: &str) -> Result<u64, PlatformError> {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let bytes = wide.len() * 2;
    let err = |what: &str, e: Error| PlatformError::Clipboard(format!("{what}：{e}"));
    let guard = ClipboardGuard::open()?;
    // SAFETY: 剪切板已打开；内存块大小与拷贝长度一致；SetClipboardData 成功后所有权归系统，失败时自行释放
    unsafe {
        EmptyClipboard().map_err(|e| err("清空剪切板失败", e))?;
        let hglobal = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| err("分配内存失败", e))?;
        let ptr = GlobalLock(hglobal) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(hglobal));
            return Err(PlatformError::Clipboard("锁定内存失败".into()));
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(hglobal);
        if let Err(e) = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(hglobal.0))) {
            let _ = GlobalFree(Some(hglobal));
            return Err(err("写入剪切板失败", e));
        }
    }
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

pub fn open_accessibility_settings() {}

pub fn foreground_window() -> Option<isize> {
    // SAFETY: 只读查询
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(hwnd.0 as isize)
}
