use std::cell::OnceCell;
use std::path::Path;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EnumClipboardFormats, GetClipboardData,
    GetClipboardOwner, GetClipboardSequenceNumber, OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, HWND_MESSAGE, MSG, RegisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLIPBOARDUPDATE, WNDCLASSW,
};
use windows::core::{Error, PCWSTR, PWSTR, w};
use xpaste_core::{AppInfo, Flavor, Snapshot};

use crate::PlatformError;

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
            && with_global(&guard, history, |b| {
                b.first_chunk::<4>().map(|&v| u32::from_ne_bytes(v))
            })
            .flatten()
                == Some(0));
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
            with_global(&guard, u32::from(CF_UNICODETEXT.0), |b| {
                let wide: Vec<u16> = b
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_ne_bytes(c))
                    .take_while(|&c| c != 0)
                    .collect();
                String::from_utf16_lossy(&wide)
            })
        })
        .flatten();
    drop(guard);
    let mut snapshot = Snapshot {
        source,
        ..Default::default()
    };
    if let Some(text) = text {
        snapshot.flavors.push(Flavor {
            item: 0,
            format: "CF_UNICODETEXT".into(),
            data: text.as_bytes().to_vec(),
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
