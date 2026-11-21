//! 表情包成品：编码（GIF / PNG，只留最近一次的结果）、存进表情包库、复制（按自身写入写剪切板，不进历史），
//! 草稿（`memes/draft.json`），接着改时读出做法，以及启动时清掉 `memes/` 下没人指向的文件。

use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use xpaste_core::{Flavor, MemeInfo, NewMeme};
use xpaste_platform::PlatformError;

use crate::meme_gif::{encode_gif, encode_png};
use crate::state::AppState;

/// 草稿：`memes/` 下，只留最后一份
pub const DRAFT: &str = "draft.json";
/// 成品边长上限、帧数上限
const SIDE_MAX: u32 = 240;
const FRAMES_MAX: usize = 32;

/// 页面发来的帧头：边长、帧间隔、颜色数、帧数
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    side: u32,
    delay_ms: u16,
    colors: u16,
    count: usize,
}

/// 编码好的成品（还没存），不派生 Debug
pub struct Encoded {
    token: u64,
    pub bytes: Vec<u8>,
    ext: &'static str,
    side: u32,
}

/// 最近一次的编码结果：导出时逐级试，只有最后用上的那一次才存
#[derive(Default)]
pub struct Last(Mutex<Option<Encoded>>);

impl Last {
    /// 取走这次编码的结果；已被更新的一次顶掉、已经存过时为 None
    pub fn take(&self, token: u64) -> Option<Encoded> {
        let mut g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        g.as_ref().filter(|e| e.token == token)?;
        g.take()
    }
}

pub static LAST: Last = Last(Mutex::new(None));
static TOKEN: AtomicU64 = AtomicU64::new(0);

#[derive(Serialize)]
pub struct EncodeOut {
    pub token: u64,
    pub bytes: u64,
}

/// 请求体：JSON 帧头 + `\n` + 各帧 RGBA 首尾相接。一帧存 PNG，多帧存 GIF；结果留在 `last` 里等存
pub fn encode(last: &Last, body: &[u8]) -> Result<EncodeOut, String> {
    let nl = body
        .iter()
        .position(|&b| b == b'\n')
        .ok_or("请求体缺少帧头")?;
    let h: Header = serde_json::from_slice(&body[..nl]).map_err(|_| "帧头读不懂")?;
    let px = &body[nl + 1..];
    let len = (h.side as usize).pow(2) * 4;
    if h.side == 0
        || h.side > SIDE_MAX
        || !(1..=FRAMES_MAX).contains(&h.count)
        || px.len() != len * h.count
    {
        return Err("帧数据不对".into());
    }
    let (bytes, ext) = if h.count == 1 {
        (encode_png(px, h.side), "png")
    } else {
        let frames: Vec<Vec<u8>> = px.chunks(len).map(<[u8]>::to_vec).collect();
        (encode_gif(&frames, h.side, h.delay_ms, h.colors), "gif")
    };
    let token = TOKEN.fetch_add(1, Ordering::SeqCst) + 1;
    log::info!(
        "表情包 编码 {ext} {} 帧 边长 {} {} 色 {} 字节",
        h.count,
        h.side,
        h.colors,
        bytes.len()
    );
    let out = EncodeOut {
        token,
        bytes: bytes.len() as u64,
    };
    *last.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Encoded {
        token,
        bytes,
        ext,
        side: h.side,
    });
    Ok(out)
}

/// 写 `memes/<uuid>.<ext>` 并存进表情包库，返回 id。进库失败时删掉刚写的文件
pub fn put(
    state: &AppState,
    bytes: &[u8],
    ext: &str,
    (width, height): (u32, u32),
    info: MemeInfo,
    search: String,
    scene: String,
) -> Result<i64, String> {
    let dir = state.data_dir.join("memes");
    let file = format!("{}.{ext}", crate::meme_gen::uuid());
    let path = dir.join(&file);
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)) {
        log::warn!("表情包 写文件失败：{e}");
        return Err("存进表情包失败".into());
    }
    let meme = NewMeme {
        file,
        info,
        search,
        scene,
        width,
        height,
        bytes: bytes.len() as u64,
    };
    match state.store().add_meme(meme, crate::clock::now_ms()) {
        Ok(id) => {
            // 面板网格的格子用静态缩略图；动图只在预览里播
            if state
                .enrich
                .send(crate::enrich::Job::MemeThumb { id, path })
                .is_err()
            {
                log::warn!("补全线程已退出");
            }
            Ok(id)
        }
        Err(e) => {
            log::warn!("表情包 进库失败：{e}");
            let _ = std::fs::remove_file(&path);
            Err("存进表情包失败".into())
        }
    }
}

/// 表情包文件的剪切板格式，粘贴、复制时现拼（库里不存，数据目录换了也对）：文件路径，macOS 的 GIF 另附一份数据。
/// 读不到 GIF 时只给文件路径
pub fn flavors(path: &Path) -> Vec<Flavor> {
    #[allow(unused_mut)]
    let mut flavors = xpaste_platform::file_flavors(path);
    // Mac 微信要有文件路径才收动图；另附一份 GIF 数据
    #[cfg(target_os = "macos")]
    if path.extension().is_some_and(|e| e == "gif")
        && let Ok(data) = std::fs::read(path)
    {
        flavors.push(Flavor {
            item: 0,
            format: "com.compuserve.gif".into(),
            data,
        });
    }
    flavors
}

/// 参与搜索的几段：想表达什么、文字、情绪名、风格名。带用户写的话，不派生 Debug
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchParts {
    pub idea: String,
    pub caption: String,
    pub emotions: Vec<String>,
    pub style_name: String,
}

/// 一个做好的成品：哪次编码、名字、做法
pub struct Final {
    pub token: u64,
    pub name: String,
    pub scene: String,
    pub parts: SearchParts,
}

/// 写剪切板的函数（测试里换成假的）
pub type Write<'a> = &'a mut dyn FnMut(&[Flavor]) -> Result<u64, PlatformError>;
/// 贴到原来的应用：关窗口、交还前台、模拟粘贴，返回贴过去没有（测试里换成假的）
pub type Paste<'a> = &'a mut dyn FnMut() -> bool;

/// 已进库、但没能放进剪切板时给页面的话
pub const CLIP_FAILED: &str = "存进表情包了，但没能放进剪切板，再点一次复制";
/// 库里那一个已经不在了（页面照常存一个新的）
pub const GONE: &str = "gone";

/// 做完之后去哪：贴到原来的应用、只放剪切板、只进库
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Then {
    Paste,
    Copy,
    Lib,
}

/// 做完的结果：库里的 id；放剪切板失败时带提示（这时已进库，窗口不关）；贴过去没有
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreOut {
    pub id: i64,
    pub clip_error: Option<&'static str>,
    pub pasted: bool,
}

/// 存进表情包库（做好的），返回 id
pub fn store(state: &AppState, last: &Last, f: Final) -> Result<i64, String> {
    let e = last.take(f.token).ok_or("成品已过期，再点一次")?;
    let name: String = f.name.trim().chars().take(16).collect();
    let name = if name.is_empty() {
        "表情包".to_owned()
    } else {
        name
    };
    let p = &f.parts;
    let emotions: Vec<&str> = p.emotions.iter().map(String::as_str).collect();
    let search = xpaste_pro::meme_search_text(&name, &p.idea, &p.caption, &emotions, &p.style_name);
    let info = MemeInfo {
        source: "final".into(),
        name,
        style: None,
        gif: e.ext == "gif",
        set: None,
    };
    let id = put(
        state,
        &e.bytes,
        e.ext,
        (e.side, e.side),
        info,
        search,
        f.scene,
    )?;
    log::info!("表情包 存成品 {id} {} {} 字节", e.ext, e.bytes.len());
    Ok(id)
}

/// 库里的 `id`（刚存的，或和上次存的一样时那一个）按 `then` 往下走：复制、贴过去时按自身写入放进剪切板（不进历史），
/// 放好了再 `paste`；只进库时不碰剪切板。放剪切板失败时带提示、不去粘贴。库里那个不在了时返回 `GONE`
pub fn deliver(
    state: &AppState,
    id: i64,
    then: Then,
    write: Write<'_>,
    paste: Paste<'_>,
) -> Result<StoreOut, &'static str> {
    let flavors = crate::commands::records::precheck(state, id).map_err(|_| GONE)?;
    let mut out = StoreOut {
        id,
        clip_error: None,
        pasted: false,
    };
    if then == Then::Lib {
        return Ok(out);
    }
    if crate::commands::records::write_marked(state, || write(&flavors)).is_err() {
        out.clip_error = Some(CLIP_FAILED);
        return Ok(out);
    }
    out.pasted = then == Then::Paste && paste();
    Ok(out)
}

fn draft_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join("memes").join(DRAFT)
}

/// 草稿；没有或坏了时为 None
pub fn draft_get(data_dir: &Path) -> Option<String> {
    std::fs::read_to_string(draft_path(data_dir))
        .ok()
        .filter(|s| xpaste_pro::meme_scene_ok(s))
}

/// 留一份草稿（替换旧的），None 时丢掉
pub fn draft_set(data_dir: &Path, scene: Option<&str>) {
    let p = draft_path(data_dir);
    let r = match scene {
        Some(s) => p
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&p, s)),
        None => std::fs::remove_file(&p).or_else(|e| {
            if e.kind() == ErrorKind::NotFound {
                Ok(())
            } else {
                Err(e)
            }
        }),
    };
    match r {
        Ok(()) => log::info!(
            "表情包 草稿{}",
            if scene.is_some() {
                "已留"
            } else {
                "已丢掉"
            }
        ),
        Err(e) => log::warn!("表情包 写草稿失败：{e}"),
    }
}

/// 接着改：做法坏了时，AI 出的只带它自己，做好的当没有做法（空白打开）。
/// AI 原图的做法里 `results` 为空表示“结果就是这一张”，换成它的 id
pub fn scene_for_open(scene: Option<String>, id: i64, ai: bool) -> Option<String> {
    let Some(s) = scene.filter(|s| xpaste_pro::meme_scene_ok(s)) else {
        return ai.then(|| crate::meme_gen::scene_json(vec![], "", "q", &[id]));
    };
    let mut v: serde_json::Value = serde_json::from_str(&s).ok()?;
    if v["look"] == "ai" && v["results"] == serde_json::json!([]) {
        v["results"] = serde_json::json!([id]);
        return Some(v.to_string());
    }
    Some(s)
}

/// 启动时清掉 `memes/`、`memes/sketch/` 下没有表情包、也没有草稿指向的文件（删了之后的撤销期已过，
/// 窗口里画了没用上的草图）。文件名是随机的，按名字在记录和草稿里找
pub fn clean_orphans(state: &AppState) {
    let refs = match state.store().meme_refs() {
        Ok(r) => r,
        Err(e) => return log::warn!("表情包 启动清理：读库失败：{e}"),
    };
    let dir = state.data_dir.join("memes");
    let draft = std::fs::read_to_string(dir.join(DRAFT)).unwrap_or_default();
    let used = |name: &str| draft.contains(name) || refs.iter().any(|r| r.contains(name));
    let mut n = 0;
    for d in [dir.clone(), dir.join("sketch")] {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name == DRAFT || !p.is_file() || used(name) {
                continue;
            }
            match std::fs::remove_file(&p) {
                Ok(()) => n += 1,
                Err(e) => log::debug!("表情包 启动清理：删文件失败：{e}"),
            }
        }
    }
    if n > 0 {
        log::info!("表情包 启动清理：删掉 {n} 个没人指向的文件");
    }
}

/// 测试用：库里放一个做好的表情包（文件写在数据目录的 `memes/` 下），返回 id
#[cfg(test)]
pub fn test_meme(state: &AppState) -> i64 {
    let info = MemeInfo {
        source: "final".into(),
        name: "甲".into(),
        style: None,
        gif: false,
        set: None,
    };
    put(state, b"x", "png", (1, 1), info, String::new(), "{}".into()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;
    use std::sync::atomic::Ordering;

    fn body(side: u32, delay: u16, colors: u16, frames: &[[u8; 4]]) -> Vec<u8> {
        let mut b = serde_json::json!({"side": side, "delayMs": delay, "colors": colors, "count": frames.len()})
            .to_string()
            .into_bytes();
        b.push(b'\n');
        for px in frames {
            b.extend(px.repeat((side * side) as usize));
        }
        b
    }

    fn parts() -> SearchParts {
        SearchParts {
            idea: "不想上班".into(),
            caption: "本喵不想上班".into(),
            emotions: vec![],
            style_name: String::new(),
        }
    }

    #[test]
    fn encode_gif_or_png_keeps_latest() {
        let last = Last::default();
        let a = encode(
            &last,
            &body(8, 80, 256, &[[255, 0, 0, 255], [0, 0, 255, 255]]),
        )
        .unwrap();
        assert!(last.take(a.token).unwrap().bytes.starts_with(b"GIF89a"));
        let b = encode(&last, &body(8, 80, 256, &[[255, 0, 0, 255]])).unwrap();
        let c = encode(&last, &body(8, 80, 256, &[[255, 0, 0, 255]])).unwrap();
        // 只留最近一次：旧的取不到
        assert!(last.take(b.token).is_none());
        let png = last.take(c.token).unwrap();
        assert!(png.bytes.starts_with(b"\x89PNG"));
        assert_eq!(c.bytes, png.bytes.len() as u64);
        // 字节数和帧数对不上、边长太大
        assert!(encode(&last, &body(8, 80, 256, &[])).is_err());
        let mut short = body(8, 80, 256, &[[0; 4]]);
        short.pop();
        assert!(encode(&last, &short).is_err());
        assert!(encode(&last, &body(300, 80, 256, &[[0; 4]])).is_err());
    }

    #[test]
    fn store_copy_marked_not_recorded() {
        let state = AppState::for_test();
        let last = Last::default();
        let history = |s: &AppState| {
            s.store()
                .list_summaries()
                .unwrap()
                .iter()
                .filter(|r| r.meme.is_none())
                .count()
        };
        let before = history(&state);
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255], [4, 5, 6, 255]])).unwrap();
        let mut seen = Vec::new();
        let id = store(
            &state,
            &last,
            Final {
                token: out.token,
                name: "本喵不想上班".into(),
                scene: "{}".into(),
                parts: parts(),
            },
        )
        .unwrap();
        let o = deliver(
            &state,
            id,
            Then::Copy,
            &mut |f: &[xpaste_core::Flavor]| {
                assert!(state.writing.load(Ordering::SeqCst));
                seen = f.iter().map(|f| f.format.clone()).collect();
                Ok(42)
            },
            &mut || panic!("复制不去粘贴"),
        )
        .unwrap();
        assert_eq!((o.id, o.clip_error, o.pasted), (id, None, false));
        // 库里多一个做好的，剪切板写入带自身写入标记，历史不变
        assert_eq!(state.store().meme_bytes().unwrap().0, 1);
        let m = state
            .store()
            .live_summary(id)
            .unwrap()
            .unwrap()
            .meme
            .unwrap();
        assert_eq!(
            (m.source.as_str(), m.name.as_str()),
            ("final", "本喵不想上班")
        );
        assert!(!seen.is_empty());
        #[cfg(target_os = "macos")]
        assert!(seen.iter().any(|f| f == "com.compuserve.gif"));
        assert_eq!(state.last_write_seq.load(Ordering::SeqCst), 42);
        assert!(crate::clipboard::is_self_write(42, 42, false));
        assert_eq!(history(&state), before);
        // 同一次编码不能存两遍
        assert!(
            store(
                &state,
                &last,
                Final {
                    token: out.token,
                    name: "x".into(),
                    scene: "{}".into(),
                    parts: parts(),
                },
            )
            .is_err()
        );
    }

    fn state_at(store: xpaste_core::Store, dir: &Path) -> AppState {
        AppState::new(
            xpaste_core::Settings::default(),
            store,
            std::sync::mpsc::channel().0,
            crate::ocr::Handle::new(false),
            dir.to_path_buf(),
        )
    }

    fn fin(token: u64, name: &str) -> Final {
        Final {
            token,
            name: name.into(),
            scene: "{}".into(),
            parts: parts(),
        }
    }

    /// 已进库、放剪切板失败：照样返回 id（页面记下“最后一次存的”，再点复制不会再存一个），带上提示，也不去粘贴
    #[test]
    fn store_clip_failure_still_returns_id() {
        let state = AppState::for_test();
        let last = Last::default();
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255]])).unwrap();
        let id = store(&state, &last, fin(out.token, "甲")).unwrap();
        let o = deliver(
            &state,
            id,
            Then::Paste,
            &mut |_: &[Flavor]| Err(PlatformError::Clipboard("x".into())),
            &mut || panic!("没放进剪切板不去粘贴"),
        )
        .unwrap();
        assert_eq!(
            (o.id, o.clip_error, o.pasted),
            (id, Some(CLIP_FAILED), false)
        );
        assert!(
            state
                .store()
                .live_summary(id)
                .unwrap()
                .unwrap()
                .meme
                .is_some()
        );
    }

    /// 贴过去没成（没有辅助功能权限、目标窗口没了）：已进库、已在剪切板，pasted 为 false
    #[test]
    fn paste_failure_still_in_library_and_clipboard() {
        let state = AppState::for_test();
        let last = Last::default();
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255]])).unwrap();
        let id = store(&state, &last, fin(out.token, "甲")).unwrap();
        let (mut wrote, mut tried) = (false, false);
        let o = deliver(
            &state,
            id,
            Then::Paste,
            &mut |_: &[Flavor]| {
                wrote = true;
                Ok(43)
            },
            &mut || {
                tried = true;
                false
            },
        )
        .unwrap();
        assert_eq!((o.id, o.clip_error, o.pasted), (id, None, false));
        assert!(wrote && tried);
        assert_eq!(state.last_write_seq.load(Ordering::SeqCst), 43);
        assert!(
            state
                .store()
                .live_summary(id)
                .unwrap()
                .unwrap()
                .meme
                .is_some()
        );
        // 贴过去了
        let o = deliver(&state, id, Then::Paste, &mut |_| Ok(44), &mut || true).unwrap();
        assert!(o.pasted);
        // 存进表情包：不碰剪切板
        let o = deliver(
            &state,
            id,
            Then::Lib,
            &mut |_| panic!("不碰剪切板"),
            &mut || panic!("不去粘贴"),
        )
        .unwrap();
        assert_eq!((o.clip_error, o.pasted), (None, false));
        // 库里那个不在了
        state.store().soft_delete(&[id], 5).unwrap();
        state.store().purge_all_soft_deleted().unwrap();
        assert_eq!(
            deliver(&state, id, Then::Lib, &mut |_| Ok(1), &mut || true).err(),
            Some(GONE)
        );
    }

    /// 名字按文字截，最多 16 字（同文字的上限）
    #[test]
    fn store_name_up_to_16() {
        let state = AppState::for_test();
        let last = Last::default();
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255]])).unwrap();
        let id = store(
            &state,
            &last,
            fin(out.token, "一二三四五六七八九十甲乙丙丁戊己庚"),
        )
        .unwrap();
        let m = state
            .store()
            .live_summary(id)
            .unwrap()
            .unwrap()
            .meme
            .unwrap();
        assert_eq!(m.name, "一二三四五六七八九十甲乙丙丁戊己");
    }

    /// 整个数据目录搬走后：表情包照样能粘贴（剪切板格式按新位置现拼）、能读帧、能复制，删掉时删的是新位置的文件
    #[test]
    fn meme_follows_moved_data_dir() {
        let base =
            std::env::temp_dir().join(format!("xpaste-meme-move-{}", crate::meme_gen::uuid()));
        let (a, b) = (base.join("a"), base.join("b"));
        let open = |d: &Path| state_at(xpaste_core::Store::open(d).unwrap(), d);
        let state = open(&a);
        let last = Last::default();
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255], [4, 5, 6, 255]])).unwrap();
        let id = store(&state, &last, fin(out.token, "甲")).unwrap();
        drop(state);
        std::fs::rename(&a, &b).unwrap();
        let state = open(&b);
        let d = state.store().detail(id).unwrap().unwrap();
        let file = std::path::PathBuf::from(&d.files[0].path);
        assert!(file.starts_with(&b) && file.exists());
        let f = crate::commands::records::precheck(&state, id).unwrap();
        assert!(!f.is_empty());
        #[cfg(target_os = "macos")]
        {
            let url = f.iter().find(|f| f.format == "public.file-url").unwrap();
            let url = String::from_utf8(url.data.clone()).unwrap();
            assert!(url.ends_with(file.file_name().unwrap().to_str().unwrap()));
            assert!(!url.contains("/a/memes/"));
            let gif = f.iter().find(|f| f.format == "com.compuserve.gif").unwrap();
            assert!(gif.data.starts_with(b"GIF89a"));
        }
        assert!(crate::meme_gen::frame_png(&state, id, 1).is_some());
        let o = deliver(&state, id, Then::Copy, &mut |_| Ok(1), &mut || false).unwrap();
        assert_eq!(o.clip_error, None);
        state.store().soft_delete(&[id], 5).unwrap();
        state.store().purge_all_soft_deleted().unwrap();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn orphans_cleaned() {
        // 别的测试共用 for_test 的数据目录，这里另开一个，免得删到它们的文件
        let dir = std::env::temp_dir().join(format!("xpaste-orphans-{}", crate::meme_gen::uuid()));
        let state = state_at(xpaste_core::Store::open_in_memory(&dir).unwrap(), &dir);
        let dir = state.data_dir.join("memes");
        let sk = dir.join("sketch");
        std::fs::create_dir_all(&sk).unwrap();
        let file = |p: &std::path::Path| std::fs::write(p, b"x").unwrap();
        let last = Last::default();
        let out = encode(&last, &body(8, 80, 256, &[[1, 2, 3, 255]])).unwrap();
        let used_sketch = format!("{}.png", "a".repeat(32));
        let draft_sketch = format!("{}.png", "b".repeat(32));
        let id = store(
            &state,
            &last,
            Final {
                token: out.token,
                name: "甲".into(),
                scene: format!(r#"{{"refs":[{{"id":-1,"file":null,"sketch":"{used_sketch}"}}]}}"#),
                parts: parts(),
            },
        )
        .unwrap();
        file(&sk.join(&used_sketch));
        file(&sk.join(&draft_sketch));
        file(&sk.join("zz.png"));
        file(&dir.join("orphan.gif"));
        draft_set(
            &state.data_dir,
            Some(&format!(r#"{{"sketch":"{draft_sketch}"}}"#)),
        );
        let kept = state.store().detail(id).unwrap().unwrap().files[0]
            .path
            .clone();

        clean_orphans(&state);
        assert!(std::path::Path::new(&kept).exists());
        assert!(sk.join(&used_sketch).exists());
        assert!(sk.join(&draft_sketch).exists());
        assert!(dir.join(DRAFT).exists());
        assert!(!sk.join("zz.png").exists());
        assert!(!dir.join("orphan.gif").exists());
        draft_set(&state.data_dir, None);
        assert!(!dir.join(DRAFT).exists());
    }

    #[test]
    fn open_scene_fills_ai_self() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let ai = r#"{"refs":[],"idea":"","cands":[],"cap":"","pos":"bottom","ts":"stroke","bg":"none","look":"ai","style":"q","count":"one","results":[],"pick":0,"fx":"none","tfx":"none","sfx":[],"act":"none"}"#;
        let v: serde_json::Value =
            serde_json::from_str(&scene_for_open(Some(ai.into()), 7, true).unwrap()).unwrap();
        assert_eq!(v["results"], serde_json::json!([7]));
        // 做法坏了：AI 出的就只带它自己，做好的当没有做法
        let v: serde_json::Value =
            serde_json::from_str(&scene_for_open(Some("{".into()), 7, true).unwrap()).unwrap();
        assert_eq!(
            (v["look"].as_str(), &v["results"]),
            (Some("ai"), &serde_json::json!([7]))
        );
        assert!(xpaste_pro::meme_scene_ok(&v.to_string()));
        assert_eq!(scene_for_open(Some("{".into()), 7, false), None);
        assert_eq!(scene_for_open(None, 7, false), None);
        // 做好的：原样
        let fin = ai.replace(r#""results":[]"#, r#""results":[3]"#);
        assert_eq!(scene_for_open(Some(fin.clone()), 7, false), Some(fin));
    }
}
