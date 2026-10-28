//! 录屏（M24）：ScreenCaptureKit 的采样缓冲直接交给 AVAssetWriter 写 H.264 MP4。
//!
//! 线程：`start_recording` / `Recorder::stop` 都阻塞等系统回调，不能在主线程调用（回调不经主线程，
//! 但会让界面卡住）。采样回调都在一个串行队列上；回调与 `stop` 通过 `State` 的锁互斥。

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError, mpsc};
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVAuthorizationStatus, AVCaptureDevice,
    AVFileTypeMPEG4, AVMediaTypeAudio, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264,
    AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_media::{CMClock, CMSampleBuffer, CMSampleTimingInfo, CMTime, kCMTimeInvalid};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString, NSURL, ns_string};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamOutput, SCStreamOutputType,
};
use xpaste_core::Flavor;

use crate::PlatformError;
use crate::record::{MicAccess, RecordOptions, RecordOutcome, output_size};

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    // objc2-core-media 的 image_buffer() 要再开 objc2-core-video；这里只判空，手写声明即可
    fn CMSampleBufferGetImageBuffer(sbuf: &CMSampleBuffer) -> *const c_void;
}

/// kAudioFormatMPEG4AAC（'aac '）
const AAC: u32 = u32::from_be_bytes(*b"aac ");
const AUDIO_RATE: i32 = 48_000;
const AUDIO_CHANNELS: i32 = 2;
const AUDIO_BITRATE: i32 = 128_000;
/// 等 ScreenCaptureKit 完成回调的上限：回调不来时不让应用卡在开始或停止里（M24 规则 8）
const CALLBACK_WAIT: Duration = Duration::from_secs(10);
/// 等写完文件的上限；正常不到一秒，留足慢盘的余量
const FINISH_WAIT: Duration = Duration::from_secs(30);

fn record_err(what: impl Into<String>) -> PlatformError {
    PlatformError::Record(what.into())
}

/// 错误只记域和码：AVFoundation 的描述里可能带文件名
fn ns_err(e: &NSError) -> String {
    format!("{} {}", e.domain(), e.code())
}

/// 调用带 `NSError *` 完成回调的接口并阻塞到回调，最多 `CALLBACK_WAIT`。
/// 超时后回调仍可能到来：系统复制并持有 block，里面的发送端随之存活，接收端已丢弃时发送只是返回错误。
fn wait_done(call: impl FnOnce(&DynBlock<dyn Fn(*mut NSError)>)) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |e: *mut NSError| {
        // SAFETY: 回调期间 e 为空或指向有效的 NSError
        let _ = tx.send(unsafe { e.as_ref() }.map(ns_err));
    });
    call(&block);
    match rx.recv_timeout(CALLBACK_WAIT) {
        Ok(None) => Ok(()),
        Ok(Some(e)) => Err(e),
        Err(_) => {
            log::warn!("录屏：等系统回调超时");
            Err("系统未回调".into())
        }
    }
}

/// 写入器的输入与进度。回调线程与 `stop` 共用，都在锁内访问。
struct State {
    writer: Retained<AVAssetWriter>,
    video: Retained<AVAssetWriterInput>,
    audio: Option<Retained<AVAssetWriterInput>>,
    mic: Option<Retained<AVAssetWriterInput>>,
    /// 首帧 PTS；有值即已 startWriting + startSession
    first: Option<CMTime>,
    /// 最后写入的视频帧及其 PTS
    last: Option<(CFRetained<CMSampleBuffer>, CMTime)>,
    /// `stop` 已接管，之后到达的样本一律丢掉
    closed: bool,
}

struct Ivars {
    failed: AtomicBool,
    state: Mutex<State>,
}

define_class!(
    // SAFETY: NSObject 没有子类化限制；Output 不实现 Drop
    #[unsafe(super(NSObject))]
    #[name = "XPasteRecordOutput"]
    #[ivars = Ivars]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    // SAFETY: 方法签名与 SCStreamOutput 协议一致
    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
            self.on_sample(sb, kind);
        }
    }

    // SAFETY: 方法签名与 SCStreamDelegate 协议一致
    unsafe impl SCStreamDelegate for Output {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, error: &NSError) {
            log::warn!("录屏：系统停止了采集：{}", ns_err(error));
            self.ivars().failed.store(true, Ordering::Relaxed);
        }
    }
);

impl Output {
    fn new(ivars: Ivars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: NSObject 的指定初始化方法
        unsafe { msg_send![super(this), init] }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.ivars()
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// 在采样队列上调用。这里不能 panic（会穿过 Objective-C 栈帧）。
    fn on_sample(&self, sb: &CMSampleBuffer, kind: SCStreamOutputType) {
        let mut st = self.state();
        if st.closed {
            return;
        }
        // SAFETY: sb 在回调期间有效，以下都是只读查询
        let (pts, has_image) = unsafe {
            (
                sb.presentation_time_stamp(),
                !CMSampleBufferGetImageBuffer(sb).is_null(),
            )
        };
        if kind == SCStreamOutputType::Screen {
            // 画面没变时 SCK 发不带图像的空闲帧，写进去会让写入器失败
            if !has_image {
                return;
            }
            if st.first.is_none() {
                // startWriting 失败后写入器停在 Failed，再调会抛 Objective-C 异常（穿过回调即中止进程）：
                // 只在 Unknown 时调用，失败后的样本都丢掉（failed 已置）
                // SAFETY: 只读属性
                if unsafe { st.writer.status() } != AVAssetWriterStatus::Unknown {
                    return;
                }
                // SAFETY: 写入器状态为 Unknown，输入都已添加
                if !unsafe { st.writer.startWriting() } {
                    self.writer_failed(&st);
                    return;
                }
                // SAFETY: 刚 startWriting 成功，会话只开一次
                unsafe { st.writer.startSessionAtSourceTime(pts) };
                st.first = Some(pts);
            }
            if append(&st, &st.video, sb) {
                // SAFETY: sb 有效，引用计数加一后由 State 持有
                let kept = unsafe { CFRetained::retain(NonNull::from(sb)) };
                st.last = Some((kept, pts));
                return;
            }
            // 如磁盘写满：录制已无意义，让应用层尽快停止
            // SAFETY: 只读属性
            if unsafe { st.writer.status() } == AVAssetWriterStatus::Failed {
                self.writer_failed(&st);
            }
            return;
        }
        // 会话开始前的声音没有对应画面，丢掉
        if st.first.is_none() {
            return;
        }
        let input = match kind {
            SCStreamOutputType::Audio => st.audio.as_ref(),
            SCStreamOutputType::Microphone => st.mic.as_ref(),
            _ => None,
        };
        if let Some(input) = input {
            append(&st, input, sb);
        }
    }

    /// 置 `failed`；只在第一次记日志。
    fn writer_failed(&self, st: &State) {
        if self.ivars().failed.swap(true, Ordering::Relaxed) {
            return;
        }
        // SAFETY: 只读属性
        let e = unsafe { st.writer.error() };
        log::warn!(
            "录屏：写入失败：{}",
            e.as_deref().map_or_else(|| "未知".into(), ns_err)
        );
    }
}

/// 写入器在写、输入能收时追加；状态不对时追加会抛 Objective-C 异常，先查。
fn append(st: &State, input: &AVAssetWriterInput, sb: &CMSampleBuffer) -> bool {
    // SAFETY: 只读查询；追加前已确认写入器在写、输入未 markAsFinished（closed 之前不会调用）
    unsafe {
        st.writer.status() == AVAssetWriterStatus::Writing
            && input.isReadyForMoreMediaData()
            && input.appendSampleBuffer(sb)
    }
}

/// 正在进行的录制。`stop` 消耗它；不调 `stop` 直接丢弃时流随 SCStream 释放而停，文件不完整。
pub struct Recorder {
    stream: Retained<SCStream>,
    output: Retained<Output>,
    queue: DispatchRetained<DispatchQueue>,
    path: PathBuf,
}

// SAFETY: SCStream 的启停接口可在任意线程调用；Output 的可变状态都在 Mutex / 原子量后面，
// 写入器只在锁内访问。应用层把 Recorder 放进 Mutex，在别的线程上 stop。
unsafe impl Send for Recorder {}

impl Recorder {
    /// 系统停掉了流（如显示器断开、权限被收回）或写入器出错：录制已无法继续，应尽快 `stop`。
    pub fn failed(&self) -> bool {
        self.output.ivars().failed.load(Ordering::Relaxed)
    }

    /// 停止采集并写完文件。阻塞到 finishWriting 回调。一帧画面都没收到时返回 `Empty`，不留文件。
    pub fn stop(self) -> Result<RecordOutcome, PlatformError> {
        // 流已被系统停掉时这里会报错，照样收尾
        if let Err(e) = wait_done(|b| {
            // SAFETY: 完成回调由 wait_done 持有到调用结束
            unsafe { self.stream.stopCaptureWithCompletionHandler(Some(b)) }
        }) {
            log::debug!("录屏：停止采集返回 {e}");
        }
        // 排空队列里已派发的回调；之后到达的样本因 closed 被丢掉
        self.queue.exec_sync(|| ());
        let mut st = self.output.state();
        st.closed = true;
        autoreleasepool(|_| finish(&mut st, &self.path))
    }
}

fn finish(st: &mut State, path: &Path) -> Result<RecordOutcome, PlatformError> {
    let Some(first) = st.first else {
        // 没开始写（startWriting 在首帧才调用），磁盘上没有我们写的文件。startWriting 失败（如文件夹
        // 不可写、目标已存在）是保存失败而不是录得太短；路径上原有的文件不删
        // SAFETY: 只读属性
        if unsafe { st.writer.status() } == AVAssetWriterStatus::Failed {
            // SAFETY: 只读属性
            let e = unsafe { st.writer.error() };
            return Err(record_err(format!(
                "无法开始写入：{}",
                e.as_deref().map_or_else(|| "未知".into(), ns_err)
            )));
        }
        return Ok(RecordOutcome::Empty);
    };
    // 画面静止时只有空闲帧，视频会停在最后一次变化处：把最后一帧按此刻再写一次，延续到停止时刻
    if let Some((last, at)) = st.last.take() {
        // SAFETY: 主机时钟是 SCK 采样 PTS 用的时钟；纯查询
        let now = unsafe { CMClock::host_time_clock().time() };
        // SAFETY: 两个 CMTime 都有效
        let later = unsafe { now.compare(at) } > 0;
        st.last = Some(match later.then(|| retimed(&last, now)).flatten() {
            Some(copy) if append(st, &st.video, &copy) => (copy, now),
            _ => (last, at),
        });
    }
    // 在补帧之后查：补帧那次追加也可能让写入器变成 Failed，对它 markAsFinished / finishWriting 会抛异常
    // SAFETY: 只读属性
    if unsafe { st.writer.status() } != AVAssetWriterStatus::Writing {
        discard(st, path);
        return Err(record_err("写入器已出错"));
    }
    let end = st.last.as_ref().map_or(first, |l| l.1);
    // SAFETY: 写入器在写；markAsFinished 之后不再追加（closed 已置）
    unsafe {
        st.video.markAsFinished();
        for input in [&st.audio, &st.mic].into_iter().flatten() {
            input.markAsFinished();
        }
    }
    let writer = st.writer.clone();
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move || {
        let _ = tx.send(());
    });
    // SAFETY: 写入器在写，所有输入已 markAsFinished；系统复制并持有 block，超时后回调照样安全
    unsafe { writer.finishWritingWithCompletionHandler(&block) };
    if rx.recv_timeout(FINISH_WAIT).is_err() {
        // 写入器还在收尾，不能 cancelWriting；文件多半是坏的，不留下（M24 规则 9）
        log::warn!("录屏：等写完文件超时");
        let _ = std::fs::remove_file(path);
        return Err(record_err("写完文件超时"));
    }
    // SAFETY: 只读属性
    if unsafe { writer.status() } != AVAssetWriterStatus::Completed {
        // SAFETY: 只读属性
        let e = unsafe { writer.error() };
        discard(st, path);
        return Err(record_err(format!(
            "写入失败：{}",
            e.as_deref().map_or_else(|| "未知".into(), ns_err)
        )));
    }
    // SAFETY: 两个 CMTime 都有效
    let secs = unsafe { end.subtract(first).seconds() };
    let duration_ms = if secs.is_finite() && secs > 0.0 {
        (secs * 1000.0).round() as u64
    } else {
        0
    };
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    Ok(RecordOutcome::Saved { duration_ms, bytes })
}

/// 同一帧换成新的显示时间。
fn retimed(sb: &CMSampleBuffer, pts: CMTime) -> Option<CFRetained<CMSampleBuffer>> {
    // SAFETY: kCMTimeInvalid 是 CoreMedia 导出的常量
    let invalid = unsafe { kCMTimeInvalid };
    let timing = CMSampleTimingInfo {
        duration: invalid,
        presentationTimeStamp: pts,
        decodeTimeStamp: invalid,
    };
    let mut out: *mut CMSampleBuffer = std::ptr::null_mut();
    // SAFETY: timing 与 out 都是本地变量；成功时 out 是 +1 引用，由 CFRetained 接管
    unsafe {
        let status = CMSampleBuffer::create_copy_with_new_timing(
            None,
            sb,
            1,
            &timing,
            NonNull::from(&mut out),
        );
        if status != 0 {
            return None;
        }
        NonNull::new(out).map(|p| CFRetained::from_raw(p))
    }
}

/// 写坏的文件不留下。
fn discard(st: &State, path: &Path) {
    // SAFETY: 只在写入器开始写之后调用
    unsafe {
        if st.writer.status() == AVAssetWriterStatus::Writing {
            st.writer.cancelWriting();
        }
    }
    let _ = std::fs::remove_file(path);
}

fn shareable_content() -> Result<Retained<SCShareableContent>, PlatformError> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |c: *mut SCShareableContent, e: *mut NSError| {
        // SAFETY: 回调期间两个指针为空或指向有效对象；内容加一次引用后交出
        let r = unsafe {
            match Retained::retain(c) {
                Some(c) => Ok(c),
                None => Err(e.as_ref().map_or_else(|| "无内容".into(), ns_err)),
            }
        };
        let _ = tx.send(r);
    });
    // SAFETY: block 在回调前一直存活（系统复制并持有）
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    rx.recv_timeout(CALLBACK_WAIT)
        .map_err(|_| record_err("取可录制内容时系统未回调"))?
        .map_err(|e| record_err(format!("取可录制内容失败：{e}")))
}

/// 开始录制：阻塞到 startCapture 回调。失败时不留文件。
pub fn start_recording(opts: RecordOptions) -> Result<Recorder, PlatformError> {
    autoreleasepool(|_| start(opts))
}

fn start(opts: RecordOptions) -> Result<Recorder, PlatformError> {
    let content = shareable_content()?;
    // SAFETY: 以下都是只读属性
    let (display, exclude) = unsafe {
        let display = content
            .displays()
            .iter()
            .find(|d| d.displayID() == opts.display)
            .ok_or_else(|| record_err("找不到显示器"))?;
        let exclude: Vec<_> = opts
            .exclude_window
            .and_then(|id| content.windows().iter().find(|w| w.windowID() == id))
            .into_iter()
            .collect();
        (display, exclude)
    };
    let (x, y, w, h) = opts.rect;
    let (pw, ph) = output_size(w, h, opts.scale);
    let mic = opts.microphone && crate::macos_version().is_some_and(|v| v >= 15);

    // SAFETY: 以下设置都是 macOS 12.3 / 13 起就有的属性；captureMicrophone 只在 15 起设置
    let (filter, config) = unsafe {
        let filter = SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            &display,
            &NSArray::from_retained_slice(&exclude),
        );
        let config = SCStreamConfiguration::new();
        config.setSourceRect(CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)));
        config.setWidth(pw as usize);
        config.setHeight(ph as usize);
        config.setMinimumFrameInterval(CMTime::new(1, 30));
        config.setShowsCursor(true);
        config.setCapturesAudio(opts.system_audio);
        config.setSampleRate(AUDIO_RATE as isize);
        config.setChannelCount(AUDIO_CHANNELS as isize);
        if mic {
            config.setCaptureMicrophone(true);
        }
        (filter, config)
    };

    let state = writer_state(&opts.path, pw, ph, opts.system_audio, mic)?;
    let output = Output::new(Ivars {
        failed: AtomicBool::new(false),
        state: Mutex::new(state),
    });
    let queue = DispatchQueue::new("app.xpaste.record", DispatchQueueAttr::SERIAL);
    // SAFETY: SCStream 弱引用 delegate、强引用输出；output 由 Recorder 持有到停止之后
    let stream = unsafe {
        SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*output)),
        )
    };
    let mut kinds = vec![SCStreamOutputType::Screen];
    if opts.system_audio {
        kinds.push(SCStreamOutputType::Audio);
    }
    if mic {
        kinds.push(SCStreamOutputType::Microphone);
    }
    for kind in kinds {
        // SAFETY: queue 是串行队列，回调都在其上执行
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                kind,
                Some(&queue),
            )
        }
        .map_err(|e| record_err(format!("添加输出失败：{}", ns_err(&e))))?;
    }
    let rec = Recorder {
        stream,
        output,
        queue,
        path: opts.path,
    };
    if let Err(e) = wait_done(|b| {
        // SAFETY: 完成回调由 wait_done 持有到调用结束
        unsafe { rec.stream.startCaptureWithCompletionHandler(Some(b)) }
    }) {
        // 万一已收到首帧开始写了文件，一并清掉
        rec.queue.exec_sync(|| ());
        let mut st = rec.output.state();
        st.closed = true;
        if st.first.is_some() {
            discard(&st, &rec.path);
        }
        return Err(record_err(format!("开始采集失败：{e}")));
    }
    Ok(rec)
}

fn writer_state(
    path: &Path,
    w: u32,
    h: u32,
    system_audio: bool,
    mic: bool,
) -> Result<State, PlatformError> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    // SAFETY: 以下是 AVFoundation 导出的常量与普通构造；设置字典的键值类型与文档一致
    unsafe {
        let file_type = AVFileTypeMPEG4.ok_or_else(|| record_err("缺少 MPEG-4 类型"))?;
        let (Some(video_type), Some(audio_type)) = (AVMediaTypeVideo, AVMediaTypeAudio) else {
            return Err(record_err("缺少媒体类型"));
        };
        let writer = AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type)
            .map_err(|e| record_err(format!("创建写入器失败：{}", ns_err(&e))))?;
        let (Some(codec_key), Some(h264), Some(w_key), Some(h_key)) = (
            AVVideoCodecKey,
            AVVideoCodecTypeH264,
            AVVideoWidthKey,
            AVVideoHeightKey,
        ) else {
            return Err(record_err("缺少视频设置键"));
        };
        let (wn, hn) = (NSNumber::new_u32(w), NSNumber::new_u32(h));
        let video_settings: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[codec_key, w_key, h_key],
            &[h264.as_ref(), wn.as_ref(), hn.as_ref()],
        );
        let video = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            video_type,
            Some(&video_settings),
        );
        let audio_input = || {
            // AVAudioSettings 的键在 AVFAudio 里，为四个字符串常量不再引一个 crate；值即键名
            let (fmt, rate, ch, br) = (
                NSNumber::new_u32(AAC),
                NSNumber::new_i32(AUDIO_RATE),
                NSNumber::new_i32(AUDIO_CHANNELS),
                NSNumber::new_i32(AUDIO_BITRATE),
            );
            let settings: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
                &[
                    ns_string!("AVFormatIDKey"),
                    ns_string!("AVSampleRateKey"),
                    ns_string!("AVNumberOfChannelsKey"),
                    ns_string!("AVEncoderBitRateKey"),
                ],
                &[fmt.as_ref(), rate.as_ref(), ch.as_ref(), br.as_ref()],
            );
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                audio_type,
                Some(&settings),
            )
        };
        let audio = system_audio.then(audio_input);
        let mic = mic.then(audio_input);
        for input in [Some(&video), audio.as_ref(), mic.as_ref()]
            .into_iter()
            .flatten()
        {
            input.setExpectsMediaDataInRealTime(true);
            if !writer.canAddInput(input) {
                return Err(record_err("写入器不接受输入"));
            }
            writer.addInput(input);
        }
        Ok(State {
            writer,
            video,
            audio,
            mic,
            first: None,
            last: None,
            closed: false,
        })
    }
}

/// 麦克风授权状态（只查询）。
pub fn microphone_access() -> MicAccess {
    // SAFETY: AVFoundation 导出的常量；纯查询
    let status = unsafe {
        let Some(audio) = AVMediaTypeAudio else {
            return MicAccess::Denied;
        };
        AVCaptureDevice::authorizationStatusForMediaType(audio)
    };
    match status {
        AVAuthorizationStatus::Authorized => MicAccess::Granted,
        AVAuthorizationStatus::NotDetermined => MicAccess::Undetermined,
        _ => MicAccess::Denied,
    }
}

/// 申请麦克风权限：未决定时弹系统框，阻塞到用户选择；已决定时直接返回结果。
/// 应用的 Info.plist 必须有 `NSMicrophoneUsageDescription`，否则系统直接终止进程。
pub fn request_microphone() -> bool {
    // SAFETY: AVFoundation 导出的常量
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return false;
    };
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |ok: Bool| {
        let _ = tx.send(ok.as_bool());
    });
    // SAFETY: block 在回调前一直存活（系统复制并持有）
    unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block) };
    rx.recv().unwrap_or(false)
}

/// NSWindow 的窗口编号（即 SCWindow.windowID / CGWindowID）。未上屏的窗口为 0 或负数，返回 None。
pub fn window_number(ns_window: *mut c_void) -> Option<u32> {
    // SAFETY: 调用方传入 Tauri 给出的 NSWindow 指针或空指针
    let window = unsafe { ns_window.cast::<AnyObject>().as_ref() }?;
    // SAFETY: NSWindow 的 windowNumber 返回 NSInteger
    let n: isize = unsafe { msg_send![window, windowNumber] };
    u32::try_from(n).ok().filter(|&n| n > 0)
}

/// 文件引用的剪切板格式：一个 `public.file-url`，值为 file:// URL 字符串。
pub fn file_flavors(path: &Path) -> Vec<Flavor> {
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    url.absoluteString()
        .map(|s| {
            vec![Flavor {
                item: 0,
                format: "public.file-url".into(),
                data: s.to_string().into_bytes(),
            }]
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_flavors_is_one_file_url() {
        let f = file_flavors(Path::new("/tmp/录屏 1.mp4"));
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].item, f[0].format.as_str()), (0, "public.file-url"));
        assert_eq!(
            String::from_utf8(f[0].data.clone()).unwrap(),
            "file:///tmp/%E5%BD%95%E5%B1%8F%201.mp4"
        );
    }

    #[test]
    fn window_number_rejects_null() {
        assert_eq!(window_number(std::ptr::null_mut()), None);
    }
}
