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

/// 写 `memes/<uuid>.<ext>` 并存进表情包库。返回 id 和这个文件的剪切板格式（文件路径，macOS 的 GIF 另附一份数据）。
/// 进库失败时删掉刚写的文件
pub fn put(
    state: &AppState,
    bytes: &[u8],
    ext: &str,
    (width, height): (u32, u32),
    info: MemeInfo,
    search: String,
    scene: String,
) -> Result<(i64, Vec<Flavor>), String> {
    let dir = state.data_dir.join("memes");
    let path = dir.join(format!("{}.{ext}", crate::meme_gen::uuid()));
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, bytes)) {
        log::warn!("表情包 写文件失败：{e}");
        return Err("存进表情包失败".into());
    }
    #[allow(unused_mut)]
    let mut flavors = xpaste_platform::file_flavors(&path);
    // Mac 微信要有文件路径才收动图；另附一份 GIF 数据
    #[cfg(target_os = "macos")]
    if ext == "gif" {
        flavors.push(Flavor {
            item: 0,
            format: "com.compuserve.gif".into(),
            data: bytes.to_vec(),
        });
    }
    let meme = NewMeme {
        file: path.clone(),
        info,
        search,
        scene,
        width,
        height,
        bytes: bytes.len() as u64,
    };
    match state
        .store()
        .add_meme(meme, &flavors, crate::clock::now_ms())
    {
        Ok(id) => {
            // 面板网格的格子用静态缩略图；动图只在预览里播
            if state
                .enrich
                .send(crate::enrich::Job::MemeThumb { id, path })
                .is_err()
            {
                log::warn!("补全线程已退出");
            }
            Ok((id, flavors))
        }
        Err(e) => {
            log::warn!("表情包 进库失败：{e}");
            let _ = std::fs::remove_file(&path);
            Err("存进表情包失败".into())
        }
    }
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

/// 存进表情包库（做好的）；`copy` 时再按自身写入放进剪切板，不进历史
pub fn store(
    state: &AppState,
    last: &Last,
    f: Final,
    copy: Option<Write<'_>>,
) -> Result<i64, String> {
    let e = last.take(f.token).ok_or("成品已过期，再点一次")?;
    let name: String = f.name.trim().chars().take(12).collect();
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
    };
    let (id, flavors) = put(
        state,
        &e.bytes,
        e.ext,
        (e.side, e.side),
        info,
        search,
        f.scene,
    )?;
    log::info!(
        "表情包 存成品 {id} {} {} 字节{}",
        e.ext,
        e.bytes.len(),
        if copy.is_some() { " 并复制" } else { "" }
    );
    if let Some(w) = copy {
        crate::commands::records::write_marked(state, || w(&flavors))
            .map_err(|_| "存进表情包了，但没能放进剪切板，再点一次复制")?;
    }
    Ok(id)
}

/// 和上次存的一样：把库里那一个按自身写入放进剪切板（不进历史）。不在了时返回 "gone"
pub fn copy(state: &AppState, id: i64, write: Write<'_>) -> Result<(), String> {
    let flavors = crate::commands::records::precheck(state, id).map_err(|_| "gone")?;
    crate::commands::records::write_marked(state, || write(&flavors))
        .map(drop)
        .map_err(|e| e.to_string())
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
            Some(&mut |f: &[xpaste_core::Flavor]| {
                assert!(state.writing.load(Ordering::SeqCst));
                seen = f.iter().map(|f| f.format.clone()).collect();
                Ok(42)
            }),
        )
        .unwrap();
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
                None,
            )
            .is_err()
        );
    }

    #[test]
    fn orphans_cleaned() {
        // 别的测试共用 for_test 的数据目录，这里另开一个，免得删到它们的文件
        let mut state = AppState::for_test();
        state.data_dir = state
            .data_dir
            .join(format!("orphans-{}", crate::meme_gen::uuid()));
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
            None,
        )
        .unwrap();
        file(&sk.join(&used_sketch));
        file(&sk.join(&draft_sketch));
        file(&sk.join("c.png"));
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
        assert!(!sk.join("c.png").exists());
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
