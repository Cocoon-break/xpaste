//! 本地文件路径 → asset 协议 URL（与 `@tauri-apps/api` 的 `convertFileSrc` 一致）。

use std::fmt::Write;
use std::path::Path;

use xpaste_core::{RecordDetail, RecordSummary};

pub fn asset_url(path: &Path) -> String {
    let mut out = String::from(if cfg!(windows) {
        "http://asset.localhost/"
    } else {
        "asset://localhost/"
    });
    out.push_str(&encode_component(&path.to_string_lossy()));
    out
}

/// 同 encodeURIComponent：只保留非保留字符。
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn url(p: String) -> String {
    asset_url(Path::new(&p))
}

pub fn summary_dto(mut s: RecordSummary) -> RecordSummary {
    s.thumb = s.thumb.map(url);
    if let Some(a) = &mut s.app {
        a.icon = a.icon.take().map(url);
    }
    s
}

pub fn detail_dto(mut d: RecordDetail) -> RecordDetail {
    d.summary = summary_dto(d.summary);
    if let Some(i) = &mut d.image {
        i.src = url(std::mem::take(&mut i.src));
    }
    if let Some(v) = &mut d.video {
        v.src = v.src.take().map(url);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(windows))]
    use xpaste_core::{Kind, VideoInfo};

    #[cfg(not(windows))]
    #[test]
    fn asset_url_encodes_like_encode_uri_component() {
        assert_eq!(
            asset_url(Path::new("/Users/a b/图.png")),
            "asset://localhost/%2FUsers%2Fa%20b%2F%E5%9B%BE.png"
        );
        assert_eq!(
            asset_url(Path::new("x-_.!~*'()#?&")),
            "asset://localhost/x-_.!~*'()%23%3F%26"
        );
    }

    #[cfg(windows)]
    #[test]
    fn asset_url_windows_matches_convert_file_src() {
        assert_eq!(
            asset_url(Path::new(r"C:\Users\a b\图.png")),
            "http://asset.localhost/C%3A%5CUsers%5Ca%20b%5C%E5%9B%BE.png"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn detail_dto_converts_video_src() {
        let d = |src: Option<&str>| RecordDetail {
            summary: RecordSummary {
                id: 1,
                kind: Kind::Video,
                title: String::new(),
                app: None,
                copied_at: 0,
                uses: 0,
                pinned: false,
                size: 0,
                lang: None,
                thumb: None,
                duration_sec: None,
                ocr_state: 0,
                snippet_body: None,
            },
            text: None,
            truncated: false,
            link_title: None,
            files: vec![],
            image: None,
            video: Some(VideoInfo {
                src: src.map(Into::into),
                w: 0,
                h: 0,
                fps: 0.0,
                codec: String::new(),
            }),
            ocr_text: None,
            translation: None,
            translation_lang: None,
            translation_engine: None,
        };
        let src = |s| detail_dto(d(s)).video.unwrap().src;
        assert_eq!(
            src(Some("/a b.mp4")).as_deref(),
            Some("asset://localhost/%2Fa%20b.mp4")
        );
        assert_eq!(src(None), None);
    }
}
