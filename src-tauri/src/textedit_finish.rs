//! 改字的完成动作：贴到应用、复制、存一份、另存为、存到截图文件夹、钉回原处、换掉贴图。
//! 只用已改好的，在改的不算、接着改；不信页面，动作和来源对不上的不做。

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use xpaste_core::Flavor;
use xpaste_platform::PlatformError;

use crate::error::AppError;
use crate::panel::Rect;
use crate::state::AppState;
use crate::textedit::{self, Source};
use crate::textedit_job::{Output, no, not_saved};
use crate::textedit_store::{self as store, SaveFail};
use crate::textedit_window;

const CLOSED: &str = "这张图已经关了";
const UNFIT: &str = "这张图做不了这个";
const DIR: &str = "这个文件夹存不进去";
const SAME: &str = "不能盖掉原文件，换个名字";
const COPIED: &str = "已复制，已加入历史";
const PIN_GONE: &str = "贴图已经关了，已复制，已加入历史";
const REPINNED: &str = "已钉回原处";

/// 页面只说动作名：另存为的路径由后端问
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Finish {
    Paste,
    Copy,
    SaveBeside,
    SaveAs(#[serde(skip)] PathBuf),
    SaveShotDir,
    Repin,
    ReplacePin,
}

impl Finish {
    fn name(&self) -> &'static str {
        match self {
            Finish::Paste => "paste",
            Finish::Copy => "copy",
            Finish::SaveBeside => "saveBeside",
            Finish::SaveAs(_) => "saveAs",
            Finish::SaveShotDir => "saveShotDir",
            Finish::Repin => "repin",
            Finish::ReplacePin => "replacePin",
        }
    }

    /// 从这个来源来的做不做得了（同页面底栏的按钮）
    fn fits(&self, src: &Source) -> bool {
        match self {
            Finish::Copy => true,
            Finish::Paste => matches!(src, Source::Clip { .. } | Source::Panel { .. }),
            Finish::SaveBeside | Finish::SaveAs(_) => matches!(src, Source::File(_)),
            Finish::SaveShotDir => !matches!(src, Source::File(_)),
            Finish::Repin => matches!(src, Source::Shot { .. }),
            Finish::ReplacePin => matches!(src, Source::Pin { .. }),
        }
    }
}

/// 页面接着做什么
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Then {
    Stay,
    Close,
    /// 另存为
    SaveAs,
}

/// 做完了。`text` 是提示，里面可能有文件名，不派生 Debug
#[derive(Serialize)]
pub struct FinishOut {
    pub text: Option<String>,
    pub then: Then,
}

impl FinishOut {
    /// 什么都不用做（贴过去的窗口已经关了，另存为取消了）
    pub const NONE: FinishOut = FinishOut {
        text: None,
        then: Then::Stay,
    };

    fn new(text: impl Into<String>, then: Then) -> Self {
        FinishOut {
            text: Some(text.into()),
            then,
        }
    }
}

impl From<SaveFail> for AppError {
    fn from(e: SaveFail) -> Self {
        match e {
            SaveFail::Dir => no(DIR),
            SaveFail::Same => no(SAME),
            SaveFail::Io(e) => not_saved(e),
        }
    }
}

/// 成品落到的地方
trait Sink {
    fn write(&self, flavors: &[Flavor]) -> Result<u64, PlatformError>;
    /// 关窗口、贴过去（没贴成的自己提示）。窗口里已经不是这张图时不贴，返回 false
    fn paste(&self, target: &str) -> bool;
    fn pin_at(&self, rect: Rect, png: &[u8]) -> Result<(), AppError>;
    /// 贴图已经关了返回 false
    fn pin_replace(&self, n: u64, png: &[u8]) -> Result<bool, AppError>;
    /// 存到截图文件夹：文件名，和有没有同时复制
    fn save_shot(&self, png: Vec<u8>) -> Result<(String, bool), AppError>;
}

fn deliver(
    state: &AppState,
    sink: &dyn Sink,
    src: &Source,
    act: &Finish,
    target: Option<&str>,
    out: &Output,
    icc: Option<&[u8]>,
) -> Result<FinishOut, AppError> {
    if !act.fits(src) {
        return Err(no(UNFIT));
    }
    let png = || store::png(&out.img, icc, out.ai).map_err(not_saved);
    // 按普通写入：成品照常进历史
    let copy = |png| {
        let flavors = xpaste_platform::png_flavors(png);
        crate::clipboard::write_own_via(state, &flavors, "改字", |f| sink.write(f))
    };
    let saved = |p: &Path, then| {
        let name = p.file_name().unwrap_or_default().to_string_lossy();
        FinishOut::new(format!("已存成 {name}，原文件没动"), then)
    };
    Ok(match (act, src) {
        (Finish::Paste, _) if let Some(target) = target => {
            copy(png()?)?;
            if sink.paste(target) {
                FinishOut::NONE
            } else {
                FinishOut::new(COPIED, Then::Stay)
            }
        }
        (Finish::SaveBeside, Source::File(p)) => {
            match store::write_beside(p, &out.img, icc, out.ai) {
                Ok(p) => saved(&p, Then::Close),
                Err(SaveFail::Dir) => FinishOut::new(DIR, Then::SaveAs),
                Err(e) => return Err(e.into()),
            }
        }
        (Finish::SaveAs(to), Source::File(p)) => {
            saved(&store::write_as(to, p, &out.img, icc, out.ai)?, Then::Stay)
        }
        (Finish::SaveShotDir, _) => {
            let (name, copied) = sink.save_shot(png()?).map_err(|e| match e {
                AppError::Io(e) => SaveFail::from(e).into(),
                e => e,
            })?;
            let copied = if copied { "并复制" } else { "" };
            FinishOut::new(format!("已保存{copied} · {name}"), Then::Stay)
        }
        (Finish::Repin, Source::Shot { rect, .. }) => {
            sink.pin_at(*rect, &png()?)?;
            FinishOut::new(REPINNED, Then::Close)
        }
        (Finish::ReplacePin, Source::Pin { n }) => {
            let png = png()?;
            if sink.pin_replace(*n, &png)? {
                // 贴图自己提示
                FinishOut {
                    text: None,
                    then: Then::Close,
                }
            } else {
                copy(png)?;
                FinishOut::new(PIN_GONE, Then::Close)
            }
        }
        _ => {
            copy(png()?)?;
            FinishOut::new(COPIED, Then::Close)
        }
    })
}

struct Real<'a> {
    app: &'a AppHandle,
    /// 窗口里这一次打开的号
    seq: u64,
}

impl Sink for Real<'_> {
    fn write(&self, flavors: &[Flavor]) -> Result<u64, PlatformError> {
        xpaste_platform::write(flavors)
    }

    fn paste(&self, target: &str) -> bool {
        let Some(o) = textedit::shut_if(self.seq) else {
            return false;
        };
        textedit_window::hide(self.app, o.back);
        // 窗口关了，用系统通知说
        if crate::meme::paste_into(self.app, o.back, "改字") {
            crate::notify(self.app, &format!("已贴到 {target}，已加入历史"));
        }
        true
    }

    fn pin_at(&self, rect: Rect, png: &[u8]) -> Result<(), AppError> {
        crate::shot::pin_at(self.app, rect, png).map(drop)
    }

    fn pin_replace(&self, n: u64, png: &[u8]) -> Result<bool, AppError> {
        crate::shot::pin_replace(self.app, n, png)
    }

    fn save_shot(&self, png: Vec<u8>) -> Result<(String, bool), AppError> {
        let o = crate::shot::save(self.app, png)?;
        Ok((o.file.unwrap_or_default(), o.copied))
    }
}

fn current(job: &str) -> Result<textedit::Current, AppError> {
    textedit::current(job).ok_or_else(|| no(CLOSED))
}

/// 窗口里的图 `job` 完成。耗时，会阻塞。试用结束也能做：改过的能看、能复制和存
pub fn finish(app: &AppHandle, job: &str, act: Finish) -> Result<FinishOut, AppError> {
    textedit::check_on(app)?;
    let cur = current(job)?;
    let t = Instant::now();
    let out = cur.live.output(&textedit::dir(app))?;
    let r = deliver(
        &app.state::<AppState>(),
        &Real { app, seq: cur.seq },
        &cur.src,
        &act,
        cur.target.as_deref(),
        &out,
        cur.live.icc.as_deref(),
    );
    log::info!(
        "改字 完成 {} 来自 {} {}×{} 改好 {} 处{}{} 耗时 {} ms",
        act.name(),
        cur.src.kind(),
        out.img.w,
        out.img.h,
        out.done,
        if out.ai { "（含 AI）" } else { "" },
        if r.is_err() { " 没成" } else { "" },
        t.elapsed().as_millis()
    );
    r
}

/// 另存为的对话框里先写好的路径
pub fn suggest(job: &str) -> Result<PathBuf, AppError> {
    match current(job)?.src {
        Source::File(p) => Ok(store::out_path(&p)),
        _ => Err(no(UNFIT)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::is_self_write;
    use crate::textedit_job::tests::*;
    use crate::textedit_job::{self as job, Live};
    use crate::textedit_tasks::tests::tasks as new_tasks;
    use std::cell::RefCell;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use xpaste_platform::PlatformError;

    const RECT: Rect = Rect {
        x: 10.0,
        y: 20.0,
        w: 300.0,
        h: 120.0,
    };
    /// 记下落到哪儿了；`on` 是窗口还开着这张图，`pin` 是贴图还在
    struct Fake {
        on: bool,
        pin: bool,
        clip: RefCell<Vec<Vec<u8>>>,
        did: RefCell<Vec<String>>,
    }
    impl Fake {
        fn new() -> Self {
            Fake {
                on: true,
                pin: true,
                clip: RefCell::default(),
                did: RefCell::default(),
            }
        }
        fn note(&self, s: String) {
            self.did.borrow_mut().push(s);
        }
        fn clip(&self) -> Vec<u8> {
            self.clip.borrow().last().unwrap().clone()
        }
    }
    impl Sink for Fake {
        fn write(&self, f: &[Flavor]) -> Result<u64, PlatformError> {
            self.clip.borrow_mut().push(f[0].data.clone());
            Ok(11)
        }
        fn paste(&self, target: &str) -> bool {
            self.note(format!("paste {target}"));
            self.on
        }
        fn pin_at(&self, r: Rect, png: &[u8]) -> Result<(), AppError> {
            self.clip.borrow_mut().push(png.to_vec());
            self.note(format!("pin {} {} {} {}", r.x, r.y, r.w, r.h));
            Ok(())
        }
        fn pin_replace(&self, n: u64, _: &[u8]) -> Result<bool, AppError> {
            self.note(format!("replace {n}"));
            Ok(self.pin)
        }
        fn save_shot(&self, png: Vec<u8>) -> Result<(String, bool), AppError> {
            self.clip.borrow_mut().push(png);
            Ok(("截图 1.png".into(), true))
        }
    }

    const NEW_BOX: (u32, u32) = (76 + 260, 90);
    const RED: [u8; 3] = [255, 0, 0];

    /// 一张改好一处的图
    fn edited(tmp: &Tmp) -> Arc<Live> {
        let l = live();
        put_local(new_tasks(), &tmp.0, &l, &draft(NEW, range(4, 5)), 4, RED).unwrap();
        l
    }
    fn src_shot() -> Source {
        Source::Shot {
            rect: RECT,
            png: vec![],
        }
    }
    fn go(
        sink: &Fake,
        src: &Source,
        act: &Finish,
        target: Option<&str>,
        o: &Output,
    ) -> Result<FinishOut, AppError> {
        deliver(&AppState::for_test(), sink, src, act, target, o, None)
    }
    fn text(r: &Result<FinishOut, AppError>) -> (Option<&str>, Then) {
        let o = r.as_ref().unwrap_or_else(|e| panic!("{e}"));
        (o.text.as_deref(), o.then)
    }
    fn px(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).unwrap().into_rgba8()
    }
    const COPIED: (Option<&str>, Then) = (Some("已复制，已加入历史"), Then::Close);

    macro_rules! need_pro {
        () => {
            if !xpaste_pro::AVAILABLE {
                return eprintln!("这个构建不含 Pro，跳过");
            }
        };
    }

    /// 在改的、没贴好的不算；一处都没改好的不做；不给改的不做
    #[test]
    fn finish_uses_done_only() {
        need_pro!();
        let (t, tmp) = (new_tasks(), Tmp::new());
        let l = live();
        let none = || msg(l.output(&tmp.0));
        assert_eq!(none(), "还没有改好的");
        // 在改
        let (task, ai) = t
            .submit(&tmp.0, &l, &draft("会政改到三点", range(1, 2)), true, true)
            .unwrap();
        assert_eq!(none(), "还没有改好的");
        put_local(t, &tmp.0, &l, &draft(NEW, range(4, 5)), 4, RED).unwrap();
        let o = l.output(&tmp.0).unwrap();
        assert_eq!((o.done, o.ai), (1, false));
        let at =
            |o: &Output, x: u32, y: u32| o.img.px[((y * o.img.w + x) * 4) as usize..][..3].to_vec();
        assert_eq!(at(&o, NEW_BOX.0, NEW_BOX.1), RED);
        // 没贴好的、没选中的也不画
        let mut job = store::load(&tmp.0, &l.id).unwrap();
        let p = xpaste_pro::TePaste {
            patch: xpaste_pro::TeImg {
                w: 2,
                h: 2,
                px: [0, 0, 255, 255].repeat(4),
            },
            x: 3,
            y: 3,
            exact: false,
            soft: false,
        };
        let i = store::add_cand(&tmp.0, &mut job, ai, &p, store::By::Ai, "", false).unwrap();
        job.edits.iter_mut().find(|e| e.id == ai).unwrap().pick = i;
        store::save(&tmp.0, &job).unwrap();
        let back = Live::load(&tmp.0, &l.id).unwrap();
        back.no_reading();
        let o = back.output(&tmp.0).unwrap();
        assert_eq!((o.done, o.ai), (1, false));
        assert_ne!(at(&o, 3, 3), [0, 0, 255]);
        // 在改的没受影响
        assert!(t.views().iter().any(|v| v.id == task));
        // 不给改的
        let banned = live_with(job::Read {
            blocked: Some(xpaste_pro::TeBlock::Receipt),
            ..read()
        });
        assert_eq!(msg(banned.output(&tmp.0)), "这张图不支持改字");
    }

    /// 复制按普通写入：不打自身写入标记，监听器照常收进历史
    #[test]
    fn copy_enters_history() {
        need_pro!();
        let tmp = Tmp::new();
        let (state, sink) = (AppState::for_test(), Fake::new());
        let o = edited(&tmp).output(&tmp.0).unwrap();
        let r = deliver(&state, &sink, &Source::Blank, &Finish::Copy, None, &o, None);
        assert_eq!(text(&r), COPIED);
        assert!(!is_self_write(
            11,
            state.last_write_seq.load(Ordering::SeqCst),
            state.writing.load(Ordering::SeqCst)
        ));
        assert_eq!(state.own_write_seq.load(Ordering::SeqCst), 11);
    }

    /// 剪切板、贴图、截图文件夹的 PNG 都带元数据和 ICC；有 AI 出的另外注明
    #[test]
    fn copy_png_marked() {
        need_pro!();
        let tmp = Tmp::new();
        let mut o = edited(&tmp).output(&tmp.0).unwrap();
        let mut icc = vec![7u8; 200];
        icc[16..20].copy_from_slice(b"RGB ");
        let has = |b: &[u8], s: &str| b.windows(s.len()).any(|w| w == s.as_bytes());
        for ai in [false, true] {
            o.ai = ai;
            for (src, act) in [
                (Source::Blank, Finish::Copy),
                (src_shot(), Finish::Repin),
                (Source::Blank, Finish::SaveShotDir),
            ] {
                let sink = Fake::new();
                let state = AppState::for_test();
                deliver(&state, &sink, &src, &act, None, &o, Some(&icc))
                    .ok()
                    .unwrap();
                let png = sink.clip();
                assert!(has(&png, "xPaste 改字"));
                assert_eq!(has(&png, "compositeWithTrainedAlgorithmicMedia"), ai);
                use image::ImageDecoder;
                let mut d =
                    image::codecs::png::PngDecoder::new(std::io::Cursor::new(&png)).unwrap();
                assert_eq!(d.icc_profile().unwrap(), Some(icc.clone()));
            }
        }
    }

    /// 复制出来的：改动范围外和原图逐字节相同，尺寸相同
    #[test]
    fn outside_bytes_equal() {
        need_pro!();
        let tmp = Tmp::new();
        let l = edited(&tmp);
        let (sink, o) = (Fake::new(), l.output(&tmp.0).unwrap());
        go(&sink, &Source::Blank, &Finish::Copy, None, &o)
            .ok()
            .unwrap();
        let out = px(&sink.clip());
        assert_eq!(out.dimensions(), (l.img.w, l.img.h));
        let c = &edit_of(&l, 1).cands[0];
        let patch = px(&l.cand_png(&tmp.0, &c.file).unwrap());
        let inside = |x: u32, y: u32| {
            (c.x..c.x + patch.width()).contains(&x) && (c.y..c.y + patch.height()).contains(&y)
        };
        for (i, (a, b)) in out.as_raw().iter().zip(&l.img.px).enumerate() {
            let (x, y) = ((i / 4) as u32 % l.img.w, (i / 4) as u32 / l.img.w);
            assert!(a == b || inside(x, y), "({x}, {y})");
        }
        assert_eq!(out.get_pixel(NEW_BOX.0, NEW_BOX.1).0[..3], RED);
    }

    #[test]
    fn paste_without_target_is_copy() {
        need_pro!();
        let tmp = Tmp::new();
        let o = edited(&tmp).output(&tmp.0).unwrap();
        let src = Source::Panel {
            record: 1,
            file: None,
        };
        let sink = Fake::new();
        assert_eq!(text(&go(&sink, &src, &Finish::Paste, None, &o)), COPIED);
        assert_eq!((sink.did.borrow().len(), sink.clip.borrow().len()), (0, 1));
        // 有目标：写好剪切板再贴，窗口由后端关
        let r = go(&sink, &src, &Finish::Paste, Some("微信"), &o);
        assert_eq!(text(&r), (None, Then::Stay));
        assert_eq!(*sink.did.borrow(), ["paste 微信"]);
        // 写的时候窗口关了、换了一张：只复制，不往别处贴
        let gone = Fake {
            on: false,
            ..Fake::new()
        };
        let r = go(&gone, &src, &Finish::Paste, Some("微信"), &o);
        assert_eq!(text(&r), (COPIED.0, Then::Stay));
    }

    /// 钉回截图时的位置
    #[test]
    fn repin_at_shot_rect() {
        need_pro!();
        let tmp = Tmp::new();
        let o = edited(&tmp).output(&tmp.0).unwrap();
        let sink = Fake::new();
        let r = go(&sink, &src_shot(), &Finish::Repin, None, &o);
        assert_eq!(text(&r), (Some("已钉回原处"), Then::Close));
        assert_eq!(*sink.did.borrow(), ["pin 10 20 300 120"]);
    }

    #[test]
    fn replace_pin_gone_falls_back_to_copy() {
        need_pro!();
        let tmp = Tmp::new();
        let o = edited(&tmp).output(&tmp.0).unwrap();
        let (src, act) = (Source::Pin { n: 7 }, Finish::ReplacePin);
        let sink = Fake::new();
        // 换好了由贴图自己提示
        let r = go(&sink, &src, &act, None, &o);
        assert_eq!(text(&r), (None, Then::Close));
        assert_eq!(*sink.did.borrow(), ["replace 7"]);
        assert!(sink.clip.borrow().is_empty());
        let gone = Fake {
            pin: false,
            ..Fake::new()
        };
        let r = go(&gone, &src, &act, None, &o);
        assert_eq!(
            text(&r),
            (Some("贴图已经关了，已复制，已加入历史"), Then::Close)
        );
        assert_eq!(gone.clip.borrow().len(), 1);
    }

    /// 存一份：写不进去的改走另存为，改好的不丢
    #[test]
    fn save_beside_dir_fail_message() {
        need_pro!();
        let tmp = Tmp::new();
        let l = edited(&tmp);
        let o = l.output(&tmp.0).unwrap();
        let (sink, from) = (Fake::new(), tmp.0.join("a.png"));
        std::fs::write(&from, b"src").unwrap();
        let src = Source::File(from.clone());
        let r = go(&sink, &src, &Finish::SaveBeside, None, &o);
        let saved = "已存成 a 改字.png，原文件没动";
        assert_eq!(text(&r), (Some(saved), Then::Close));
        assert_eq!(std::fs::read(&from).unwrap(), b"src");
        let gone = Source::File(tmp.0.join("没有/a.png"));
        let r = go(&sink, &gone, &Finish::SaveBeside, None, &o);
        assert_eq!(text(&r), (Some("这个文件夹存不进去"), Then::SaveAs));
        let to = Finish::SaveAs(tmp.0.join("b.png"));
        let r = go(&sink, &gone, &to, None, &o);
        assert_eq!(text(&r), (Some("已存成 b.png，原文件没动"), Then::Stay));
        let back = px(&std::fs::read(tmp.0.join("b.png")).unwrap());
        assert!(back.into_raw() == o.img.px);
        let again = Finish::SaveAs(tmp.0.join("没有/c.png"));
        assert_eq!(msg(go(&sink, &src, &again, None, &o)), "这个文件夹存不进去");
        let itself = Finish::SaveAs(from.clone());
        assert_eq!(
            msg(go(&sink, &src, &itself, None, &o)),
            "不能盖掉原文件，换个名字"
        );
        assert!(l.output(&tmp.0).is_ok());
        assert!(sink.clip.borrow().is_empty());
    }

    #[test]
    fn save_to_shot_dir() {
        need_pro!();
        let tmp = Tmp::new();
        let o = edited(&tmp).output(&tmp.0).unwrap();
        let r = go(&Fake::new(), &Source::Blank, &Finish::SaveShotDir, None, &o);
        assert_eq!(text(&r), (Some("已保存并复制 · 截图 1.png"), Then::Stay));
    }

    /// 来源和动作对不上的不做
    #[test]
    fn act_must_fit_source() {
        let panel = Source::Panel {
            record: 1,
            file: None,
        };
        let table: [(Source, &[&str]); 7] = [
            (src_shot(), &["copy", "saveShotDir", "repin"]),
            (
                Source::Clip { record: 1 },
                &["paste", "copy", "saveShotDir"],
            ),
            (panel, &["paste", "copy", "saveShotDir"]),
            (
                Source::File("a.png".into()),
                &["copy", "saveBeside", "saveAs"],
            ),
            (Source::Pin { n: 1 }, &["copy", "saveShotDir", "replacePin"]),
            (Source::Blank, &["copy", "saveShotDir"]),
            (Source::Recent("x".into()), &["copy", "saveShotDir"]),
        ];
        let all = [
            "paste",
            "copy",
            "saveBeside",
            "saveAs",
            "saveShotDir",
            "repin",
            "replacePin",
        ];
        let o = Output {
            img: scene(),
            ai: false,
            done: 1,
        };
        for (src, can) in &table {
            for name in all {
                // 页面只说动作名，给不了路径
                let act: Finish = serde_json::from_value(name.into()).unwrap();
                assert_eq!(act.name(), name);
                assert_eq!(act.fits(src), can.contains(&name), "{} {name}", src.kind());
                if !can.contains(&name) {
                    let sink = Fake::new();
                    assert_eq!(
                        msg(go(&sink, src, &act, Some("微信"), &o)),
                        "这张图做不了这个"
                    );
                    assert!(sink.did.borrow().is_empty() && sink.clip.borrow().is_empty());
                }
            }
        }
        assert!(serde_json::from_value::<Finish>(serde_json::json!({"saveAs": "/x"})).is_err());
    }

    #[test]
    fn out_serializes() {
        let v = serde_json::to_value(FinishOut {
            text: None,
            then: Then::SaveAs,
        });
        assert_eq!(
            v.unwrap(),
            serde_json::json!({"text": null, "then": "saveAs"})
        );
    }
}
