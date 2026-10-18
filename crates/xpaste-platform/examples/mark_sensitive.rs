//! 写入一段带敏感标记的文本，用于验证监听端的 `sensitive=true`。
//! 运行：`cargo run -p xpaste-platform --example mark_sensitive`

const TEXT: &str = "xpaste-sensitive-test";

#[cfg(target_os = "macos")]
fn write() {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString};
    use objc2_foundation::{NSArray, NSData, NSString, ns_string};

    let text = NSString::from_str(TEXT);
    // SAFETY: AppKit 导出的常量
    let text_type = unsafe { NSPasteboardTypeString };
    let plain = NSPasteboardItem::new();
    plain.setString_forType(&text, text_type);
    let marked = NSPasteboardItem::new();
    marked.setString_forType(&text, text_type);
    marked.setData_forType(&NSData::new(), ns_string!("org.nspasteboard.ConcealedType"));

    let pb = NSPasteboard::generalPasteboard();
    pb.clearContents();
    let items = NSArray::from_retained_slice(&[
        ProtocolObject::from_retained(plain),
        ProtocolObject::from_retained(marked),
    ]);
    assert!(pb.writeObjects(&items), "写入剪切板失败");
}

#[cfg(windows)]
fn write() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;
    use windows::core::w;

    /// 复制到可移动全局内存并交给剪切板；成功后内存归系统所有。
    unsafe fn set(format: u32, bytes: &[u8]) {
        unsafe {
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).expect("GlobalAlloc");
            let p = GlobalLock(h) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(h);
            SetClipboardData(format, Some(HANDLE(h.0))).expect("SetClipboardData");
        }
    }

    let wide: Vec<u8> = TEXT
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_ne_bytes)
        .collect();
    // SAFETY: 打开、写入、关闭都在本函数内完成
    unsafe {
        OpenClipboard(None).expect("OpenClipboard");
        EmptyClipboard().expect("EmptyClipboard");
        set(u32::from(CF_UNICODETEXT.0), &wide);
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        set(exclude, &1u32.to_ne_bytes());
        let _ = CloseClipboard();
    }
}

fn main() {
    write();
    println!("已写入带敏感标记的内容");
}
