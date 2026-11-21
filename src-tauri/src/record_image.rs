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

/// 一张参考图的 PNG：`file` 为 None 读位图记录，否则读文件记录里第 `file` 个文件（要是图片文件）。读不到为 None。
pub fn read_record_image(state: &AppState, id: i64, file: Option<u32>) -> Option<Vec<u8>> {
    let file = match file {
        None => None,
        Some(n) => {
            let d = state.store().detail(id).ok()??;
            let p = PathBuf::from(&d.files.get(n as usize)?.path);
            if !xpaste_core::detect::is_image_file(&p) {
                return None;
            }
            Some(p)
        }
    };
    let img = decode(state, id, file.as_deref())?;
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
