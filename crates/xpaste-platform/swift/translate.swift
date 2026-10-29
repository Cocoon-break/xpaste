// 系统翻译桥接（设计 §4.2）：Translation 框架只有 Swift 接口，这里用 @_cdecl 导出 C 函数给 Rust 调用。
// 以 macOS 13 为部署目标编译、弱链接 Translation，所有入口先判断 macOS 26。

import Foundation
import NaturalLanguage
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
