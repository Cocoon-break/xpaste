//! 生图接口客户端：OpenAI 兼容、火山方舟、Gemini 三种格式。
//! 参考图由调用方先用 `encode_ref` 编好；结果统一转成 PNG 字节返回。

// 调用方（表情包的生成与“测试生成”）在后续任务里接入
#![allow(dead_code)]

use std::io::Cursor;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use xpaste_core::translate::{http_error, is_loopback};

use crate::translate::{Fail, Timeouts, build_client, net};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Openai,
    Ark,
    Gemini,
}

/// 带 Key 和接口地址，不派生 Debug。
pub struct ImageCfg {
    pub format: Format,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

pub enum ImgFail {
    Base(Fail),
    /// 服务商因内容审核拒绝
    Moderation,
    /// OpenAI 兼容格式带参考图时被 400 / 415 / 422 拒绝：模型多半不收参考图
    NoRefs,
}

/// 连接 10 秒；读空闲 180 秒（生图慢，首段与段间都算）。
const TIMEOUTS: Timeouts = Timeouts {
    connect: Duration::from_secs(10),
    read: Duration::from_secs(180),
};

const REF_EDGE: u32 = 1024;
const REF_MAX_BYTES: usize = 4_194_304;

pub fn unconfigured(c: &ImageCfg) -> bool {
    c.base_url.trim().is_empty() || c.model.trim().is_empty()
}

pub async fn generate(c: &ImageCfg, prompt: &str, refs: &[Vec<u8>]) -> Result<Vec<u8>, ImgFail> {
    let client = build_client(is_loopback(&c.base_url), TIMEOUTS).map_err(ImgFail::Base)?;
    let base = c.base_url.trim().trim_end_matches('/');
    let model = c.model.trim();
    let key = c.api_key.trim();
    let data_url = |r: &Vec<u8>| format!("data:{};base64,{}", mime(r), B64.encode(r));
    let req = match c.format {
        Format::Openai if refs.is_empty() => client
            .post(format!("{base}/images/generations"))
            .json(&json!({"model": model, "prompt": prompt, "size": "1024x1024", "n": 1})),
        Format::Openai => {
            let mut form = reqwest::multipart::Form::new()
                .text("model", model.to_string())
                .text("prompt", prompt.to_string())
                .text("size", "1024x1024")
                .text("n", "1");
            for (i, r) in refs.iter().enumerate() {
                let ext = if mime(r) == "image/png" { "png" } else { "jpg" };
                let part = reqwest::multipart::Part::bytes(r.clone())
                    .file_name(format!("ref{i}.{ext}"))
                    .mime_str(mime(r))
                    .map_err(|_| ImgFail::Base(Fail::Bad))?;
                form = form.part("image[]", part);
            }
            client.post(format!("{base}/images/edits")).multipart(form)
        }
        Format::Ark => {
            let mut body = json!({"model": model, "prompt": prompt, "size": "1024x1024",
                "response_format": "b64_json"});
            if !refs.is_empty() {
                body["image"] = refs.iter().map(data_url).collect();
            }
            client
                .post(format!("{base}/images/generations"))
                .json(&body)
        }
        Format::Gemini => {
            let mut parts = vec![json!({"text": prompt})];
            parts
                .extend(refs.iter().map(
                    |r| json!({"inline_data": {"mime_type": mime(r), "data": B64.encode(r)}}),
                ));
            client
                .post(format!("{base}/models/{model}:generateContent"))
                .header("x-goog-api-key", key)
                .json(&json!({"contents": [{"parts": parts}]}))
        }
    };
    let req = match (c.format, key.is_empty()) {
        (Format::Gemini, _) | (_, true) => req,
        _ => req.bearer_auth(key),
    };
    let resp = req.send().await.map_err(|e| ImgFail::Base(net(e)))?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| ImgFail::Base(net(e)))?;
    if !status.is_success() {
        let s = status.as_u16();
        let text = String::from_utf8_lossy(&bytes);
        let moderated = s == 400
            && ["content_policy", "moderation", "safety", "SAFETY"]
                .iter()
                .any(|m| text.contains(m));
        return Err(if moderated {
            ImgFail::Moderation
        } else if !refs.is_empty() && c.format == Format::Openai && matches!(s, 400 | 415 | 422) {
            ImgFail::NoRefs
        } else {
            ImgFail::Base(Fail::Http(s))
        });
    }
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| ImgFail::Base(Fail::Bad))?;
    let raw = if c.format == Format::Gemini {
        gemini_image(&v)?
    } else {
        let d = &v["data"][0];
        if let Some(b) = d["b64_json"].as_str() {
            B64.decode(b).map_err(|_| ImgFail::Base(Fail::Bad))?
        } else if let Some(u) = d["url"].as_str() {
            // 结果链接是服务商给的，下载不带 Key
            let r = client
                .get(u)
                .send()
                .await
                .map_err(|e| ImgFail::Base(net(e)))?;
            if !r.status().is_success() {
                return Err(ImgFail::Base(Fail::Bad));
            }
            r.bytes().await.map_err(|e| ImgFail::Base(net(e)))?.to_vec()
        } else {
            return Err(ImgFail::Base(Fail::Bad));
        }
    };
    // 结果可能是 PNG / JPEG / WebP，统一转成 PNG
    let img = image::load_from_memory(&raw).map_err(|_| ImgFail::Base(Fail::Bad))?;
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|_| ImgFail::Base(Fail::Bad))?;
    Ok(png)
}

fn gemini_image(v: &Value) -> Result<Vec<u8>, ImgFail> {
    let cand = &v["candidates"][0];
    if matches!(
        cand["finishReason"].as_str(),
        Some("SAFETY" | "PROHIBITED_CONTENT")
    ) {
        return Err(ImgFail::Moderation);
    }
    cand["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|p| {
            p["inline_data"]["data"]
                .as_str()
                .or(p["inlineData"]["data"].as_str())
        })
        .and_then(|d| B64.decode(d).ok())
        .ok_or(ImgFail::Base(Fail::Bad))
}

fn mime(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xFF, 0xD8]) {
        "image/jpeg"
    } else {
        "image/png"
    }
}

/// 失败原因的文案：同 `mixer::reason` 的分类，“AI 接口”换成“生图接口”。
pub fn reason(f: &ImgFail) -> String {
    match f {
        ImgFail::Moderation => "生图接口拒绝了这次请求（可能是内容审核），换个说法再试".into(),
        ImgFail::NoRefs => "这个模型不支持参考图，换一个模型，或拿掉参考图再试".into(),
        ImgFail::Base(f) => match f {
            Fail::Http(s @ (401 | 403 | 404 | 429)) => http_error(*s),
            Fail::Http(s) => format!("生图接口出错（HTTP {s}）"),
            Fail::Config => "生图接口还没配好".into(),
            Fail::Timeout => "生图接口超时".into(),
            Fail::Connect => "连不上生图接口，请检查网络或接口地址".into(),
            Fail::Internal(e) => e.to_string(),
            _ => "生图接口返回的内容无法识别".into(),
        },
    }
}

/// 参考图：长边缩到 1024，PNG 超过 4 MB 改质量 85 的 JPEG（透明处铺白）。
pub fn encode_ref(png_or_any: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(png_or_any).ok()?;
    crate::mixer::encode_image_within(img, REF_EDGE, REF_MAX_BYTES)
        .ok()
        .map(|(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    struct Req {
        path: String,
        head: String,
        body: Vec<u8>,
    }
    impl Req {
        fn json(&self) -> Value {
            serde_json::from_slice(&self.body).unwrap()
        }
    }

    type Reply = (u16, &'static str, Vec<u8>);

    /// 假服务：依次接 `n` 个连接，`handler` 按请求给 (状态码, Content-Type, 体)。返回 Base URL 与收到的请求。
    fn fake(
        n: usize,
        handler: impl Fn(&Req) -> Reply + Send + 'static,
    ) -> (String, thread::JoinHandle<Vec<Req>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let h = thread::spawn(move || {
            let mut seen = Vec::new();
            for _ in 0..n {
                let (mut s, _) = l.accept().unwrap();
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                let body_at = loop {
                    let k = s.read(&mut buf).unwrap();
                    raw.extend_from_slice(&buf[..k]);
                    if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&raw[..body_at]).to_ascii_lowercase();
                let len: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .map_or(0, |v| v.trim().parse().unwrap());
                while raw.len() < body_at + len {
                    let k = s.read(&mut buf).unwrap();
                    raw.extend_from_slice(&buf[..k]);
                }
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let req = Req {
                    path,
                    head,
                    body: raw[body_at..].to_vec(),
                };
                let (st, ct, body) = handler(&req);
                let h = format!(
                    "HTTP/1.1 {st} X\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                s.write_all(h.as_bytes()).unwrap();
                s.write_all(&body).unwrap();
                seen.push(req);
            }
            seen
        });
        (url, h)
    }

    fn png_1x1() -> Vec<u8> {
        let mut v = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1))
            .write_to(&mut Cursor::new(&mut v), image::ImageFormat::Png)
            .unwrap();
        v
    }

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn cfg(format: Format, base: &str) -> ImageCfg {
        ImageCfg {
            format,
            base_url: base.into(),
            api_key: "k".into(),
            model: "m".into(),
        }
    }

    fn json_reply(v: Value) -> Reply {
        (200, "application/json", v.to_string().into_bytes())
    }

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    #[test]
    fn openai_b64() {
        let png = png_1x1();
        let p2 = png.clone();
        let (url, h) = fake(1, move |_| {
            json_reply(json!({"data":[{"b64_json": b64(&p2)}]}))
        });
        let r = run(generate(&cfg(Format::Openai, &url), "p", &[]))
            .ok()
            .unwrap();
        assert_eq!(image::load_from_memory(&r).unwrap().width(), 1);
        let seen = h.join().unwrap();
        assert_eq!(seen[0].path, "/images/generations");
        assert!(seen[0].head.contains("authorization: bearer k"));
        let b = seen[0].json();
        assert_eq!(b["size"], "1024x1024");
        assert_eq!(b["n"], 1);
        assert_eq!(b["model"], "m");
    }

    #[test]
    fn openai_edits_multipart() {
        let png = png_1x1();
        let p2 = png.clone();
        let (url, h) = fake(1, move |_| {
            json_reply(json!({"data":[{"b64_json": b64(&p2)}]}))
        });
        run(generate(&cfg(Format::Openai, &url), "p", &[png]))
            .ok()
            .unwrap();
        let seen = h.join().unwrap();
        assert_eq!(seen[0].path, "/images/edits");
        assert!(seen[0].head.contains("content-type: multipart/form-data"));
        assert!(String::from_utf8_lossy(&seen[0].body).contains("name=\"image[]\""));
    }

    fn host(r: &Req) -> String {
        r.head
            .lines()
            .find_map(|l| l.strip_prefix("host:"))
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn url_result_downloaded() {
        let png = png_1x1();
        let (url, h) = fake(2, move |r| {
            if r.path == "/x.png" {
                (200, "image/png", png.clone())
            } else {
                json_reply(json!({"data":[{"url": format!("http://{}/x.png", host(r))}]}))
            }
        });
        let r = run(generate(&cfg(Format::Openai, &url), "p", &[]))
            .ok()
            .unwrap();
        assert_eq!(image::load_from_memory(&r).unwrap().width(), 1);
        let seen = h.join().unwrap();
        // 下载图片不带 Key
        assert!(!seen[1].head.contains("authorization"));
    }

    #[test]
    fn url_download_fails() {
        let (url, _h) = fake(2, move |r| {
            if r.path == "/x.png" {
                (404, "text/plain", b"no".to_vec())
            } else {
                json_reply(json!({"data":[{"url": format!("http://{}/x.png", host(r))}]}))
            }
        });
        let e = run(generate(&cfg(Format::Openai, &url), "p", &[]))
            .err()
            .unwrap();
        assert_eq!(reason(&e), "生图接口返回的内容无法识别");
    }

    #[test]
    fn ark_refs_as_data_urls() {
        let png = png_1x1();
        let p2 = png.clone();
        let (url, h) = fake(1, move |_| {
            json_reply(json!({"data":[{"b64_json": b64(&p2)}]}))
        });
        run(generate(&cfg(Format::Ark, &url), "p", &[png]))
            .ok()
            .unwrap();
        let seen = h.join().unwrap();
        assert_eq!(seen[0].path, "/images/generations");
        let b = seen[0].json();
        assert_eq!(b["image"].as_array().unwrap().len(), 1);
        assert!(b["image"][0].as_str().unwrap().starts_with("data:image/"));
    }

    #[test]
    fn gemini_inline() {
        let png = png_1x1();
        let p2 = png.clone();
        let (url, h) = fake(1, move |_| {
            json_reply(json!({"candidates":[{"content":{"parts":[
                {"text":"ok"},{"inline_data":{"mime_type":"image/png","data": b64(&p2)}}]}}]}))
        });
        run(generate(&cfg(Format::Gemini, &url), "p", &[png]))
            .ok()
            .unwrap();
        let seen = h.join().unwrap();
        assert_eq!(seen[0].path, "/models/m:generatecontent");
        assert!(seen[0].head.contains("x-goog-api-key: k"));
        let b = seen[0].json();
        assert!(
            b["contents"][0]["parts"]
                .to_string()
                .contains("inline_data")
        );
    }

    #[test]
    fn gemini_safety_is_moderation() {
        let (url, _h) = fake(1, |_| {
            json_reply(json!({"candidates":[{"finishReason":"PROHIBITED_CONTENT"}]}))
        });
        let e = run(generate(&cfg(Format::Gemini, &url), "p", &[]))
            .err()
            .unwrap();
        assert!(matches!(e, ImgFail::Moderation));
    }

    #[test]
    fn moderation_reason() {
        let (url, _h) = fake(1, |_| {
            (
                400,
                "application/json",
                br#"{"error":{"code":"content_policy_violation"}}"#.to_vec(),
            )
        });
        let e = run(generate(&cfg(Format::Openai, &url), "p", &[png_1x1()]))
            .err()
            .unwrap();
        assert!(matches!(e, ImgFail::Moderation));
        assert_eq!(
            reason(&e),
            "生图接口拒绝了这次请求（可能是内容审核），换个说法再试"
        );
    }

    #[test]
    fn no_refs_reason() {
        let (url, _h) = fake(1, |_| (422, "text/plain", b"nope".to_vec()));
        let e = run(generate(&cfg(Format::Openai, &url), "p", &[png_1x1()]))
            .err()
            .unwrap();
        assert!(matches!(e, ImgFail::NoRefs));
        assert_eq!(
            reason(&e),
            "这个模型不支持参考图，换一个模型，或拿掉参考图再试"
        );
    }

    #[test]
    fn plain_http_reasons() {
        let (url, _h) = fake(1, |_| (500, "text/plain", b"x".to_vec()));
        let e = run(generate(&cfg(Format::Openai, &url), "p", &[]))
            .err()
            .unwrap();
        assert_eq!(reason(&e), "生图接口出错（HTTP 500）");
        assert_eq!(
            reason(&ImgFail::Base(Fail::Http(401))),
            "API Key 无效或没有权限"
        );
        assert_eq!(reason(&ImgFail::Base(Fail::Timeout)), "生图接口超时");
    }

    #[test]
    fn jpeg_result_becomes_png() {
        let mut jpg = Vec::new();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2))
            .write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Jpeg)
            .unwrap();
        let (url, _h) = fake(1, move |_| {
            json_reply(json!({"data":[{"b64_json": b64(&jpg)}]}))
        });
        let r = run(generate(&cfg(Format::Openai, &url), "p", &[]))
            .ok()
            .unwrap();
        assert_eq!(&r[..4], b"\x89PNG");
    }

    #[test]
    fn encode_ref_shrinks_and_flattens() {
        let mut big = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(8000, 2000))
            .write_to(&mut Cursor::new(&mut big), image::ImageFormat::Png)
            .unwrap();
        let out = encode_ref(&big).unwrap();
        let back = image::load_from_memory(&out).unwrap();
        assert_eq!((back.width(), back.height()), (1024, 256));

        // xorshift 噪点：PNG 压不下去；左上角留一块全透明
        let mut x: u32 = 0x9E37_79B9;
        let img = image::RgbaImage::from_fn(1024, 1024, |px, py| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            if px < 50 && py < 50 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba(x.to_le_bytes())
            }
        });
        let mut noisy = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut noisy), image::ImageFormat::Png)
            .unwrap();
        assert!(noisy.len() > REF_MAX_BYTES);
        let out = encode_ref(&noisy).unwrap();
        assert_eq!(&out[..2], &[0xFF, 0xD8]);
        let px = image::load_from_memory(&out)
            .unwrap()
            .to_rgb8()
            .get_pixel(10, 10)
            .0;
        assert!(px.iter().all(|&c| c >= 250), "{px:?}");
    }
}
