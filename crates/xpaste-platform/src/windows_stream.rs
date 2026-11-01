//! 长截图的区域画面流（M23 规则 13）：Windows.Graphics.Capture 截选区所在的整块显示器，
//! 每帧在 GPU 上按选区裁到暂存纹理，读回后 BGRA → RGBA 放进最新一帧槽位。
//! 尺寸、位置与 `capture_rect` 对同一区域的输出一致。
//!
//! 线程：帧池是自由线程的（`CreateFreeThreaded`），`FrameArrived` 在系统线程池上回调，不经消息循环；
//! 回调之间可能并发，裁剪与读回用一把锁串行（D3D11 的立即上下文不是线程安全的）。
//! 停止只关闭会话与帧池，不等回调：回调持有的都是 `Arc`，停止后迟到的一帧写进无人读取的槽位，无害。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{E_BOUNDS, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{IInspectable, Interface, factory};

use super::{ComGuard, monitors, windows_build};
use crate::stream::{Latest, StreamFrame, bgra_to_rgba};
use crate::{AreaOptions, PlatformError};

/// 能关掉系统黄色截屏边框（`IsBorderRequired`）的最低版本；更早的 Windows 10 回退逐帧截屏（M23 规则 13）
const MIN_BUILD: u32 = 20348;
/// 画面流帧率上限 60 帧/秒（M23 规则 13）：TimeSpan 以 100 ns 为单位。只有 Windows 11 24H2 起认这个设置，
/// 更早的系统按屏幕刷新率送帧，槽位只留最新一帧，不积压
const MIN_INTERVAL: TimeSpan = TimeSpan {
    Duration: 10_000_000 / 60,
};

/// 把 WinRT / D3D 的错误记成 HRESULT（不带画面内容）。
fn hr(what: &'static str) -> impl Fn(windows::core::Error) -> PlatformError {
    move |e| PlatformError::Capture(format!("{what}：{:?}", e.code()))
}

/// 裁剪与读回：GPU 上把选区拷进同尺寸的暂存纹理，再映射到内存。
struct Crop {
    ctx: ID3D11DeviceContext,
    staging: ID3D11Texture2D,
    /// 显示器本地物理像素：x, y, w, h
    rect: (u32, u32, u32, u32),
}

impl Crop {
    fn grab(&self, frame: &Direct3D11CaptureFrame) -> windows::core::Result<StreamFrame> {
        let (x, y, w, h) = self.rect;
        // SAFETY: 帧的表面在 frame 存活期间有效；GetInterface 返回带引用计数的纹理
        let tex: ID3D11Texture2D = unsafe {
            frame
                .Surface()?
                .cast::<IDirect3DDxgiInterfaceAccess>()?
                .GetInterface()?
        };
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: desc 是本地变量
        unsafe { tex.GetDesc(&mut desc) };
        // 显示器分辨率中途变了：选区对不上，交给调用方结束画面流
        if x + w > desc.Width || y + h > desc.Height {
            return Err(E_BOUNDS.into());
        }
        let src_box = D3D11_BOX {
            left: x,
            top: y,
            front: 0,
            right: x + w,
            bottom: y + h,
            back: 1,
        };
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: 暂存纹理与源纹理同为 BGRA8、属于同一设备，拷贝区域已核对在源纹理内；调用方持锁，
        // 立即上下文不会并发使用。Map 成功后 pData 指向 h 行、每行 RowPitch 字节，在 Unmap 前读完
        unsafe {
            self.ctx
                .CopySubresourceRegion(&self.staging, 0, 0, 0, 0, &tex, 0, Some(&src_box));
            self.ctx
                .Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let (w, h, stride) = (w as usize, h as usize, m.RowPitch as usize);
            let rgba = (!m.pData.is_null() && stride >= w * 4).then(|| {
                let src =
                    std::slice::from_raw_parts(m.pData as *const u8, stride * (h - 1) + w * 4);
                bgra_to_rgba(src, w, h, stride)
            });
            self.ctx.Unmap(&self.staging, 0);
            rgba.map(|rgba| StreamFrame {
                w: w as u32,
                h: h as u32,
                rgba,
            })
            .ok_or_else(|| E_BOUNDS.into())
        }
    }
}

/// 正在跑的画面流。停止闭包持有它。
struct Running {
    session: GraphicsCaptureSession,
    pool: Direct3D11CaptureFramePool,
    _item: GraphicsCaptureItem,
}

impl Running {
    /// 关闭会话与帧池，不等待。在 Drop 里调用，不能 panic。
    fn stop(self) {
        if let Err(e) = self.session.Close() {
            log::debug!("long 画面流：关闭会话返回 {:?}", e.code());
        }
        if let Err(e) = self.pool.Close() {
            log::debug!("long 画面流：关闭帧池返回 {:?}", e.code());
        }
    }
}

/// 长截图的区域画面流（M23 规则 13）。Windows 10、选区跨屏、或任何一步失败时返回 `Err`，
/// 由调用方回退逐帧截屏。
pub(crate) fn start_native_stream(
    opts: &AreaOptions,
    latest: Arc<Latest>,
) -> Result<Box<dyn FnOnce() + Send>, PlatformError> {
    let build = windows_build();
    if build < MIN_BUILD {
        return Err(PlatformError::Capture(format!(
            "Windows 10 不用画面流（build {build}）"
        )));
    }
    // 先于所有 COM 对象声明；ComGuard 保住进程级 MTA，下面的对象离开本线程后照样可用
    let _com = ComGuard::init().ok_or_else(|| PlatformError::Capture("COM 初始化失败".into()))?;
    let (monitor, r) = monitors()?
        .get(opts.display as usize)
        .copied()
        .ok_or_else(|| PlatformError::Capture("没有这块显示器".into()))?;
    let (mw, mh) = (r.right - r.left, r.bottom - r.top);
    // 选区 = opts.native（虚拟桌面物理像素，即 origin + rect × scale）减去显示器左上角。
    // 取整与 capture_rect 完全相同（as i32），两边输出的宽高、位置逐像素一致
    let n = opts.native;
    let (x, y, w, h) = (
        n.x as i32 - r.left,
        n.y as i32 - r.top,
        n.w as i32,
        n.h as i32,
    );
    if w <= 0 || h <= 0 || x < 0 || y < 0 || x + w > mw || y + h > mh {
        return Err(PlatformError::Capture("选区不在这块显示器内".into()));
    }
    // 四个值都已核对非负
    let rect = (x as u32, y as u32, w as u32, h as u32);

    let mut device: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    // SAFETY: 输出参数指向本地变量；默认适配器，不带软件光栅模块
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )
    }
    .map_err(hr("创建 D3D11 设备失败"))?;
    let (Some(device), Some(ctx)) = (device, ctx) else {
        return Err(PlatformError::Capture("创建 D3D11 设备失败".into()));
    };
    let desc = D3D11_TEXTURE2D_DESC {
        Width: rect.2,
        Height: rect.3,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    // SAFETY: desc、staging 是本地变量；不带初始数据
    unsafe { device.CreateTexture2D(&desc, None, Some(&mut staging)) }
        .map_err(hr("创建暂存纹理失败"))?;
    let staging = staging.ok_or_else(|| PlatformError::Capture("创建暂存纹理失败".into()))?;
    let dxgi: IDXGIDevice = device.cast().map_err(hr("取 DXGI 设备失败"))?;
    // SAFETY: dxgi 是有效的 DXGI 设备；返回的 WinRT 设备自己持有引用
    let d3d: IDirect3DDevice = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }
        .and_then(|d| d.cast())
        .map_err(hr("创建 WinRT 设备失败"))?;

    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
        .map_err(hr("取采集接口失败"))?;
    // SAFETY: monitor 来自刚才的 EnumDisplayMonitors
    let item: GraphicsCaptureItem =
        unsafe { interop.CreateForMonitor(monitor) }.map_err(hr("创建采集项失败"))?;
    // 进程不是 Per-Monitor-v2 DPI 感知时显示器矩形是缩放后的逻辑尺寸，而采集总是物理像素：对不上就回退
    let size = item.Size().map_err(hr("取采集尺寸失败"))?;
    if (size.Width, size.Height) != (mw, mh) {
        return Err(PlatformError::Capture(format!(
            "采集尺寸 {}x{} 与显示器 {mw}x{mh} 不符",
            size.Width, size.Height
        )));
    }
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &d3d,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(hr("创建帧池失败"))?;
    let session = pool
        .CreateCaptureSession(&item)
        .map_err(hr("创建采集会话失败"))?;
    // 关不掉黄色边框就不用画面流（贴边的选区会把黄边截进画面）
    session
        .SetIsBorderRequired(false)
        .map_err(hr("关闭截屏边框失败"))?;
    session
        .SetIsCursorCaptureEnabled(false)
        .map_err(hr("关闭光标采集失败"))?;
    if let Err(e) = session.SetMinUpdateInterval(MIN_INTERVAL) {
        log::debug!("long 画面流：系统不支持帧率上限 {:?}", e.code());
    }

    let crop = Mutex::new(Crop { ctx, staging, rect });
    // 出错后只报一次失败，之后的帧不再取：帧池缓冲满了系统就不再送帧
    let dead = AtomicBool::new(false);
    let l = latest.clone();
    pool.FrameArrived(
        &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |pool, _| {
            // 系统线程池上回调，不能 panic；没有新帧时 TryGetNextFrame 返回错误，直接跳过
            if dead.load(Ordering::Relaxed) {
                return Ok(());
            }
            let Ok(frame) = pool.ok().and_then(|p| p.TryGetNextFrame()) else {
                return Ok(());
            };
            let got = crop
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .grab(&frame);
            // 尽快把缓冲还给帧池
            drop(frame);
            match got {
                Ok(f) => l.push(f),
                Err(e) if !dead.swap(true, Ordering::Relaxed) => {
                    log::warn!("long 画面流：取帧失败 {:?}", e.code());
                    l.fail();
                }
                Err(_) => {}
            }
            Ok(())
        }),
    )
    .map_err(hr("订阅帧失败"))?;
    // 显示器断开等情况系统关闭采集项
    item.Closed(
        &TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
            log::warn!("long 画面流：系统关闭了采集项");
            latest.fail();
            Ok(())
        }),
    )
    .map_err(hr("订阅关闭事件失败"))?;
    session.StartCapture().map_err(hr("开始采集失败"))?;
    log::info!("long 画面流开始 {}x{}", rect.2, rect.3);

    let running = Running {
        session,
        pool,
        _item: item,
    };
    Ok(Box::new(move || {
        // 停止可能在别的线程（Drop）：同样先初始化 COM，且先于 running 的释放
        let _com = ComGuard::init();
        running.stop();
    }))
}
