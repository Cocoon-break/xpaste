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

/// `source` 为用户在划词卡片上指定的源语言（M26 规则 20），空串为自动识别。
@_cdecl("xp_tr_translate")
public func xp_tr_translate(
    _ text: UnsafePointer<CChar>, _ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>,
    _ ctx: UnsafeMutableRawPointer?, _ cb: XpTrCallback
) {
    guard #available(macOS 26, *) else { return cb(ctx, 2, nil) }
    let src = String(cString: text)
    let tgt = String(cString: target)
    let given = String(cString: source)
    var cands: [String] = given.isEmpty ? [] : [given]
    if given.isEmpty {
        let rec = NLLanguageRecognizer()
        rec.processString(src)
        guard let top = rec.dominantLanguage, top != .undetermined else { return cb(ctx, 2, nil) }
        // 候选源语言：最可能的在前，再加概率不低于 0.1 的其他猜测，简繁互为候选。
        // 终端输出、短文本、中英混排常被认成别的语言或繁体，只用最可能的那个会要用户下载用不上的语言包
        let hyps = rec.languageHypotheses(withMaximum: 3)
        let guesses = hyps.filter { $0.value >= 0.1 }.sorted { $0.value > $1.value }.map(\.key.rawValue)
        // 没把握（最可能的也不到 0.5，多见于短词、缩写，如 ok 认成波兰语）时按文字再补一个，放在最后
        let byScript = (hyps.values.max() ?? 0) < 0.5 ? [scriptLanguage(src)] : []
        for g in [top.rawValue] + guesses + byScript {
            let pair = g == "zh-Hant" ? ["zh-Hans"] : g == "zh-Hans" ? ["zh-Hant"] : []
            for c in [g] + pair where !cands.contains(c) { cands.append(c) }
        }
    }
    // ctx 换成整数再跨进 Task：裸指针不是 Sendable
    let ctxBits = UInt(bitPattern: ctx)
    Task {
        let ctx = UnsafeMutableRawPointer(bitPattern: ctxBits)
        let target = Locale.Language(identifier: tgt)
        // 用第一个语言包已下载的候选；都没下载时按最可能的、系统支持的那个报缺语言包。与目标同一种语言的候选跳过
        var pick: String?
        var need: String?
        for c in cands where Locale.Language(identifier: c).languageCode != target.languageCode {
            let st = await LanguageAvailability().status(from: Locale.Language(identifier: c), to: target)
            if st == .installed {
                pick = c
                break
            }
            if st == .supported && need == nil { need = c }
        }
        guard let lang = pick else {
            if let n = need { return n.withCString { cb(ctx, 1, $0) } }
            return cb(ctx, 2, nil)
        }
        let run = {
            try await TranslationSession(installedSource: Locale.Language(identifier: lang), target: target)
                .translate(src).targetText
        }
        do {
            let out: String
            do {
                out = try await run()
            } catch TranslationError.notInstalled {
                // 刚查过是已下载：系统翻译服务刚起来时偶尔仍报未下载（下载按钮一点即“已下载”），稍等再试一次
                try await Task.sleep(for: .milliseconds(300))
                out = try await run()
            }
            out.withCString { cb(ctx, 0, $0) }
        } catch TranslationError.notInstalled {
            lang.withCString { cb(ctx, 1, $0) }
        } catch TranslationError.unsupportedLanguagePairing, TranslationError.unsupportedSourceLanguage,
            TranslationError.unsupportedTargetLanguage
        {
            cb(ctx, 2, nil)
        } catch {
            cb(ctx, 3, nil)
        }
    }
}

/// 按文字猜语言：有假名日语，有谚文韩语，有汉字中文，否则英语。
private func scriptLanguage(_ s: String) -> String {
    let has = { (r: ClosedRange<UInt32>) in s.unicodeScalars.contains { r.contains($0.value) } }
    return has(0x3040...0x30FF) ? "ja" : has(0xAC00...0xD7AF) ? "ko" : has(0x4E00...0x9FFF) ? "zh-Hans" : "en"
}

/// 下载回调：code 0 已下载，1 用户取消（或仍未下载），2 失败（确认框弹不出来）。在主线程上调用，恰好一次。
public typealias XpTrPrepareCallback = @convention(c) (UnsafeMutableRawPointer?, Int32) -> Void

/// 弹出系统“下载翻译语言”确认并等下载完成（M26 规则 17）。`prepareTranslation()` 的会话只能来自 SwiftUI 的
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
        guard #available(macOS 26, *), let ptr = UnsafeMutableRawPointer(bitPattern: winBits) else {
            return cb(ctx, 2)
        }
        let win = Unmanaged<NSWindow>.fromOpaque(ptr).takeUnretainedValue()
        guard let content = win.contentView else { return cb(ctx, 2) }
        // 排队期间窗口已被隐藏（xp_tr_prepare_cancel 先到了）：隐藏窗口里不弹确认也不返回，按取消处理
        guard win.isVisible else { return cb(ctx, 1) }
        let holder = PrepareHolder(window: winBits) { cb(ctx, $0) }
        let host = NSHostingView(
            rootView: PrepareView(
                cfg: .init(source: Locale.Language(identifier: src), target: Locale.Language(identifier: tgt)),
                finish: holder.finish))
        host.frame = NSRect(x: 0, y: 0, width: 1, height: 1)
        holder.view = host
        pendingPrepare = holder
        content.addSubview(host)
    }
}

/// 隐藏 `window` 之前调用（M26 规则 17）：那个窗口里有进行中的下载时，移除视图并按取消回调（code 1）。
/// 窗口隐藏后 prepareTranslation 不会返回，不取消的话 Rust 侧会一直等、下载标记一直不复位。
/// 任意线程可调，没有进行中的下载或不是这个窗口时什么都不做。
@_cdecl("xp_tr_prepare_cancel")
public func xp_tr_prepare_cancel(_ window: UInt) {
    DispatchQueue.main.async {
        if let h = pendingPrepare, h.window == window { h.finish(1) }
    }
}

/// 进行中的下载；只在主线程读写（同一时刻最多一个，Rust 侧有标记把关）
private var pendingPrepare: PrepareHolder?

private final class PrepareHolder {
    var view: NSView?
    let window: UInt
    let done: (Int32) -> Void

    init(window: UInt, done: @escaping (Int32) -> Void) {
        self.window = window
        self.done = done
    }

    /// 主线程上调用。只回调一次：.translationTask 重跑或取消与完成先后到达时，后到的直接忽略，否则 Rust 侧会重复收回 ctx
    func finish(_ code: Int32) {
        guard let v = view else { return }
        v.removeFromSuperview()
        view = nil
        if pendingPrepare === self { pendingPrepare = nil }
        done(code)
    }
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
                    // 1 秒内就抛错视为确认框弹不出来（M26 规则 17）；之后的错误多半是用户关掉了确认框
                    code = ContinuousClock.now - t0 < .seconds(1) ? 2 : 1
                }
                let c = code
                await MainActor.run { finish(c) }
            }
    }
}
