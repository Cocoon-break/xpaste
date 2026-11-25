//! 发布前量改字用的：照应用自己的做法跑一遍，把结果交给脚本去数。
//!
//! 只在测试里编译，不进安装包；不设 `XPASTE_TEXTEDIT_GATES` 什么也不做。从标准输入一行收一个 JSON，
//! 往标准输出写一行 `TE {…}`。生图接口的 Key 只从标准输入来，不打印、不落盘。

use super::*;
use std::io::BufRead;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::imagegen::{self, EditReq, Format, ImageCfg, ImgFail};
use crate::textedit_tasks::NEIGHBORS;

/// 自检各项的数值只写在日志里，截下来
static SAID: Mutex<Vec<String>> = Mutex::new(Vec::new());
struct Ear;
impl log::Log for Ear {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, r: &log::Record<'_>) {
        let s = r.args().to_string();
        if s.starts_with("改字自检") {
            lock(&SAID).push(s);
        }
    }
    fn flush(&self) {}
}

#[test]
#[ignore = "不是检查：给量改字的脚本用，从标准输入收请求"]
fn gates() {
    if std::env::var_os("XPASTE_TEXTEDIT_GATES").is_none() {
        return;
    }
    let _ = log::set_logger(&Ear);
    log::set_max_level(log::LevelFilter::Info);
    let mut held = None;
    for row in std::io::stdin().lock().lines() {
        let v: Value = serde_json::from_str(&row.unwrap()).unwrap();
        lock(&SAID).clear();
        let out = answer(&mut held, &v).unwrap_or_else(|e| json!({ "err": e.to_string() }));
        println!("\nTE {out}");
    }
}

fn bad(e: impl std::fmt::Display) -> AppError {
    no(&e.to_string())
}

fn get<T: DeserializeOwned>(v: &Value, k: &str) -> Result<T, AppError> {
    T::deserialize(&v[k]).map_err(|e| bad(format!("{k}：{e}")))
}

fn save(v: &Value, k: &str, img: &TeImg, icc: Option<&[u8]>) -> Result<(), AppError> {
    let Some(to) = v[k].as_str() else {
        return Ok(());
    };
    let png = crate::img_meta::encode_png(img.w, img.h, &img.px, icc).map_err(bad)?;
    std::fs::write(to, png).map_err(bad)
}

/// 原图盖上改过的一块
fn laid(live: &Live, p: &TePaste) -> TeImg {
    let mut img = cut(&live.img, [0, 0, live.img.w, live.img.h]);
    lay(&mut img, (0, 0), p);
    img
}

fn answer(held: &mut Option<(String, Live)>, v: &Value) -> Result<Value, AppError> {
    let file: String = get(v, "file")?;
    if held.as_ref().is_none_or(|h| h.0 != file) {
        let bytes = std::fs::read(&file).map_err(bad)?;
        let (img, icc) = crate::textedit::decode(&bytes).map_err(|e| no(e.text()))?;
        let live = Live::fresh(img, icc);
        live.start(ocr);
        *held = Some((file, live));
    }
    let live = &held.as_ref().unwrap().1;
    let icc = live.icc.as_deref();
    match v["do"].as_str().unwrap_or_default() {
        "read" => {
            let r = live.read();
            let lines = r
                .lines
                .iter()
                .map(|l| json!({ "text": l.text, "quad": l.quad, "chars": l.chars.len() }));
            Ok(json!({
                "w": live.img.w, "h": live.img.h,
                "engine": r.engine, "failed": r.failed, "locked": r.locked.len(),
                "blocked": r.blocked.map(|b| b.label()),
                // 读不出字时还认不认得出
                "blind": xpaste_pro::te_block(&[], &live.img).map(|b| b.label()),
                "lines": lines.collect::<Vec<_>>(),
            }))
        }
        "ranges" => {
            let (line, n, from, to) = (
                get::<LineRef>(v, "line")?,
                get(v, "n")?,
                get(v, "from")?,
                get(v, "to")?,
            );
            let r = ranges(live, &line, n, Vec::new(), from, to)?;
            let s = xpaste_pro::te_strip(&live.img, line.get(live)?.0);
            Ok(json!({ "range": r.range, "quad": span(&s, r.range), "m": r.m, "h": r.h }))
        }
        "local" => {
            let s = live.check(&get::<Draft>(v, "draft")?)?;
            let l = live.local(&s);
            let said = lock(&SAID).clone();
            let Some(l) = l else {
                return Ok(json!({ "verdict": null, "said": said }));
            };
            save(v, "out", &laid(live, &l.paste), icc)?;
            let verdict = format!("{:?}", l.verdict);
            Ok(json!({ "verdict": verdict, "why": l.why, "said": said }))
        }
        "strip" => {
            let (h, px) = strip(live, &get(v, "line")?, Extend::default())?;
            let img = TeImg { w: h.w, h: h.h, px };
            save(v, "out", &img, None)?;
            Ok(json!({ "quad": h.quad }))
        }
        // 改过的条带盖回去
        "back" => {
            let px = image::open(get::<String>(v, "strip")?)
                .map_err(bad)?
                .into_rgba8();
            let s = xpaste_pro::te_strip_native(&live.img, get(v, "quad")?);
            let p =
                xpaste_pro::te_strip_paste(&live.img, &s, &px).ok_or_else(|| no(NOT_CHANGED))?;
            save(v, "out", &laid(live, &p), icc)?;
            Ok(json!({}))
        }
        "probe" => probe(live, v),
        _ => Err(no("不认识的请求")),
    }
}

/// 照应用的做法发一次请求、贴回去
fn probe(live: &Live, v: &Value) -> Result<Value, AppError> {
    let s = live.check(&get::<Draft>(v, "draft")?)?;
    let cfg = ImageCfg {
        format: get::<Format>(v, "format")?,
        base_url: get(v, "base")?,
        api_key: get(v, "key")?,
        model: get(v, "model")?,
    };
    let (line, range) = wide(&live.img, &s.line, s.range);
    let crop = xpaste_pro::te_crop(&live.img, line, range, NEIGHBORS);
    let prompt = xpaste_pro::te_prompt(&s.old, &s.new, s.realign());
    let req = EditReq {
        prompt: &prompt,
        crop: &crop.png,
        mask: get::<bool>(v, "mask")?.then_some(&crop.mask[..]),
    };
    let t = Instant::now();
    let got = tauri::async_runtime::block_on(imagegen::edit(&cfg, req));
    let secs = t.elapsed().as_secs_f32();
    let png = match got {
        Ok(png) => png,
        Err(e) => {
            let mask = matches!(e, ImgFail::MaskRejected);
            return Ok(
                json!({ "fail": imagegen::edit_reason(&e), "maskRejected": mask, "secs": secs }),
            );
        }
    };
    let raw = image::load_from_memory(&png).map_err(bad)?.into_rgba8();
    let raw = TeImg {
        w: raw.width(),
        h: raw.height(),
        px: raw.into_raw(),
    };
    save(v, "raw", &raw, None)?;
    let mut out = json!({ "secs": secs, "w": raw.w, "h": raw.h });
    match xpaste_pro::te_paste(&live.img, line, range, &crop, &png, &s.old, &s.new) {
        Ok(p) => {
            save(v, "out", &laid(live, &p), live.icc.as_deref())?;
            out["exact"] = p.exact.into();
            out["soft"] = p.soft.into();
            out["misread"] = misread(&live.img, &line, &p, &s.new, ocr).into();
        }
        Err(e) => out["paste"] = format!("{e:?}").into(),
    }
    Ok(out)
}
