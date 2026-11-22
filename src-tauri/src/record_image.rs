//! 历史记录里的原图：位图记录在 `images/<id>.png`，图片文件记录就是那个文件。搅拌机和表情包制作共用。不要在持有别的锁时调用。

use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::state::AppState;

/// 不对应历史记录（从别的应用拖进来的）：只看文件在不在
pub const NO_RECORD: i64 = 0;

/// 原记录还在、文件也在时返回原图路径。`file` 为 None 时是位图记录的 `images/<id>.png`。
pub fn path(state: &AppState, record_id: i64, file: Option<&Path>) -> Option<PathBuf> {
    let live =
        record_id == NO_RECORD || matches!(state.store().live_summary(record_id), Ok(Some(_)));
    let p = file.map(Path::to_path_buf).unwrap_or_else(|| {
        state
            .data_dir
            .join("images")
            .join(format!("{record_id}.png"))
    });
    (live && p.exists()).then_some(p)
}

/// 解出原图；动图（GIF、动态 WebP）只取第一帧。原记录没了、文件没了或解不开时为 None。
pub fn decode(
    state: &AppState,
    record_id: i64,
    file: Option<&Path>,
) -> Option<image::DynamicImage> {
    let p = path(state, record_id, file)?;
    image::ImageReader::open(&p)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()
}

/// 文件记录里第 `n` 个文件的路径（要是图片文件）；`n` 为 None 是位图记录，没有路径。外层 None 是找不到
fn file_of(state: &AppState, id: i64, n: Option<u32>) -> Option<Option<PathBuf>> {
    let Some(n) = n else { return Some(None) };
    let d = state.store().detail(id).ok()??;
    let p = PathBuf::from(&d.files.get(n as usize)?.path);
    xpaste_core::detect::is_image_file(&p).then_some(Some(p))
}

/// 这张参考图的记录和文件还在不在：只查记录和文件，不解码
pub fn exists(state: &AppState, id: i64, file: Option<u32>) -> bool {
    file_of(state, id, file)
        .and_then(|f| path(state, id, f.as_deref()))
        .is_some_and(|p| p.exists())
}

/// 一张参考图的 PNG：`file` 为 None 读位图记录，否则读文件记录里第 `file` 个文件（要是图片文件）。读不到为 None。
pub fn read_record_image(state: &AppState, id: i64, file: Option<u32>) -> Option<Vec<u8>> {
    read_record_image_within(state, id, file, None)
}

/// 同 `read_record_image`；`short` 给了时把短边缩到不超过它（比例不变）
pub fn read_record_image_within(
    state: &AppState,
    id: i64,
    file: Option<u32>,
    short: Option<u32>,
) -> Option<Vec<u8>> {
    let file = file_of(state, id, file)?;
    let mut img = decode(state, id, file.as_deref())?;
    if let Some(short) = short.filter(|&s| s > 0 && s < img.width().min(img.height())) {
        let k = f64::from(short) / f64::from(img.width().min(img.height()));
        let side = |v: u32| ((f64::from(v) * k).round() as u32).max(1);
        img = img.resize_exact(
            side(img.width()),
            side(img.height()),
            image::imageops::FilterType::Triangle,
        );
    }
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两帧（红、蓝）的 GIF 只取第一帧
    #[test]
    fn gif_reads_first_frame() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};
        let dir = std::env::temp_dir().join(format!("xpaste-recimg-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.gif");
        {
            let f = std::fs::File::create(&p).unwrap();
            let mut enc = GifEncoder::new(f);
            for c in [[255, 0, 0, 255], [0, 0, 255, 255]] {
                let img = RgbaImage::from_pixel(2, 2, Rgba(c));
                enc.encode_frame(Frame::from_parts(
                    img,
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
            }
        }
        let state = AppState::for_test();
        let img = decode(&state, NO_RECORD, Some(&p)).unwrap().to_rgba8();
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
