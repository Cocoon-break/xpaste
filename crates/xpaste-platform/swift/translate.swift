// 系统翻译桥接（M26 规则 7）：Translation 框架只有 Swift 接口，这里用 @_cdecl 导出 C 函数给 Rust 调用。
// 以 macOS 13 为部署目标编译、弱链接 Translation，所有入口先判断 macOS 26。

import AppKit
import Foundation
import NaturalLanguage
import SwiftUI
import Translation

/// 回调：code 0 成功（out 为译文），1 缺语言包（out 为源语言代码），2 不支持，3 其他失败。
/// 可能在任意线程上调用，out 只在回调期间有效；每次 xp_tr_translate 恰好回调一次。
public typealias XpTrCallback = @convention(c) (UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?) -> Void

/// 英语 ↔ 目标语言的语言包状态（目标是 en 时查简体中文 ↔ 英语）：0 已下载，1 需要下载，2 不支持。
@_cdecl("xp_tr_pack")
public func xp_tr_pack(_ target: UnsafePointer<CChar>) -> Int32 {
    guard #available(macOS 26, *) else { return 2 }
    let tgt = String(cString: target)
    let src = tgt == "en" ? "zh-Hans" : "en"
    // LanguageAvailability 内部要靠主队列回调：调用方须是工作线程，且主线程在跑事件循环（应用里成立）。
    // 在主线程上调用会等不到结果，10 秒后按不支持返回，免得卡死
    let done = DispatchSemaphore(value: 0)
    let box = PackResult()
    Task {
        let st = await LanguageAvailability().status(
            from: Locale.Language(identifier: src), to: Locale.Language(identifier: tgt))
        box.code = switch st {
        case .installed: 0
        case .supported: 1
        default: 2
        }
        done.signal()
    }
    return done.wait(timeout: .now() + 10) == .success ? box.code : 2
}

private final class PackResult: @unchecked Sendable {
    var code: Int32 = 2
}

@_cdecl("xp_tr_translate")
public func xp_tr_translate(
    _ text: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>,
    _ ctx: UnsafeMutableRawPointer?, _ cb: XpTrCallback
) {
    guard #available(macOS 26, *) else { return cb(ctx, 2, nil) }
    let src = String(cString: text)
    let tgt = String(cString: target)
    let rec = NLLanguageRecognizer()
    rec.processString(src)
    guard let lang = rec.dominantLanguage, lang != .undetermined else { return cb(ctx, 2, nil) }
    // ctx 换成整数再跨进 Task：裸指针不是 Sendable
    let ctxBits = UInt(bitPattern: ctx)
    Task {
        let ctx = UnsafeMutableRawPointer(bitPattern: ctxBits)
        do {
            let session = TranslationSession(
                installedSource: Locale.Language(identifier: lang.rawValue),
                target: Locale.Language(identifier: tgt))
            let r = try await session.translate(src)
            r.targetText.withCString { cb(ctx, 0, $0) }
        } catch TranslationError.notInstalled {
            lang.rawValue.withCString { cb(ctx, 1, $0) }
        } catch TranslationError.unsupportedLanguagePairing, TranslationError.unsupportedSourceLanguage,
            TranslationError.unsupportedTargetLanguage
        {
            cb(ctx, 2, nil)
        } catch {
            cb(ctx, 3, nil)
        }
    }
}

/// 下载回调：code 0 已下载，1 用户取消（或仍未下载），2 失败（确认框弹不出来）。在主线程上调用，恰好一次。
public typealias XpTrPrepareCallback = @convention(c) (UnsafeMutableRawPointer?, Int32) -> Void

/// 弹出系统“下载翻译语言”确认并等下载完成（设计 §3.1、§4）。`prepareTranslation()` 的会话只能来自 SwiftUI 的
/// `.translationTask`，所以在主线程把一个 1×1 透明的 SwiftUI 视图挂进 `window`（NSWindow，须是正在显示的
/// xPaste 窗口，隐藏窗口里不弹确认、也不返回）的 contentView，完成后先移除视图再回调。
/// 立即返回；`source` 为空串时用英语（目标是 en 时用简体中文），同 xp_tr_pack。
@_cdecl("xp_tr_prepare")
public func xp_tr_prepare(
    _ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>, _ window: UnsafeMutableRawPointer?,
    _ ctx: UnsafeMutableRawPointer?, _ cb: XpTrPrepareCallback
) {
    let tgt = String(cString: target)
    let given = String(cString: source)
    let src = !given.isEmpty ? given : tgt == "en" ? "zh-Hans" : "en"
    // 裸指针不是 Sendable，换成整数跨进主队列
    let winBits = UInt(bitPattern: window)
    let ctxBits = UInt(bitPattern: ctx)
    DispatchQueue.main.async {
        let ctx = UnsafeMutableRawPointer(bitPattern: ctxBits)
        guard #available(macOS 26, *), let ptr = UnsafeMutableRawPointer(bitPattern: winBits),
            let content = Unmanaged<NSWindow>.fromOpaque(ptr).takeUnretainedValue().contentView
        else { return cb(ctx, 2) }
        let holder = ViewHolder()
        let host = NSHostingView(
            rootView: PrepareView(
                cfg: .init(source: Locale.Language(identifier: src), target: Locale.Language(identifier: tgt))
            ) { code in
                holder.view?.removeFromSuperview()
                holder.view = nil
                cb(ctx, code)
            })
        host.frame = NSRect(x: 0, y: 0, width: 1, height: 1)
        holder.view = host
        content.addSubview(host)
    }
}

private final class ViewHolder {
    var view: NSView?
}

@available(macOS 26, *)
private struct PrepareView: View {
    let cfg: TranslationSession.Configuration
    /// 主线程上调用
    let finish: (Int32) -> Void

    var body: some View {
        Color.clear.frame(width: 1, height: 1)
            .translationTask(cfg) { session in
                let t0 = ContinuousClock.now
                var code: Int32
                do {
                    try await session.prepareTranslation()
                    // 返回不等于装好了（例如用户关掉确认框）：再查一次这一对的状态
                    let st = await LanguageAvailability().status(from: cfg.source!, to: cfg.target)
                    code = st == .installed ? 0 : 1
                } catch is CancellationError {
                    code = 1
                } catch let e as CocoaError where e.code == .userCancelled {
                    code = 1
                } catch {
                    // 1 秒内就抛错视为确认框弹不出来（设计 §7）；之后的错误多半是用户关掉了确认框
                    code = ContinuousClock.now - t0 < .seconds(1) ? 2 : 1
                }
                let c = code
                await MainActor.run { finish(c) }
            }
    }
}
