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
    // 同 encodeURIComponent：只保留非保留字符
    for b in path.to_string_lossy().bytes() {
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
    d
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
