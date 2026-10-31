//! 翻译的纯逻辑：语言、方向判断、请求体、SSE 解析、错误文案。
//! 不发请求（网络在 `src-tauri`），M26 规则 8、9。

use serde_json::{Value, json};

/// 原文最多翻译的字符数，M26 规则 9。
pub const MAX_CHARS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    ZhHans,
    ZhHant,
    En,
    Ja,
    Ko,
}

impl Lang {
    pub fn from_code(code: &str) -> Option<Lang> {
        Some(match code {
            "zh-Hans" => Lang::ZhHans,
            "zh-Hant" => Lang::ZhHant,
            "en" => Lang::En,
            "ja" => Lang::Ja,
            "ko" => Lang::Ko,
            _ => return None,
        })
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::ZhHans => "zh-Hans",
            Lang::ZhHant => "zh-Hant",
            Lang::En => "en",
            Lang::Ja => "ja",
            Lang::Ko => "ko",
        }
    }

    /// 界面显示名。
    pub fn label(self) -> &'static str {
        match self {
            Lang::ZhHans => "简体中文",
            Lang::ZhHant => "繁體中文",
            Lang::En => "English",
            Lang::Ja => "日本語",
            Lang::Ko => "한국어",
        }
    }

    /// 写进提示词的英文名。
    pub fn name_en(self) -> &'static str {
        match self {
            Lang::ZhHans => "Simplified Chinese",
            Lang::ZhHant => "Traditional Chinese",
            Lang::En => "English",
            Lang::Ja => "Japanese",
            Lang::Ko => "Korean",
        }
    }
}

/// M26 规则 9：按原文语言决定实际翻成什么语言。
pub fn resolve_target(text: &str, target: Lang) -> Lang {
    let (mut kana, mut hangul, mut han, mut latin) = (false, false, 0usize, 0usize);
    for c in text.chars() {
        match c as u32 {
            0x3040..=0x30FF | 0x31F0..=0x31FF => kana = true,
            0xAC00..=0xD7AF | 0x1100..=0x11FF | 0x3130..=0x318F => hangul = true,
            0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF => han += 1,
            0xC0..=0x24F if c.is_alphabetic() => latin += 1,
            _ if c.is_ascii_alphabetic() => latin += 1,
            _ => {}
        }
    }
    let src = if kana {
        Some(Lang::Ja)
    } else if hangul {
        Some(Lang::Ko)
    } else if han > latin {
        Some(Lang::ZhHans) // 简繁同类
    } else if latin > han {
        Some(Lang::En)
    } else {
        None
    };
    let is_zh = |l: Lang| matches!(l, Lang::ZhHans | Lang::ZhHant);
    match src {
        Some(s) if s == target || (is_zh(s) && is_zh(target)) => {
            if target == Lang::En {
                Lang::ZhHans
            } else {
                Lang::En
            }
        }
        _ => target,
    }
}

/// 归一成 `.../chat/completions`。
pub fn endpoint(base_url: &str) -> String {
    let s = base_url.trim().trim_end_matches('/');
    let s = s.strip_suffix("/chat/completions").unwrap_or(s);
    format!("{}/chat/completions", s.trim_end_matches('/'))
}

/// 主机是否 localhost / 127.* / ::1（这类地址始终直连，M26 规则 11）。
pub fn is_loopback(base_url: &str) -> bool {
    let rest = base_url.trim().split_once("://").map_or("", |(_, r)| r);
    let auth = rest.split('/').next().unwrap_or("");
    let host = match auth.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => auth.split(':').next().unwrap_or(""),
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

pub fn chat_body(model: &str, target: Lang, text: &str) -> Value {
    let system = format!(
        "Translate the user's message into {}. Output only the translation. Keep line breaks and formatting. Keep code, commands, URLs, file paths and identifiers unchanged. Do not add explanations.",
        target.name_en()
    );
    json!({
        "model": model,
        "temperature": 0,
        "stream": true,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": text},
        ],
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum SseLine {
    Delta(String),
    Done,
    Skip,
    Bad,
}

pub fn sse_line(line: &str) -> SseLine {
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Skip; // 空行、注释、event: 等
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseLine::Done;
    }
    let Ok(v) = serde_json::from_str::<Value>(data) else {
        return SseLine::Bad;
    };
    if v.get("error").is_some() {
        return SseLine::Bad;
    }
    match v["choices"][0]["delta"]["content"].as_str() {
        Some(s) if !s.is_empty() => SseLine::Delta(s.to_string()),
        _ => SseLine::Skip, // role、reasoning、只带 usage 的块
    }
}

/// 非流式响应：`choices[0].message.content`。
pub fn completion_text(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_string)
}

/// 翻译方式标签的显示名（M26 规则 16）：`system` → 系统翻译，`openai:{模型}` → AI（模型），认不出为空。
pub fn engine_label(tag: &str) -> String {
    match tag.strip_prefix("openai:") {
        _ if tag == "system" => "系统翻译".into(),
        Some("") => "AI".into(),
        Some(m) => format!("AI（{m}）"),
        None => String::new(),
    }
}

pub fn http_error(status: u16) -> String {
    match status {
        401 | 403 => "API Key 无效或没有权限".into(),
        404 => "接口地址或模型 ID 不对".into(),
        429 => "请求太频繁或额度不足，稍后再试".into(),
        _ => format!("翻译服务出错（HTTP {status}）"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_label_rules() {
        assert_eq!(engine_label("system"), "系统翻译");
        assert_eq!(engine_label("openai:gpt-4o-mini"), "AI（gpt-4o-mini）");
        assert_eq!(engine_label("openai:"), "AI");
        assert_eq!(engine_label("deepl"), "");
    }

    #[test]
    fn resolve_target_follows_design_rules() {
        use Lang::*;
        assert_eq!(resolve_target("The build failed", ZhHans), ZhHans);
        assert_eq!(resolve_target("构建失败了", ZhHans), En);
        assert_eq!(resolve_target("構建失敗", ZhHant), En); // 简繁都算中文
        assert_eq!(resolve_target("hello", En), ZhHans);
        assert_eq!(resolve_target("日本語の文章です", ZhHans), ZhHans); // 含假名算日文，即使汉字更多
        assert_eq!(resolve_target("日本語の文章です", Ja), En);
        assert_eq!(resolve_target("안녕하세요", Ko), En);
        assert_eq!(resolve_target("Привет мир", ZhHans), ZhHans); // 其他文字照常翻成目标语言
        assert_eq!(resolve_target("12345 !!", ZhHans), ZhHans);
        assert_eq!(resolve_target("ab 中文", ZhHans), ZhHans); // 打平，照常翻成目标语言
    }

    #[test]
    fn endpoint_normalizes_base_url() {
        for b in [
            "https://api.openai.com/v1",
            "https://api.openai.com/v1/",
            "https://api.openai.com/v1/chat/completions",
            " https://api.openai.com/v1/chat/completions/ ",
        ] {
            assert_eq!(
                endpoint(b),
                "https://api.openai.com/v1/chat/completions",
                "{b}"
            );
        }
    }

    #[test]
    fn loopback_hosts() {
        for b in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:1234/v1",
            "http://[::1]:8080/v1",
            "http://LOCALHOST/v1",
        ] {
            assert!(is_loopback(b), "{b}");
        }
        for b in [
            "https://api.openai.com/v1",
            "http://localhost.evil.com/v1",
            "http://10.0.0.2/v1",
        ] {
            assert!(!is_loopback(b), "{b}");
        }
    }

    #[test]
    fn chat_body_shape() {
        let v = chat_body("gpt-4o-mini", Lang::ZhHans, "hi");
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["stream"], true);
        assert_eq!(v["temperature"], 0);
        assert_eq!(v["messages"][0]["role"], "system");
        assert!(
            v["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Simplified Chinese")
        );
        assert_eq!(
            v["messages"][1],
            serde_json::json!({"role": "user", "content": "hi"})
        );
    }

    #[test]
    fn sse_parses_deltas_and_done() {
        assert_eq!(
            sse_line(r#"data: {"choices":[{"delta":{"content":"你好"}}]}"#),
            SseLine::Delta("你好".into())
        );
        assert_eq!(sse_line("data: [DONE]"), SseLine::Done);
        assert_eq!(sse_line(""), SseLine::Skip);
        assert_eq!(sse_line(": keep-alive"), SseLine::Skip);
        assert_eq!(sse_line("event: message"), SseLine::Skip);
    }

    #[test]
    fn sse_skips_role_and_reasoning_chunks() {
        assert_eq!(
            sse_line(r#"data: {"choices":[{"delta":{"role":"assistant"}}]}"#),
            SseLine::Skip
        );
        assert_eq!(
            sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"…"}}]}"#),
            SseLine::Skip
        );
        assert_eq!(sse_line(r#"data: {"choices":[]}"#), SseLine::Skip); // 部分服务最后一段只带 usage
    }

    #[test]
    fn sse_error_object_is_bad() {
        assert_eq!(sse_line(r#"data: {"error":{"message":"x"}}"#), SseLine::Bad);
        assert_eq!(sse_line("data: not json"), SseLine::Bad);
    }

    #[test]
    fn completion_and_http_errors() {
        assert_eq!(
            completion_text(r#"{"choices":[{"message":{"content":" 你好 "}}]}"#).as_deref(),
            Some(" 你好 ")
        );
        assert_eq!(completion_text(r#"{"error":{}}"#), None);
        assert_eq!(http_error(401), "API Key 无效或没有权限");
        assert_eq!(http_error(403), "API Key 无效或没有权限");
        assert_eq!(http_error(404), "接口地址或模型 ID 不对");
        assert_eq!(http_error(429), "请求太频繁或额度不足，稍后再试");
        assert_eq!(http_error(500), "翻译服务出错（HTTP 500）");
    }
}
