//! 表情包制作的出文案与生成：读参考图、调生图接口、抠掉白底、切网格，每张结果立刻存进表情包库；
//! 另有抠图贴纸（抠主体加白边）与草图存盘。
//!
//! 生成由 AI 任务队列（`meme_tasks`）发起；取消记在队列的任务上，请求照常等它回来：
//! 结果照样进库（花了的钱不白花），但不再显示、不提示。

use std::collections::HashMap;
use std::hash::{BuildHasher, RandomState};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use image::RgbaImage;
use serde::Deserialize;
use xpaste_core::{MemeInfo, Settings};
use xpaste_pro::{MemeAct, MemeGen, MemePrompt};

use crate::imagegen::{self, Format, ImageCfg, ImgFail};
use crate::meme::RefKey;
use crate::state::AppState;
use crate::translate::Fail;

/// 晚到的结果（任务已取消）：不再显示、不提示
pub const LATE: &str = "已取消";
const GONE: &str = "原记录已删除，拿掉了一张";
const CAPTION_UNCONFIGURED: &str = "AI 接口还没配好";
/// 交给系统抠主体的图，长边上限：平台层解码不设上限，超过的不抠
const CUT_EDGE: u32 = 2048;
/// 出文案、生图最多带几张参考图（草图算一张）：张数多了费用高，有的服务商也收不下
const REFS_SENT: usize = 4;

/// 系统抠主体：返回与输入同尺寸的 RGBA，没抠到为 None
pub type Cutout = fn(&[u8]) -> Option<(u32, u32, Vec<u8>)>;

/// 页面发来的生成请求。带用户写的话，不派生 Debug。
#[derive(Deserialize, Clone)]
pub struct GenReq {
    /// one | set | set_part | act；AI 任务另有 set_parts（分 4 次重画，见 `meme_tasks`）
    pub kind: String,
    /// set_part：第几次（0–3）
    pub part: Option<u8>,
    /// act：blink | nod | wave | cry
    pub act: Option<String>,
    /// 内置风格的键名；自定义时为空
    pub style: String,
    pub custom: Option<String>,
    pub idea: String,
    pub caption: String,
    /// 页面里的草图用负数 id 占位，这里不读，草图走 `sketch`
    pub refs: Vec<RefKey>,
    /// `memes/sketch/` 下的文件名
    pub sketch: Option<String>,
    /// act：以库里哪张为底
    pub base: Option<i64>,
    /// 套编号：分 4 次重画时由队列填，4 部分共用一个；页面不传
    #[serde(skip)]
    pub set: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum GenOut {
    Done {
        ids: Vec<i64>,
    },
    /// 网格没画规整：整张图已存成一个表情包
    Irregular {
        id: i64,
    },
}

/// 生成要用到的外部：状态、抠主体（测试里换掉）、存进库后通知面板
pub struct Env<'a> {
    pub state: &'a AppState,
    pub cutout: Cutout,
    pub added: &'a (dyn Fn(i64) + Sync),
}

/// 设置里的生图接口
pub fn image_cfg(s: &Settings) -> ImageCfg {
    ImageCfg {
        format: match s.meme_image_format.as_str() {
            "ark" => Format::Ark,
            "gemini" => Format::Gemini,
            _ => Format::Openai,
        },
        base_url: s
            .meme_image_base_url
            .trim()
            .trim_end_matches('/')
            .to_owned(),
        api_key: s.meme_image_api_key.trim().to_owned(),
        model: s.meme_image_model.trim().to_owned(),
    }
}

/// 每个服务商（地址加模型 ID）上次一套 16 张画规整了没有。只在内存里，只用来提示
static GRID_OK: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);

fn provider(c: &ImageCfg) -> String {
    format!("{}\n{}", c.base_url, c.model)
}

/// 这个服务商上次的一套没画规整：在“数量”旁边提示“这个模型画网格不太稳，可能要分 4 次”
pub fn grid_shaky(c: &ImageCfg) -> bool {
    GRID_OK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&provider(c))
        == Some(&false)
}

fn act_of(s: Option<&str>) -> Option<(MemeAct, &'static str)> {
    Some(match s? {
        "blink" => (MemeAct::Blink, "眨眼"),
        "nod" => (MemeAct::Nod, "点头"),
        "wave" => (MemeAct::Wave, "挥手"),
        "cry" => (MemeAct::Cry, "哭"),
        _ => return None,
    })
}

/// 风格的显示名（存进库、参与搜索）与写日志用的名字（内置键名或“自定义”）
fn style_names(req: &GenReq) -> (String, String) {
    match xpaste_pro::meme_styles()
        .iter()
        .find(|s| s.key == req.style)
    {
        Some(s) => (s.name.to_owned(), s.key.to_owned()),
        None => (
            format!("自定义：{}", req.custom.as_deref().unwrap_or("").trim()),
            "自定义".into(),
        ),
    }
}

pub(crate) fn uuid() -> String {
    let s = RandomState::new();
    format!("{:016x}{:016x}", s.hash_one(0u8), s.hash_one(1u8))
}

pub(crate) fn sketch_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let plain = Path::new(name).file_name().is_some_and(|f| f == name) && name.ends_with(".png");
    plain.then(|| data_dir.join("memes").join("sketch").join(name))
}

/// 草图存成 `memes/sketch/<uuid>.png`，返回文件名（生成、出文案时带回来）
pub fn save_sketch(data_dir: &Path, png: &[u8]) -> Result<String, String> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("草图不是 PNG".into());
    }
    let name = format!("{}.png", uuid());
    let p = sketch_path(data_dir, &name).expect("自己起的名字");
    let written = p
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&p, png));
    if let Err(e) = written {
        log::warn!("表情包 存草图失败：{e}");
        return Err("存草图失败".into());
    }
    log::info!("表情包 存草图 {} 字节", png.len());
    Ok(name)
}

/// 参考图读得出、编不了码：原因写成 `unreadable:<id>:<文件序号>`（后半同页面的 refKey），页面据此拿掉这一张
pub const UNREADABLE: &str = "unreadable:";
fn encode_one(r: RefKey, png: &[u8]) -> Result<Vec<u8>, String> {
    imagegen::encode_ref(png).ok_or_else(|| {
        let file = r.file.map_or_else(String::new, |f| f.to_string());
        format!("{UNREADABLE}{}:{file}", r.id)
    })
}

/// 参考图（草图排最前，页面的负数 id 跳过，最多 `REFS_SENT` 张），各自编码好。原记录没了时返回给用户看的原因
fn read_refs(
    state: &AppState,
    refs: &[RefKey],
    sketch: Option<&str>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut out = Vec::new();
    if let Some(name) = sketch {
        let png = sketch_path(&state.data_dir, name)
            .and_then(|p| std::fs::read(p).ok())
            .ok_or("草图读不出来，重新画一张再试")?;
        out.push(imagegen::encode_ref(&png).ok_or("草图读不出来，重新画一张再试")?);
    }
    for r in refs.iter().filter(|r| r.id >= 0) {
        if out.len() >= REFS_SENT {
            break;
        }
        let png = crate::record_image::read_record_image(state, r.id, r.file).ok_or(GONE)?;
        out.push(encode_one(*r, &png)?);
    }
    Ok(out)
}

/// 生成一张 / 一套 / 一套里的一部分 / AI 动作。结果立刻进库。
pub async fn generate(env: &Env<'_>, cfg: &ImageCfg, req: GenReq) -> Result<GenOut, String> {
    let act = act_of(req.act.as_deref());
    let what = match (req.kind.as_str(), act) {
        ("one", _) => MemeGen::One,
        ("set", _) => MemeGen::Set,
        ("set_part", _) => MemeGen::SetPart(req.part.unwrap_or(0).min(3)),
        ("act", Some((a, _))) if req.base.is_some() => MemeGen::Act(a),
        _ => return Err("不认识的生成方式".into()),
    };
    if imagegen::unconfigured(cfg) {
        return Err(imagegen::reason(&ImgFail::Base(Fail::Config)));
    }
    // AI 动作只拿库里那张当参考图
    let refs = match req.base {
        Some(base) if matches!(what, MemeGen::Act(_)) => {
            let png = crate::record_image::read_record_image(env.state, base, Some(0))
                .ok_or("这张图已经不在表情包里了")?;
            vec![imagegen::encode_ref(&png).ok_or("这张图已经不在表情包里了")?]
        }
        _ => read_refs(env.state, &req.refs, req.sketch.as_deref())?,
    };
    let has_sketch = req.sketch.is_some() && !matches!(what, MemeGen::Act(_));
    let prompt = xpaste_pro::meme_image_prompt(&MemePrompt {
        style_key: (!req.style.is_empty()).then_some(req.style.as_str()),
        custom_style: req.custom.as_deref(),
        idea: &req.idea,
        caption: &req.caption,
        r#gen: what,
        has_photo: refs.len() > usize::from(has_sketch),
        has_sketch,
    });
    let (style_name, style_log) = style_names(&req);
    let started = Instant::now();
    let r = imagegen::generate(cfg, &prompt, &refs).await;
    log::info!(
        "表情包 生成 {} 参考图 {} 张 {} 字节 风格 {style_log} {} 耗时 {} ms",
        req.kind,
        refs.len(),
        refs.iter().map(Vec::len).sum::<usize>(),
        if r.is_ok() { "成功" } else { "失败" },
        started.elapsed().as_millis()
    );
    let png = match r {
        Ok(p) => p,
        Err(e) => return Err(imagegen::reason(&e)),
    };
    // AI 动作的名字带上底图的情绪名：读库在这里做，抠图、切格、编码挪到阻塞线程上
    let base = match what {
        MemeGen::Act(_) => req.base.and_then(|b| {
            env.state
                .store()
                .live_summary(b)
                .ok()
                .flatten()?
                .meme
                .map(|m| m.name)
        }),
        _ => None,
    };
    let (cutout, act_name) = (env.cutout, act.map(|a| a.1));
    let out = tauri::async_runtime::spawn_blocking(move || {
        cut_result(cutout, &png, what, act_name, base)
    })
    .await
    .unwrap_or_else(|_| Err(imagegen::reason(&ImgFail::Base(Fail::Bad))))
    .and_then(|c| save_cut(env, cfg, &req, what, &style_name, c));
    match &out {
        Ok(GenOut::Done { ids }) => log::info!("表情包 进库 {ids:?}"),
        Ok(GenOut::Irregular { id }) => log::info!("表情包 网格没画规整 进库 {id}"),
        Err(_) => {}
    }
    out
}

/// 切好、编好码、等着存的一张：字节、扩展名、尺寸、名字、情绪名
struct Piece {
    bytes: Vec<u8>,
    ext: &'static str,
    size: (u32, u32),
    name: String,
    emotions: Vec<&'static str>,
}

/// 切开的结果：一张或按网格切开的几张；网格没画规整时是整张
enum Cut {
    Done(Vec<Piece>),
    Whole(Piece),
}

/// 收到的图：抠白底、按种类切开、编码（重活，在阻塞线程上跑）。`base` 是 AI 动作底图的名字
fn cut_result(
    cutout: Cutout,
    png: &[u8],
    what: MemeGen,
    act_name: Option<&str>,
    base: Option<String>,
) -> Result<Cut, String> {
    let img = image::load_from_memory(png)
        .map_err(|_| imagegen::reason(&ImgFail::Base(Fail::Bad)))?
        .to_rgba8();
    let (img, cut) = clear_white(cutout, png, img);
    let (w, h) = img.dimensions();
    let piece = |img: &RgbaImage, name: &str, emotions: Vec<&'static str>| Piece {
        bytes: png_of(img),
        ext: "png",
        size: img.dimensions(),
        name: name.into(),
        emotions,
    };
    let emo = xpaste_pro::meme_emotions();
    let whole = || Cut::Whole(piece(&img, "表情包", vec![]));
    let cells = |n: u32, names: &[&'static str]| {
        let Some(boxes) = xpaste_pro::meme_split_grid(img.as_raw(), w, h, n) else {
            return whole();
        };
        Cut::Done(
            boxes
                .iter()
                .zip(names)
                .map(|(&[x, y, cw, ch], &name)| {
                    let cell = image::imageops::crop_imm(&img, x, y, cw, ch).to_image();
                    piece(&cell, name, vec![name])
                })
                .collect(),
        )
    };
    Ok(match what {
        MemeGen::One => Cut::Done(vec![piece(&img, "表情包", vec![])]),
        MemeGen::Set => cells(4, &emo),
        MemeGen::SetPart(k) => {
            let k = usize::from(k) * 4;
            cells(2, &emo[k..k + 4])
        }
        MemeGen::Act(_) => {
            let Some(boxes) = xpaste_pro::meme_split_grid(img.as_raw(), w, h, 2) else {
                return Ok(whole());
            };
            let (side, frames) = act_frames(&img, &boxes, cut);
            let act = act_name.unwrap_or_default();
            let name = match base.filter(|n| emo.contains(&n.as_str())) {
                Some(e) => format!("{e} · {act}"),
                None => act.to_owned(),
            };
            let emotions = emo
                .iter()
                .copied()
                .filter(|e| name.starts_with(e))
                .collect();
            Cut::Done(vec![Piece {
                bytes: crate::meme_gif::encode_gif(&frames, side, 200, 256),
                ext: "gif",
                size: (side, side),
                name,
                emotions,
            }])
        }
    })
}

/// 切好的存进库；一套 16 张时记下这个服务商这次画规整了没有
fn save_cut(
    env: &Env<'_>,
    cfg: &ImageCfg,
    req: &GenReq,
    what: MemeGen,
    style_name: &str,
    cut: Cut,
) -> Result<GenOut, String> {
    // 一套（含分 4 次重画的一部分）切开的几张共用一个套编号；整张、单张、AI 动作没有
    let set = match (what, &cut) {
        (MemeGen::Set, Cut::Done(_)) => Some(uuid()),
        (MemeGen::SetPart(_), Cut::Done(_)) => Some(req.set.clone().unwrap_or_else(uuid)),
        _ => None,
    };
    let save = |p: &Piece| save_meme(env, req, p, set.clone(), style_name);
    if what == MemeGen::Set {
        GRID_OK
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(provider(cfg), matches!(cut, Cut::Done(_)));
    }
    match cut {
        Cut::Done(ps) => ps
            .iter()
            .map(save)
            .collect::<Result<Vec<_>, _>>()
            .map(|ids| GenOut::Done { ids }),
        Cut::Whole(p) => save(&p).map(|id| GenOut::Irregular { id }),
    }
}

/// macOS 14+ 用系统抠主体去掉白底；抠不出来、图太大时保留原图。第二项为抠过没有
fn clear_white(cutout: Cutout, png: &[u8], img: RgbaImage) -> (RgbaImage, bool) {
    if img.width().max(img.height()) > CUT_EDGE {
        return (img, false);
    }
    match cutout(png).and_then(|(w, h, rgba)| RgbaImage::from_raw(w, h, rgba)) {
        Some(c) => (c, true),
        None => (img, false),
    }
}

/// AI 动作的 4 帧：按 2 × 2 的格子（不是各格的外接框，免得帧间跳位置）切开，居中补成同样大的正方形。
/// 背景抠掉了就补透明，否则补白
fn act_frames(img: &RgbaImage, boxes: &[[u32; 4]], cut: bool) -> (u32, Vec<Vec<u8>>) {
    let (w, h) = img.dimensions();
    // 格线取左右（上下）两格外接框之间的中点
    let mid = |a: &[usize], b: &[usize], pos: usize, len: usize| {
        let end = a
            .iter()
            .map(|&i| boxes[i][pos] + boxes[i][pos + 2])
            .max()
            .unwrap_or(0);
        let start = b.iter().map(|&i| boxes[i][pos]).min().unwrap_or(len as u32);
        ((end + start) / 2).clamp(1, len as u32 - 1)
    };
    let xm = mid(&[0, 2], &[1, 3], 0, w as usize);
    let ym = mid(&[0, 1], &[2, 3], 1, h as usize);
    let rects = [
        [0, 0, xm, ym],
        [xm, 0, w - xm, ym],
        [0, ym, xm, h - ym],
        [xm, ym, w - xm, h - ym],
    ];
    let side = rects.iter().map(|r| r[2].max(r[3])).max().unwrap_or(1);
    let bg = if cut {
        [0, 0, 0, 0]
    } else {
        [255, 255, 255, 255]
    };
    let frames = rects
        .iter()
        .map(|&[x, y, cw, ch]| {
            let mut f = RgbaImage::from_pixel(side, side, image::Rgba(bg));
            let cell = image::imageops::crop_imm(img, x, y, cw, ch).to_image();
            image::imageops::replace(
                &mut f,
                &cell,
                i64::from((side - cw) / 2),
                i64::from((side - ch) / 2),
            );
            f.into_raw()
        })
        .collect();
    (side, frames)
}

fn png_of(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .expect("写进内存不会失败");
    out
}

/// AI 原图的做法（字段同页面的做法，`接着改`时照它恢复）：画面是 AI，文字和四类动效都空着。
/// 约定：`results` 为空数组、`look` 为 "ai" 表示“AI 结果就是这一张自己”（进库前还不知道自己的 id），
/// 读做法的一方把它换成这个表情包的 id。参考图是历史记录的 {id, file}；草图排最前，
/// 写成 {id: -1, file: null, sketch: "<memes/sketch/ 下的文件名>"}（负数 id 同页面里草图的占位）
fn ai_scene(req: &GenReq) -> String {
    let style = match req.custom.as_deref() {
        Some(c) if req.style.is_empty() => format!("自定义：{}", c.trim()),
        _ => req.style.clone(),
    };
    let sketch = req
        .sketch
        .iter()
        .map(|s| serde_json::json!({"id": -1, "file": null, "sketch": s}));
    let refs: Vec<serde_json::Value> = sketch
        .chain(
            req.refs
                .iter()
                .filter(|r| r.id >= 0)
                .map(|r| serde_json::json!({"id": r.id, "file": r.file})),
        )
        .collect();
    scene_json(refs, req.idea.trim(), &style, &[])
}

/// AI 画面、文字和四类动效都空着的做法
pub(crate) fn scene_json(
    refs: Vec<serde_json::Value>,
    idea: &str,
    style: &str,
    results: &[i64],
) -> String {
    serde_json::json!({
        "refs": refs, "idea": idea, "cands": [], "cap": "", "pos": "bottom", "ts": "stroke",
        "bg": "none", "look": "ai", "style": style, "count": "one", "results": results, "pick": 0,
        "fx": "none", "tfx": "none", "sfx": [], "act": "none",
    })
    .to_string()
}

/// 库里这几个表情包的名字（AI 出的一套是各格的情绪名）；读不到的为空串
pub fn names(state: &AppState, ids: &[i64]) -> Vec<String> {
    let store = state.store();
    ids.iter()
        .map(|&id| {
            store
                .live_summary(id)
                .ok()
                .flatten()
                .and_then(|s| s.meme)
                .map(|m| m.name)
                .unwrap_or_default()
        })
        .collect()
}

/// 存一个 AI 出的表情包，通知面板
fn save_meme(
    env: &Env<'_>,
    req: &GenReq,
    p: &Piece,
    set: Option<String>,
    style_name: &str,
) -> Result<i64, String> {
    let search =
        xpaste_pro::meme_search_text(&p.name, &req.idea, &req.caption, &p.emotions, style_name);
    let info = MemeInfo {
        source: "ai".into(),
        name: p.name.clone(),
        style: Some(style_name.into()),
        gif: p.ext == "gif",
        set,
    };
    let id = crate::meme_store::put(
        env.state,
        &p.bytes,
        p.ext,
        p.size,
        info,
        search,
        ai_scene(req),
    )?;
    (env.added)(id);
    Ok(id)
}

/// 出文案：用翻译里配置的 AI 接口，带上前 4 张参考图（草图在最前），“换一批”时带上刚才给过的
pub async fn captions(
    state: &AppState,
    cfg: &crate::translate::Cfg,
    idea: &str,
    refs: &[RefKey],
    sketch: Option<&str>,
    avoid: &[String],
) -> Result<Vec<String>, String> {
    if cfg.base_url.trim().is_empty() || cfg.model.trim().is_empty() {
        return Err(CAPTION_UNCONFIGURED.into());
    }
    let images: Vec<String> = read_refs(state, refs, sketch)?
        .iter()
        .map(|b| {
            let mime = if b.starts_with(&[0xFF, 0xD8]) {
                "image/jpeg"
            } else {
                "image/png"
            };
            format!("data:{mime};base64,{}", B64.encode(b))
        })
        .collect();
    let body = xpaste_pro::meme_caption_body(cfg.model.trim(), idea, &images, avoid);
    let started = Instant::now();
    let r = crate::translate::chat_stream(&cfg.base_url, &cfg.api_key, &body, None, &mut |_| true)
        .await;
    let list = r
        .as_ref()
        .map(|t| xpaste_pro::meme_parse_captions(t))
        .unwrap_or_default();
    log::info!(
        "表情包 出文案 参考图 {} 张 {} 字节 换一批 {} {} 条 {} 耗时 {} ms",
        images.len(),
        images.iter().map(String::len).sum::<usize>(),
        !avoid.is_empty(),
        list.len(),
        r.as_ref().map_or_else(|f| f.tag(), |_| "成功".into()),
        started.elapsed().as_millis()
    );
    match r {
        Err(f) => Err(crate::mixer::reason(&f, false)),
        Ok(_) if list.is_empty() => Err(crate::mixer::reason(&Fail::Bad, false)),
        Ok(_) => Ok(list),
    }
}

/// 抠图贴纸：抠出主体，外加一圈白边（宽为边长的 2.5%），背景透明。没抠出主体时为 None
pub fn sticker(cutout: Cutout, png: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(png).ok()?;
    let bounded;
    let png = if img.width().max(img.height()) > CUT_EDGE {
        bounded = png_of(
            &img.resize(CUT_EDGE, CUT_EDGE, image::imageops::FilterType::Triangle)
                .to_rgba8(),
        );
        &bounded
    } else {
        png
    };
    let (w, h, rgba) = cutout(png)?;
    let cut = RgbaImage::from_raw(w, h, rgba)?;
    Some(png_of(&white_border(&cut, w.max(h) as f32 * 0.025)))
}

/// 主体（alpha ≥ 128）外 `r` 像素内铺白，边缘抗锯齿，主体叠在上面
fn white_border(cut: &RgbaImage, r: f32) -> RgbaImage {
    let (w, h) = (cut.width() as usize, cut.height() as usize);
    // 每个像素到同一行最近主体像素的横向距离
    let mut hd = vec![u32::MAX; w * h];
    for y in 0..h {
        let row = &mut hd[y * w..(y + 1) * w];
        let inside = |x: usize| cut.as_raw()[(y * w + x) * 4 + 3] >= 128;
        let mut last = None;
        for (x, d) in row.iter_mut().enumerate() {
            if inside(x) {
                last = Some(x);
            }
            if let Some(l) = last {
                *d = (x - l) as u32;
            }
        }
        last = None;
        for x in (0..w).rev() {
            if inside(x) {
                last = Some(x);
            }
            if let Some(l) = last {
                row[x] = row[x].min((l - x) as u32);
            }
        }
    }
    let ri = r.ceil() as i64;
    let mut out = RgbaImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let mut best = f32::MAX;
            for dy in -ri..=ri {
                let yy = y as i64 + dy;
                if !(0..h as i64).contains(&yy) {
                    continue;
                }
                let d = hd[yy as usize * w + x];
                if d != u32::MAX {
                    best = best.min((d as f32).powi(2) + (dy * dy) as f32);
                }
            }
            let white = (r + 0.5 - best.sqrt()).clamp(0.0, 1.0);
            let p = cut.get_pixel(x as u32, y as u32).0;
            let sa = f32::from(p[3]) / 255.0;
            let a = sa + white * (1.0 - sa);
            let px = if a <= 0.0 {
                [0, 0, 0, 0]
            } else {
                let c =
                    |s: u8| ((f32::from(s) * sa + 255.0 * white * (1.0 - sa)) / a).round() as u8;
                [c(p[0]), c(p[1]), c(p[2]), (a * 255.0).round() as u8]
            };
            out.put_pixel(x as u32, y as u32, image::Rgba(px));
        }
    }
    out
}

/// 库里一个表情包的第 `n` 帧（PNG）：AI 动作的 GIF 给页面预览用。不是动图时只有第 0 帧
pub fn frame_png(state: &AppState, id: i64, n: usize) -> Option<Vec<u8>> {
    use image::AnimationDecoder as _;
    let file = state.store().detail(id).ok()??.files.into_iter().next()?;
    if file.ext != "gif" {
        return (n == 0)
            .then(|| crate::record_image::read_record_image(state, id, Some(0)))
            .flatten();
    }
    crate::record_image::path(state, id, Some(Path::new(&file.path)))?;
    let f = std::io::BufReader::new(std::fs::File::open(&file.path).ok()?);
    let frame = image::codecs::gif::GifDecoder::new(f)
        .ok()?
        .into_frames()
        .nth(n)?
        .ok()?;
    Some(png_of(frame.buffer()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::imagegen::tests::{b64, fake, json_reply};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use xpaste_core::NewMeme;

    fn run<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    fn cfg(base: &str) -> ImageCfg {
        ImageCfg {
            format: Format::Openai,
            base_url: base.into(),
            api_key: "k".into(),
            model: "m".into(),
        }
    }

    fn req(v: serde_json::Value) -> GenReq {
        let mut base = json!({"kind": "one", "style": "q", "idea": "", "caption": "", "refs": []});
        base.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
        serde_json::from_value(base).unwrap()
    }

    fn png(img: &RgbaImage) -> Vec<u8> {
        png_of(img)
    }

    /// 白底上 n × n 个色块，格间留白
    fn grid(n: u32) -> Vec<u8> {
        let cell = 1024 / n;
        let pad = cell / 8;
        png(&RgbaImage::from_fn(1024, 1024, |x, y| {
            let (cx, cy) = (x % cell, y % cell);
            if (pad..cell - pad).contains(&cx) && (pad..cell - pad).contains(&cy) {
                image::Rgba([200, (x / cell * 40) as u8, (y / cell * 40) as u8, 255])
            } else {
                image::Rgba([255, 255, 255, 255])
            }
        }))
    }

    fn reply_with(
        p: Vec<u8>,
    ) -> impl Fn(&crate::imagegen::tests::Req) -> crate::imagegen::tests::Reply + Send + 'static
    {
        move |_| json_reply(json!({"data": [{"b64_json": b64(&p)}]}))
    }

    fn memes(state: &AppState) -> u64 {
        state.store().meme_bytes().unwrap().0
    }

    #[test]
    fn set_splits_into_16() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let state = AppState::for_test();
        let added = AtomicUsize::new(0);
        let on_added = |_| {
            added.fetch_add(1, Ordering::SeqCst);
        };
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &on_added,
        };
        let (url, _h) = fake(1, reply_with(grid(4)));
        let c = cfg(&url);
        let out = run(generate(&env, &c, req(json!({"kind": "set"})))).unwrap();
        let GenOut::Done { ids } = out else {
            panic!("应切成 16 张")
        };
        assert_eq!(ids.len(), 16);
        assert_eq!(memes(&state), 16);
        assert_eq!(added.load(Ordering::SeqCst), 16);
        let names: Vec<String> = ids
            .iter()
            .map(|&id| {
                state
                    .store()
                    .live_summary(id)
                    .unwrap()
                    .unwrap()
                    .meme
                    .unwrap()
                    .name
            })
            .collect();
        assert_eq!(names, xpaste_pro::meme_emotions());
        assert!(!grid_shaky(&c));
        // 一套的 16 张带同一个套编号
        let sets: std::collections::HashSet<Option<String>> = ids
            .iter()
            .map(|&id| {
                state
                    .store()
                    .live_summary(id)
                    .unwrap()
                    .unwrap()
                    .meme
                    .unwrap()
                    .set
            })
            .collect();
        assert_eq!(sets.len(), 1);
        assert!(sets.iter().all(Option::is_some));
    }

    /// 分 4 次重画的一部分用队列给的套编号；单张没有套编号
    #[test]
    fn set_part_uses_given_set() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        let set_of = |id: i64| {
            state
                .store()
                .live_summary(id)
                .unwrap()
                .unwrap()
                .meme
                .unwrap()
                .set
        };
        let (url, _h) = fake(1, reply_with(grid(2)));
        let r = GenReq {
            set: Some("s1".into()),
            ..req(json!({"kind": "set_part", "part": 1}))
        };
        let GenOut::Done { ids } = run(generate(&env, &cfg(&url), r)).unwrap() else {
            panic!("应切成 4 张")
        };
        assert_eq!(ids.len(), 4);
        assert!(ids.iter().all(|&id| set_of(id).as_deref() == Some("s1")));
        let (url, _h) = fake(1, reply_with(grid(1)));
        let GenOut::Done { ids } =
            run(generate(&env, &cfg(&url), req(json!({"kind": "one"})))).unwrap()
        else {
            panic!("应是一张")
        };
        assert_eq!(set_of(ids[0]), None);
    }

    /// 抠图在哪个线程上跑（抠图、切格、编码不占调用方的异步线程）
    static CUT_ON: Mutex<Option<std::thread::ThreadId>> = Mutex::new(None);

    #[test]
    fn irregular_grid_kept() {
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| {
                *CUT_ON.lock().unwrap() = Some(std::thread::current().id());
                None
            },
            added: &|_| {},
        };
        // 中间一整块：找不到格间留白
        let blob = png(&RgbaImage::from_fn(512, 512, |x, y| {
            let c = if (64..448).contains(&x) && (64..448).contains(&y) {
                30
            } else {
                255
            };
            image::Rgba([c, c, c, 255])
        }));
        let (url, _h) = fake(1, reply_with(blob));
        let c = cfg(&url);
        let out = run(generate(&env, &c, req(json!({"kind": "set"})))).unwrap();
        assert!(matches!(out, GenOut::Irregular { .. }));
        assert_eq!(memes(&state), 1);
        assert!(grid_shaky(&c));
        let on = CUT_ON.lock().unwrap().expect("抠过图");
        assert_ne!(on, std::thread::current().id());
    }

    #[test]
    fn unconfigured_no_request() {
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        let r = run(generate(&env, &cfg(""), req(json!({}))));
        assert_eq!(r, Err("生图接口还没配好".to_string()));
        assert_eq!(memes(&state), 0);
    }

    #[test]
    fn gone_ref_dropped() {
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
        let r = run(generate(
            &env,
            &cfg(&url),
            req(json!({"refs": [{"id": 999, "file": null}, {"id": -1, "file": null}]})),
        ));
        assert_eq!(r, Err(GONE.to_string()));
        assert!(l.accept().is_err(), "不应发出请求");
    }

    #[test]
    fn captions_unconfigured() {
        let state = AppState::for_test();
        let c = crate::translate::cfg(&Settings::default());
        let r = run(captions(&state, &c, "无语", &[], None, &[]));
        assert_eq!(r, Err(CAPTION_UNCONFIGURED.to_string()));
    }

    #[test]
    fn ai_item_has_valid_scene() {
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        let sketch = save_sketch(&state.data_dir, &grid(1)).unwrap();
        let (url, _h) = fake(1, reply_with(grid(1)));
        let GenOut::Done { ids } = run(generate(
            &env,
            &cfg(&url),
            req(json!({"style": "", "custom": "毛线", "idea": "无语", "caption": "行吧", "sketch": sketch})),
        ))
        .unwrap() else {
            panic!()
        };
        let scene = state.store().meme_scene(ids[0]).unwrap().unwrap();
        if xpaste_pro::AVAILABLE {
            assert!(xpaste_pro::meme_scene_ok(&scene), "{scene}");
        }
        let v: serde_json::Value = serde_json::from_str(&scene).unwrap();
        assert_eq!(v["look"], "ai");
        assert_eq!(v["results"], json!([]));
        assert_eq!(v["style"], "自定义：毛线");
        assert_eq!(v["cap"], "");
        assert_eq!(v["refs"][0]["sketch"], sketch);
        assert_eq!(
            names(&state, &[ids[0], 999]),
            vec!["表情包".to_string(), String::new()]
        );
    }

    #[test]
    fn unreadable_ref_named() {
        let r = RefKey {
            id: 7,
            file: Some(2),
        };
        assert_eq!(encode_one(r, b"junk"), Err(format!("{UNREADABLE}7:2")));
        let r = RefKey { id: 8, file: None };
        assert_eq!(encode_one(r, b"junk"), Err(format!("{UNREADABLE}8:")));
    }

    #[test]
    fn at_most_4_refs_sent() {
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        // 库里 6 张图当参考图，外加一张草图
        let dir = state.data_dir.join("memes");
        std::fs::create_dir_all(&dir).unwrap();
        let refs: Vec<serde_json::Value> = (0..6)
            .map(|i| {
                let f = format!("{}.png", uuid());
                std::fs::write(dir.join(&f), grid(1)).unwrap();
                let id = state
                    .store()
                    .add_meme(
                        NewMeme {
                            file: f,
                            info: MemeInfo {
                                source: "ai".into(),
                                name: format!("{i}"),
                                style: None,
                                gif: false,
                                set: None,
                            },
                            search: String::new(),
                            scene: String::new(),
                            width: 1024,
                            height: 1024,
                            bytes: 1,
                        },
                        0,
                    )
                    .unwrap();
                json!({"id": id, "file": 0})
            })
            .collect();
        let sketch = save_sketch(&state.data_dir, &grid(1)).unwrap();
        let (url, h) = fake(1, reply_with(grid(1)));
        run(generate(
            &env,
            &cfg(&url),
            req(json!({"refs": refs, "sketch": sketch})),
        ))
        .unwrap();
        let seen = h.join().unwrap();
        let body = String::from_utf8_lossy(&seen[0].body);
        assert_eq!(body.matches("name=\"image[]\"").count(), 4);
    }

    #[test]
    fn act_makes_one_gif() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let state = AppState::for_test();
        let env = Env {
            state: &state,
            cutout: |_| None,
            added: &|_| {},
        };
        // 先有一张 AI 出的当底；AI 动作带着它当参考图，走 /images/edits，回一张 2 × 2
        let (url, _h) = fake(2, {
            let (one, four) = (grid(1), grid(2));
            move |r| {
                let p = if r.path == "/images/edits" {
                    &four
                } else {
                    &one
                };
                json_reply(json!({"data": [{"b64_json": b64(p)}]}))
            }
        });
        let c = cfg(&url);
        let GenOut::Done { ids } = run(generate(&env, &c, req(json!({})))).unwrap() else {
            panic!()
        };
        let base = ids[0];
        let out = run(generate(
            &env,
            &c,
            req(json!({"kind": "act", "act": "wave", "base": base})),
        ))
        .unwrap();
        let GenOut::Done { ids } = out else {
            panic!("应画成 4 帧")
        };
        let s = state.store().live_summary(ids[0]).unwrap().unwrap();
        assert_eq!(s.meme.unwrap().name, "挥手");
        assert!(frame_png(&state, ids[0], 3).is_some());
        assert!(frame_png(&state, ids[0], 4).is_none());
    }

    #[test]
    fn sticker_has_white_border() {
        // 假抠图：100 × 100 中间 20 × 20 的红色主体
        let cut: Cutout = |_| {
            let img = RgbaImage::from_fn(100, 100, |x, y| {
                if (40..60).contains(&x) && (40..60).contains(&y) {
                    image::Rgba([255, 0, 0, 255])
                } else {
                    image::Rgba([0, 0, 0, 0])
                }
            });
            Some((100, 100, img.into_raw()))
        };
        let src = png(&RgbaImage::new(100, 100));
        let out = image::load_from_memory(&sticker(cut, &src).unwrap())
            .unwrap()
            .to_rgba8();
        assert_eq!(out.get_pixel(50, 50).0, [255, 0, 0, 255]);
        // 白边 2.5 px
        assert_eq!(out.get_pixel(61, 50).0, [255, 255, 255, 255]);
        assert_eq!(out.get_pixel(70, 50).0[3], 0);
        assert!(sticker(|_| None, &src).is_none());
    }
}
