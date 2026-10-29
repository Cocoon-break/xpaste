//! 系统翻译桥接（M26 规则 7）。两端都跑。

#[test]
fn unsupported_input_does_not_panic() {
    // 空串、只有标点的输入返回 Err（不支持或失败），不 panic、不挂起（30 秒内返回）
    assert!(xpaste_platform::translate_text("!!!", "zh-Hans").is_err());
    assert!(xpaste_platform::translate_text("", "zh-Hans").is_err());
}
