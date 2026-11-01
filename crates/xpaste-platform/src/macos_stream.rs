//! 长截图的区域画面流（M23 规则 13）：ScreenCaptureKit 按选区的物理像素送来 BGRA 帧，
//! 在采样队列上转成 RGBA 放进最新一帧槽位。尺寸、色彩与 `capture_rect` 对同一区域的输出一致。

use std::ffi::c_void;
use std::sync::Arc;

use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::kCGColorSpaceSRGB;
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCFrameStatus, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamFrameInfoStatus,
    SCStreamOutput, SCStreamOutputType,
};

use super::macos_record::{CMSampleBufferGetImageBuffer, content_filter, ns_err, wait_done};
use crate::stream::{Latest, StreamFrame};
use crate::{AreaOptions, PlatformError};

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    // 按 Get 规则返回（不转移所有权）；CFArray<CFDictionary> 与 NSArray<NSDictionary> 免费桥接
    fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: &CMSampleBuffer,
        create_if_necessary: u8,
    ) -> *const NSArray<NSDictionary<NSString, AnyObject>>;
}

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    // 只用这几个，不为它们再开 objc2-core-video
    fn CVPixelBufferLockBaseAddress(pb: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: *const c_void, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddress(pb: *const c_void) -> *const u8;
    fn CVPixelBufferGetBytesPerRow(pb: *const c_void) -> usize;
    fn CVPixelBufferGetWidth(pb: *const c_void) -> usize;
    fn CVPixelBufferGetHeight(pb: *const c_void) -> usize;
}

/// kCVPixelBufferLock_ReadOnly
const LOCK_READ_ONLY: u64 = 1;
/// kCVPixelFormatType_32BGRA（'BGRA'）
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
/// 画面流帧率上限 60 帧/秒（M23 规则 13）
const FPS: i32 = 60;

struct Ivars {
    latest: Arc<Latest>,
}

define_class!(
    // SAFETY: NSObject 没有子类化限制；AreaOutput 不实现 Drop
    #[unsafe(super(NSObject))]
    #[name = "XPasteAreaOutput"]
    #[ivars = Ivars]
    struct AreaOutput;

    unsafe impl NSObjectProtocol for AreaOutput {}

    // SAFETY: 方法签名与 SCStreamOutput 协议一致
    unsafe impl SCStreamOutput for AreaOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind == SCStreamOutputType::Screen
                && let Some(f) = frame(sb)
            {
                self.ivars().latest.push(f);
            }
        }
    }

    // SAFETY: 方法签名与 SCStreamDelegate 协议一致
    unsafe impl SCStreamDelegate for AreaOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            log::warn!("long 画面流：系统停止了采集：{}", ns_err(error));
            self.ivars().latest.fail();
        }
    }
);

impl AreaOutput {
    fn new(latest: Arc<Latest>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { latest });
        // SAFETY: NSObject 的指定初始化方法
        unsafe { msg_send![super(this), init] }
    }
}

/// 取一帧画面。空闲帧（画面没变，状态不是 complete）、没有图像缓冲时为 `None`。
/// 在采样队列上调用，不能 panic（会穿过 Objective-C 栈帧）。
fn frame(sb: &CMSampleBuffer) -> Option<StreamFrame> {
    // SAFETY: sb 在回调期间有效；附件数组按 Get 规则返回，回调期间随 sb 存活；只读查询
    let complete = unsafe {
        let info = CMSampleBufferGetSampleAttachmentsArray(sb, 0)
            .as_ref()?
            .firstObject()?;
        let status = info.objectForKey(SCStreamFrameInfoStatus)?;
        status.downcast_ref::<NSNumber>()?.integerValue() == SCFrameStatus::Complete.0
    };
    if !complete {
        return None;
    }
    // SAFETY: sb 有效；图像缓冲按 Get 规则返回，回调期间随 sb 存活
    let pb = unsafe { CMSampleBufferGetImageBuffer(sb) };
    if pb.is_null() {
        return None;
    }
    // SAFETY: pb 是有效的 CVPixelBuffer（像素格式按配置为 BGRA）；只读锁定后基址在解锁前有效，
    // 读取范围到最后一行的 w × 4 字节为止；锁定成功才解锁
    unsafe {
        if CVPixelBufferLockBaseAddress(pb, LOCK_READ_ONLY) != 0 {
            return None;
        }
        let (w, h, stride) = (
            CVPixelBufferGetWidth(pb),
            CVPixelBufferGetHeight(pb),
            CVPixelBufferGetBytesPerRow(pb),
        );
        let base = CVPixelBufferGetBaseAddress(pb);
        let out = (!base.is_null() && w > 0 && h > 0 && stride >= w * 4).then(|| {
            let src = std::slice::from_raw_parts(base, stride * (h - 1) + w * 4);
            bgra_to_rgba(src, w, h, stride)
        });
        CVPixelBufferUnlockBaseAddress(pb, LOCK_READ_ONLY);
        let (w, h) = (u32::try_from(w).ok()?, u32::try_from(h).ok()?);
        out.map(|rgba| StreamFrame { w, h, rgba })
    }
}

/// BGRA（每行 `stride` 字节，可能有补齐）转成行紧密排列的 RGBA。屏幕画面不透明：alpha 置 255，
/// 预乘与否没有区别，即非预乘。
fn bgra_to_rgba(src: &[u8], w: usize, h: usize, stride: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for row in src.chunks(stride).take(h) {
        for p in row[..w * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    out
}

/// 正在跑的画面流。停止闭包持有它。
struct Running {
    stream: Retained<SCStream>,
    // SCStream 弱引用 delegate：输出对象要活到停止之后
    _output: Retained<AreaOutput>,
    _queue: DispatchRetained<DispatchQueue>,
}

// SAFETY: SCStream 的启停接口可在任意线程调用；AreaOutput 的状态只有 Arc<Latest>（Send + Sync）
unsafe impl Send for Running {}

impl Running {
    /// 停止采集，等 stopCapture 回调（最多 `CALLBACK_WAIT`）。在 Drop 里调用，不能 panic。
    fn stop(self) {
        autoreleasepool(|_| {
            // 流已被系统停掉时这里会报错，忽略
            if let Err(e) = wait_done(|b| {
                // SAFETY: 完成回调由 wait_done 持有到调用结束
                unsafe { self.stream.stopCaptureWithCompletionHandler(Some(b)) }
            }) {
                log::debug!("long 画面流：停止采集返回 {e}");
            }
        });
    }
}

/// 长截图的区域画面流（M23 规则 13）：阻塞到 startCapture 回调；失败时由调用方回退逐帧截屏。
pub(crate) fn start_native_stream(
    opts: &AreaOptions,
    latest: Arc<Latest>,
) -> Result<Box<dyn FnOnce() + Send>, PlatformError> {
    autoreleasepool(|_| start(opts, latest))
}

fn start(
    opts: &AreaOptions,
    latest: Arc<Latest>,
) -> Result<Box<dyn FnOnce() + Send>, PlatformError> {
    let err = |what: String| PlatformError::Capture(what);
    let filter = content_filter(opts.display, opts.exclude_window)?;
    let (x, y, w, h) = opts.rect;
    // 与 capture_rect 同尺寸，否则拼接把帧当成尺寸变了而跳过
    let (pw, ph) = ((w * opts.scale).round(), (h * opts.scale).round());
    if !(pw >= 1.0 && ph >= 1.0) {
        return Err(err("选区为空".into()));
    }
    // SAFETY: 以下都是 macOS 12.3 起就有的属性；kCGColorSpaceSRGB 是 CoreGraphics 导出的常量
    let config = unsafe {
        let config = SCStreamConfiguration::new();
        config.setSourceRect(CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)));
        config.setWidth(pw as usize);
        config.setHeight(ph as usize);
        config.setMinimumFrameInterval(CMTime::new(1, FPS));
        config.setPixelFormat(BGRA);
        // 前端画布与 PNG 都按 sRGB 解读，同 capture_rect 的色彩转换
        config.setColorSpaceName(kCGColorSpaceSRGB);
        config.setShowsCursor(false);
        config
    };
    let output = AreaOutput::new(latest);
    let queue = DispatchQueue::new("app.xpaste.longshot", DispatchQueueAttr::SERIAL);
    // SAFETY: SCStream 弱引用 delegate、强引用输出；output 由 Running 持有到停止之后
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*output)),
        )
    };
    // SAFETY: queue 是串行队列，回调都在其上执行；SCStream 持有队列
    unsafe {
        stream.addStreamOutput_type_sampleHandlerQueue_error(
            ProtocolObject::from_ref(&*output),
            SCStreamOutputType::Screen,
            Some(&queue),
        )
    }
    .map_err(|e| err(format!("添加输出失败：{}", ns_err(&e))))?;
    if let Err(e) = wait_done(|b| {
        // SAFETY: 完成回调由 wait_done 持有到调用结束
        unsafe { stream.startCaptureWithCompletionHandler(Some(b)) }
    }) {
        // 超时后开始可能迟到，流照样采集：发一次停止，不等回调
        // SAFETY: 不带完成回调
        unsafe { stream.stopCaptureWithCompletionHandler(None) };
        return Err(err(format!("开始画面流失败：{e}")));
    }
    let running = Running {
        stream,
        _output: output,
        _queue: queue,
    };
    Ok(Box::new(move || running.stop()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgra_to_rgba_swaps_channels_and_drops_row_padding() {
        // 2×2，每行补齐到 12 字节，最后一行不带补齐
        let src = [
            1, 2, 3, 255, 4, 5, 6, 255, 9, 9, 9, 9, //
            7, 8, 9, 254, 10, 11, 12, 255,
        ];
        assert_eq!(
            bgra_to_rgba(&src, 2, 2, 12),
            [3, 2, 1, 255, 6, 5, 4, 255, 9, 8, 7, 255, 12, 11, 10, 255]
        );
    }
}
