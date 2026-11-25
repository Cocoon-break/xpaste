//! 改字的 AI 任务：每次写字是一个任务，不跟窗口走；名额和排队的先后与表情包制作共用（见 `gen_slots`）。
//! 只在内存里，不存盘。结果落到发起的那张图，不管窗口里现在是哪张。
//!
//! 花钱的规矩：只发裁块、遮罩和提示词；接口当场拒绝遮罩的，去掉遮罩重发一次，这个接口以后都不带；
//! 别的失败（超时、429）都不自动重发；取消了的不再发；花钱出的图都留着。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Instant;

use serde::Serialize;
use tauri::{AppHandle, Manager};
use xpaste_pro::{TeImg, TePaste};

use crate::error::AppError;
use crate::gen_slots::{self, Slots, Who};
use crate::imagegen::{self, EditReq, ImageCfg, ImgFail};
use crate::state::AppState;
use crate::textedit;
use crate::textedit_job::{self as job, Draft, GONE, LOCKED, Late, Live, MISREAD, Ocr, Spot, lock};
use crate::textedit_store as store;
use crate::translate::Fail;

/// 裁块两侧各带几个邻字
const NEIGHBORS: usize = 4;
const UNALIGNED: &str = "AI 出的图对不上";
pub const TASK_GONE: &str = "这个任务已经不在了";

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Waiting,
    Running,
    Done,
    Failed,
}

/// 发给页面的一个任务。带原文和新文，不派生 Debug
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: u64,
    pub job: String,
    pub edit: u64,
    pub status: Status,
    /// 排在它前面、还在排队的个数（表情包的也算）
    pub ahead: usize,
    /// 开始的 Unix 毫秒
    pub started_at: Option<u64>,
    pub reason: String,
    pub old: String,
    pub new: String,
}

/// 要跑的一个任务
pub struct Spec {
    pub live: Arc<Live>,
    pub edit: u64,
    pub spot: Spot,
    pub mask: bool,
}

struct Task {
    view: TaskView,
    spot: Spot,
    mask: bool,
    /// 排队、在改时拿着，结束了放开（一张图可能有几百 MB）
    live: Option<Arc<Live>>,
}

#[derive(Default)]
pub struct Queue {
    last_id: u64,
    tasks: Vec<Task>,
}

/// 任务列表和名额：平时是全局的那一份，测试各用各的
#[derive(Clone, Copy)]
pub struct Tasks {
    q: &'static Mutex<Queue>,
    slots: &'static Mutex<Slots>,
}

impl Tasks {
    fn q(&self) -> MutexGuard<'static, Queue> {
        lock(self.q)
    }

    fn push(&self, live: &Arc<Live>, edit: u64, spot: Spot, mask: bool) -> u64 {
        let mut q = self.q();
        q.last_id += 1;
        let id = q.last_id;
        q.tasks.push(Task {
            view: TaskView {
                id,
                job: live.id.clone(),
                edit,
                status: Status::Waiting,
                ahead: 0,
                started_at: None,
                reason: String::new(),
                old: spot.old.clone(),
                new: spot.new.clone(),
            },
            spot,
            mask,
            live: Some(live.clone()),
        });
        lock(self.slots).enqueue(Who::Edit, id);
        id
    }

    /// 轮到自己且有名额时，把排头标成在改
    fn next(&self) -> Option<(u64, Spec)> {
        let mut q = self.q();
        let mut slots = lock(self.slots);
        let id = slots.take(Who::Edit)?;
        let t = q.tasks.iter_mut().find(|t| t.view.id == id);
        let Some((t, live)) = t.and_then(|t| t.live.clone().map(|l| (t, l))) else {
            slots.release();
            return None;
        };
        t.view.status = Status::Running;
        t.view.started_at = Some(u64::try_from(crate::clock::now_ms()).unwrap_or(0));
        let spec = Spec {
            live,
            edit: t.view.edit,
            spot: t.spot.clone(),
            mask: t.mask,
        };
        Some((id, spec))
    }

    /// 在改时被取消了：已经不在列表里
    fn canceled(&self, id: u64) -> bool {
        self.q().tasks.iter().all(|t| t.view.id != id)
    }

    /// 在改的结束了，让出名额。返回改好了没有；取消了的为 None
    pub fn finish(&self, id: u64, r: Result<(), String>) -> Option<bool> {
        let mut q = self.q();
        lock(self.slots).release();
        let t = q.tasks.iter_mut().find(|t| t.view.id == id)?;
        t.live = None;
        match r {
            Ok(()) => t.view.status = Status::Done,
            Err(why) => {
                t.view.status = Status::Failed;
                t.view.reason = why;
            }
        }
        Some(t.view.status == Status::Done)
    }

    /// 没存进去的结果存进去了：算改好；`why` 是没贴好的，照旧没改出来
    pub fn saved(&self, id: u64, why: Option<&str>) {
        let mut q = self.q();
        let t = q.tasks.iter_mut().find(|t| t.view.id == id);
        if let Some(t) = t.filter(|t| t.view.status == Status::Failed) {
            if why.is_none() {
                t.view.status = Status::Done;
            }
            t.view.reason = why.unwrap_or("").into();
        }
    }

    /// 按条件拿掉。在改的请求不中止：结果回来照样存成候选、不选中，名额到那时才让
    fn cancel_if(&self, hit: impl Fn(&TaskView) -> bool) {
        let mut q = self.q();
        let mut slots = lock(self.slots);
        q.tasks.retain(|t| {
            let hit = hit(&t.view);
            if hit && t.view.status == Status::Waiting {
                slots.remove(Who::Edit, t.view.id);
            }
            !hit
        });
    }

    /// 取消一个排队的、在改的
    pub fn cancel(&self, id: u64) {
        let live = |s| matches!(s, Status::Waiting | Status::Running);
        self.cancel_if(|v| v.id == id && live(v.status));
    }

    /// 这几处被替换了、改回去了：在改的取消，没改出来的也从列表拿掉
    pub fn cancel_edits(&self, job: &str, edits: &[u64]) {
        self.cancel_if(|v| v.job == job && edits.contains(&v.edit));
    }

    /// 图删了、清空了：它排队的任务拿掉，撤销也不恢复
    pub fn cancel_gone(&self, dir: &Path) {
        self.cancel_if(|v| v.status == Status::Waiting && !store::exists(dir, &v.job));
    }

    /// 还没结束的一样的任务：同一张图、同四角、同范围、同文字
    fn twin(&self, job: &str, s: &Spot) -> Option<(u64, u64)> {
        let near = |a: [f32; 2]| (0..2).all(|k| (a[k] - s.range[k]).abs() < 0.5);
        let q = self.q();
        q.tasks
            .iter()
            .filter(|t| matches!(t.view.status, Status::Waiting | Status::Running))
            .find(|t| {
                let o = &t.spot;
                t.view.job == job
                    && (o.line, &o.old, &o.new) == (s.line, &s.old, &s.new)
                    && near(o.range)
            })
            .map(|t| (t.view.id, t.view.edit))
    }

    /// 没改出来的重新入队，回到队尾；不在了、不是没改出来的返回 false
    fn again(&self, id: u64, live: &Arc<Live>, mask: bool) -> bool {
        let mut q = self.q();
        let at = |t: &Task| t.view.id == id && t.view.status == Status::Failed;
        let Some(i) = q.tasks.iter().position(at) else {
            return false;
        };
        let mut t = q.tasks.remove(i);
        t.view.status = Status::Waiting;
        t.view.started_at = None;
        t.view.reason.clear();
        t.live = Some(live.clone());
        t.mask = mask;
        q.tasks.push(t);
        lock(self.slots).enqueue(Who::Edit, id);
        true
    }

    /// 去掉改好的、没改出来的（排队、在改的用 `cancel`）
    pub fn drop(&self, id: u64) {
        let over = |s| matches!(s, Status::Done | Status::Failed);
        self.q()
            .tasks
            .retain(|t| t.view.id != id || !over(t.view.status));
    }

    /// 换图、关窗口时：改好的都已经告诉过页面了
    pub fn sweep(&self) {
        self.q().tasks.retain(|t| t.view.status != Status::Done);
    }

    pub fn views(&self) -> Vec<TaskView> {
        let q = self.q();
        let slots = lock(self.slots);
        q.tasks
            .iter()
            .map(|t| TaskView {
                ahead: slots.ahead(Who::Edit, t.view.id),
                ..t.view.clone()
            })
            .collect()
    }

    /// 没改出来的那个任务是哪张图、哪一处的什么
    fn failed(&self, id: u64) -> Option<(String, u64, Spot)> {
        let q = self.q();
        let t = q.tasks.iter().find(|t| t.view.id == id)?;
        (t.view.status == Status::Failed).then(|| (t.view.job.clone(), t.view.edit, t.spot.clone()))
    }

    /// 提交一处给 AI 写：替换同一个地方的、存盘、入队。返回任务号和那一处的 id
    pub fn submit(
        self,
        dir: &Path,
        live: &Arc<Live>,
        d: &Draft,
        mask: bool,
        api: bool,
    ) -> Result<(u64, u64), AppError> {
        if !api {
            return Err(job::no(&imagegen::reason(&ImgFail::Base(Fail::Config))));
        }
        let s = live.check(d)?;
        live.commit(self, dir, |job| {
            // 连着提交两次一样的：只发一次
            if let Some(same) = self.twin(&live.id, &s) {
                return Ok(same);
            }
            let (edit, also) = job::put(job, &s)?;
            let gone = job::replace(&live.img, job, edit, also);
            store::save(dir, job).map_err(job::not_saved)?;
            self.cancel_edits(&live.id, &gone);
            Ok((self.push(live, edit, s, mask), edit))
        })
    }

    /// 重试一个没改出来的。`find` 按 id 找图；照样重新查一遍。出了图只是没存进去的，再存一次，不发请求
    pub fn retry(
        self,
        dir: &Path,
        id: u64,
        mask: bool,
        api: bool,
        find: impl FnOnce(&str) -> Result<Arc<Live>, AppError>,
    ) -> Result<(), AppError> {
        let gone = || job::no(TASK_GONE);
        let (job, edit, s) = self.failed(id).ok_or_else(gone)?;
        let live = find(&job)?;
        if let Some(r) = live.save_late(self, dir, id) {
            return r;
        }
        if live.gone(dir) {
            return Err(job::no(GONE));
        }
        if !api {
            return Err(job::no(&imagegen::reason(&ImgFail::Base(Fail::Config))));
        }
        live.check(&Draft {
            edit: Some(edit),
            line: s.line,
            range: Some(s.range),
            old: s.old,
            new: s.new,
            align: s.align,
        })?;
        if !self.again(id, &live, mask) {
            return Err(gone());
        }
        Ok(())
    }
}

// ---- 跑一个任务 ----

/// 跑任务要用到的外部
pub struct Env {
    pub dir: PathBuf,
    pub ocr: Ocr,
}

/// 当场拒绝过遮罩的接口（地址加模型）。只在内存里：重开应用后最多多试一次，不把接口地址写到盘上
static MASKLESS: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);

fn provider(c: &ImageCfg) -> String {
    format!("{}\n{}", c.base_url, c.model)
}

/// 失败的种类，写日志用
fn tag(f: &ImgFail) -> String {
    match f {
        ImgFail::Moderation => "审核拒绝".into(),
        ImgFail::NoRefs => "不能改图".into(),
        ImgFail::MaskRejected => "不接受遮罩".into(),
        ImgFail::Base(Fail::Http(s)) => format!("HTTP {s}"),
        ImgFail::Base(Fail::Timeout) => "超时".into(),
        ImgFail::Base(Fail::Connect) => "连不上".into(),
        ImgFail::Base(_) => "返回的内容不对".into(),
    }
}

fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> T + Send + 'static,
) -> impl Future<Output = Option<T>> {
    let h = tauri::async_runtime::spawn_blocking(f);
    async move { h.await.ok() }
}

/// 发出去、贴回来。成了是要存的结果；没贴好的也有结果要存（花了钱），另带原因
async fn work(
    t: Tasks,
    env: &Env,
    cfg: &ImageCfg,
    id: u64,
    spec: &Spec,
) -> Result<Late, (String, Option<Late>)> {
    let bad = || (imagegen::reason(&ImgFail::Base(Fail::Bad)), None);
    let (live, s) = (spec.live.clone(), spec.spot.clone());
    let made = blocking(move || {
        let (line, range) = job::wide(&live.img, &s.line, s.range);
        let crop = xpaste_pro::te_crop(&live.img, line, range, NEIGHBORS);
        (
            line,
            range,
            crop,
            xpaste_pro::te_prompt(&s.old, &s.new, s.realign()),
        )
    });
    let (line, range, crop, prompt) = made.await.ok_or_else(bad)?;
    let mut mask = spec.mask && !lock(&MASKLESS).contains(&provider(cfg));
    let png = loop {
        if t.canceled(id) {
            return Err((String::new(), None));
        }
        // 删了的图，出了也没处放
        if spec.live.gone(&env.dir) {
            return Err((GONE.into(), None));
        }
        let started = Instant::now();
        let req = EditReq {
            prompt: &prompt,
            crop: &crop.png,
            mask: mask.then_some(&crop.mask[..]),
        };
        let r = imagegen::edit(cfg, req).await;
        log::info!(
            "改字 任务 {id} 发出 1 次请求{} {} 字节 改 {} 字 {} 耗时 {} ms",
            if mask { "（带遮罩）" } else { "" },
            crop.png.len(),
            job::changed_count(&spec.spot.old, &spec.spot.new),
            r.as_ref()
                .map_or_else(tag, |b| format!("收到 {} 字节", b.len())),
            started.elapsed().as_millis()
        );
        match r {
            Ok(png) => break png,
            Err(ImgFail::MaskRejected) if mask => {
                lock(&MASKLESS).insert(provider(cfg));
                mask = false;
            }
            Err(e) => return Err((imagegen::edit_reason(&e), None)),
        }
    };
    let (live, s, edit, ocr) = (spec.live.clone(), spec.spot.clone(), spec.edit, env.ocr);
    let pasted = blocking(move || {
        let started = Instant::now();
        let late = |paste, note, placed| Late {
            task: id,
            edit,
            paste,
            note,
            placed,
            pick: placed,
        };
        let r = xpaste_pro::te_paste(&live.img, line, range, &crop, &png, &s.old, &s.new);
        let out = match r {
            // 范围外面一圈碰到签名：留着看，不放上去
            Ok(p) if live.spoils_locked(&p) => (Some(LOCKED), Some(late(p, LOCKED, false))),
            Ok(p) => {
                let wrong = job::misread(&live.img, &line, &p, &s.new, ocr);
                (None, Some(late(p, if wrong { MISREAD } else { "" }, true)))
            }
            Err(_) => (
                Some(UNALIGNED),
                whole(&png).map(|p| late(p, UNALIGNED, false)),
            ),
        };
        log::info!(
            "改字 任务 {id} 贴回 {} 耗时 {} ms",
            out.0.map_or_else(|| how(out.1.as_ref()), |_| "没贴好"),
            started.elapsed().as_millis()
        );
        out
    });
    match pasted.await.ok_or_else(bad)? {
        (None, Some(l)) => Ok(l),
        (why, l) => Err((why.unwrap_or(UNALIGNED).into(), l)),
    }
}

/// 贴成什么样，写日志用
fn how(l: Option<&Late>) -> &'static str {
    match l.map(|l| (l.paste.exact, l.paste.soft)) {
        Some((true, false)) => "框外没动过",
        Some((true, true)) => "框外没动过（可能有痕迹）",
        Some((false, false)) => "框外动过",
        Some((false, true)) => "框外动过（可能有痕迹）",
        None => "没贴好",
    }
}

/// 没贴好的：接口出的整张图
fn whole(png: &[u8]) -> Option<TePaste> {
    let img = image::load_from_memory(png).ok()?.into_rgba8();
    Some(TePaste {
        patch: TeImg {
            w: img.width(),
            h: img.height(),
            px: img.into_raw(),
        },
        x: 0,
        y: 0,
        exact: false,
        soft: false,
    })
}

/// 跑完一个在改的任务：结果存到发起的那张图，让出名额。返回改好了没有
async fn run(t: Tasks, env: &Env, cfg: &ImageCfg, id: u64, spec: Spec) -> bool {
    let r = work(t, env, cfg, id, &spec).await;
    let (why, late) = match r {
        Ok(l) => (None, Some(l)),
        Err((why, l)) => (Some(why), l),
    };
    let (live, dir) = (spec.live.clone(), env.dir.clone());
    let done = blocking(move || live.land(t, &dir, id, late, why)).await;
    done.unwrap_or_else(|| {
        let why = imagegen::reason(&ImgFail::Base(Fail::Bad));
        t.finish(id, Err(why)) == Some(true)
    })
}

// ---- 运行时 ----

static Q: Mutex<Queue> = Mutex::new(Queue {
    last_id: 0,
    tasks: Vec::new(),
});
/// 发列表时持有：取快照和发出去要成对，免得旧的晚于新的发出
static EMIT: Mutex<()> = Mutex::new(());

pub fn tasks() -> Tasks {
    Tasks {
        q: &Q,
        slots: &gen_slots::SLOTS,
    }
}

/// 记下来给后台补存的用
static APP: OnceLock<AppHandle> = OnceLock::new();

/// 后台补存了结果：告诉页面
pub fn saved_later() {
    if let Some(app) = APP.get() {
        changed(app);
    }
}

/// 任务或图变了：叫醒共用名额的每一家（自己在 `pump` 里接上排队的、发列表）
pub fn changed(app: &AppHandle) {
    gen_slots::freed(app);
}

/// 空出来的名额接上排队的，再把整份列表发给改字窗口。启动时登记给 `gen_slots::on_free`
pub fn pump(app: &AppHandle) {
    APP.get_or_init(|| app.clone());
    let t = tasks();
    loop {
        let next = t.next();
        let Some((id, spec)) = next else { break };
        log::info!("改字 任务 {id} 开始");
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let cfg = crate::meme_gen::image_cfg(&app.state::<AppState>().settings());
            let env = Env {
                dir: textedit::dir(&app),
                ocr: job::ocr,
            };
            if run(t, &env, &cfg, id, spec).await
                && let Some(n) = textedit::missed()
            {
                crate::notify(&app, &format!("改字：有 {n} 处改好了"));
            }
            changed(&app);
        });
    }
    let _emit = EMIT.lock().unwrap_or_else(PoisonError::into_inner);
    let views = t.views();
    crate::events::emit_to(app, textedit::LABEL, crate::events::TE_TASKS, views);
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::imagegen::Format;
    use crate::imagegen::tests::{Reply, Req, b64, fake, json_reply};
    use crate::textedit_job::tests::*;
    use crate::textedit_job::{Read, no};
    use crate::textedit_store::{By, NO_PICK};
    use serde_json::json;
    use std::net::TcpListener;
    use xpaste_platform::{OcrError, OcrLine};
    use xpaste_pro::TeBlock;

    pub fn tasks() -> Tasks {
        Tasks {
            q: Box::leak(Box::default()),
            slots: Box::leak(Box::new(Mutex::new(Slots::new()))),
        }
    }

    fn cfg(base: &str) -> ImageCfg {
        ImageCfg {
            format: Format::Openai,
            base_url: base.into(),
            api_key: "k".into(),
            model: "m".into(),
        }
    }

    type Got = Result<Option<Vec<OcrLine>>, OcrError>;
    fn no_engine(_: &[u8]) -> Got {
        Ok(None)
    }
    fn reads_new(_: &[u8]) -> Got {
        // 分成两段、带空格也是一样的字
        Ok(Some(vec![
            row("到 四点", moved(200.0), vec![]),
            row("会议改", moved(0.0), vec![]),
        ]))
    }
    fn reads_wrong(_: &[u8]) -> Got {
        Ok(Some(vec![row("会议改到西点", moved(0.0), vec![])]))
    }
    fn reads_nothing(_: &[u8]) -> Got {
        Ok(Some(vec![]))
    }
    fn moved(dx: f32) -> xpaste_pro::TeQuad {
        LINE.map(|p| [p[0] + dx, p[1]])
    }

    fn env(dir: &Path, ocr: Ocr) -> Env {
        Env {
            dir: dir.into(),
            ocr,
        }
    }

    /// 跑排头那个
    fn go(t: Tasks, env: &Env, cfg: &ImageCfg) -> bool {
        let (id, spec) = t.next().expect("有能跑的");
        tauri::async_runtime::block_on(run(t, env, cfg, id, spec))
    }

    /// 请求体里的各张 PNG：裁块、遮罩
    fn pngs(body: &[u8]) -> Vec<Vec<u8>> {
        let find = |from: usize, what: &[u8]| {
            body[from..]
                .windows(what.len())
                .position(|w| w == what)
                .map(|i| from + i)
        };
        let (mut out, mut at) = (vec![], 0);
        while let Some(i) = find(at, b"\x89PNG\r\n\x1a\n") {
            let end = find(i, b"IEND").unwrap() + 8;
            out.push(body[i..end].to_vec());
            at = end;
        }
        out
    }

    fn rgba(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).unwrap().into_rgba8()
    }

    fn reply(img: &image::RgbaImage) -> Reply {
        let mut png = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        json_reply(json!({"data": [{"b64_json": b64(&png)}]}))
    }

    /// 像样的模型：遮罩里擦干净、写一个新字，别处不动
    fn model(r: &Req) -> Reply {
        write(r, 80)
    }

    /// 新字的一横写出遮罩一点
    fn spill(r: &Req) -> Reply {
        write(r, 106)
    }

    /// 一竖一横，一横到遮罩宽的百分之 `to`
    fn write(r: &Req, to: u32) -> Reply {
        let p = pngs(&r.body);
        let (mut img, mask) = (rgba(&p[0]), rgba(&p[1]));
        let hole: Vec<(u32, u32)> = mask
            .enumerate_pixels()
            .filter(|p| p.2.0[3] == 0)
            .map(|p| (p.0, p.1))
            .collect();
        let (x0, y0) = hole[0];
        let (x1, y1) = hole[hole.len() - 1];
        let (w, h) = (x1 - x0, y1 - y0);
        for y in y0..=y1 {
            for x in x0..=x1 + w * to.saturating_sub(100) / 100 {
                let (u, v) = ((x - x0) * 100 / w, (y - y0) * 100 / h);
                let on = (44..56).contains(&u) && (20..80).contains(&v)
                    || (20..to).contains(&u) && (62..74).contains(&v);
                if on {
                    img.put_pixel(x, y, image::Rgba([20, 24, 30, 255]));
                } else if x <= x1 {
                    img.put_pixel(x, y, image::Rgba([240, 238, 232, 255]));
                }
            }
        }
        reply(&img)
    }

    fn refused() -> TcpListener {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        l
    }

    fn url(l: &TcpListener) -> String {
        format!("http://127.0.0.1:{}", l.local_addr().unwrap().port())
    }

    fn one(t: Tasks, id: u64) -> TaskView {
        t.views().into_iter().find(|v| v.id == id).unwrap()
    }

    // ---- 发请求 ----

    /// 命中的图、没配接口：不入队，一个请求也没有
    #[test]
    fn blocked_never_runs() {
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live_with(Read {
            blocked: Some(TeBlock::Id),
            ..read()
        });
        let srv = refused();
        let d = draft(NEW, [0.0, 10.0]);
        assert!(t.submit(&tmp.0, &l, &d, true, true).is_err());
        assert!(t.next().is_none() && t.views().is_empty());
        assert!(srv.accept().is_err(), "不应发出请求 {}", url(&srv));
        assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0);
        assert_eq!(
            msg(t.submit(&tmp.0, &live(), &d, true, false)),
            "生图接口还没配好"
        );
        assert!(t.next().is_none());
    }

    /// 手填的原文、新文命中词表
    #[test]
    fn typed_text_checked() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let id = "11010519491231002X";
        let base = draft(NEW, range(4, 5));
        for (old, new) in [(id, "改掉"), (OLD, id)] {
            let d = Draft {
                old: old.into(),
                new: new.into(),
                ..base.clone()
            };
            assert!(l.plan(&d, false, true).is_err());
            assert!(t.submit(&tmp.0, &l, &d, true, true).is_err());
        }
        assert!(t.next().is_none());
        assert_eq!(std::fs::read_dir(&tmp.0).unwrap().count(), 0);
    }

    /// 读字没读完就点了“改”：等它判断完
    #[test]
    fn submit_waits_for_reading() {
        let (t, tmp) = (tasks(), Tmp::new());
        let l = crate::textedit_job::tests::unread();
        let (a, dir) = (l.clone(), tmp.0.clone());
        let h = std::thread::spawn(move || {
            t.submit(&dir, &a, &draft(NEW, [0.0, 9.0]), true, true)
                .is_err()
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!h.is_finished());
        crate::textedit_job::tests::set_read(
            &l,
            Read {
                blocked: Some(TeBlock::Id),
                ..read()
            },
        );
        assert!(h.join().unwrap());
        assert!(t.views().is_empty());
    }

    /// 只发裁块和遮罩，都是 1024 × 1024，不是整图；改好后放上去
    #[test]
    fn run_sends_crop_only() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (url, h) = fake(1, model);
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        assert_eq!(
            (one(t, id).status, one(t, id).job == l.id),
            (Status::Waiting, true)
        );
        assert!(go(t, &env(&tmp.0, reads_new), &cfg(&url)));
        let seen = h.join().unwrap();
        assert_eq!(seen.len(), 1);
        let p = pngs(&seen[0].body);
        assert_eq!(p.len(), 2);
        for p in &p {
            assert_eq!(rgba(p).dimensions(), (1024, 1024));
        }
        let body = String::from_utf8_lossy(&seen[0].body).into_owned();
        assert!(body.contains("name=\"prompt\"") && body.contains("【四】"));
        let e = edit_of(&l, edit);
        assert_eq!((e.pick, e.cands.len()), (0, 1));
        let c = &e.cands[0];
        assert!(c.placed && c.by == By::Ai && c.note.is_empty());
        assert_eq!(
            (one(t, id).status, one(t, id).reason.as_str()),
            (Status::Done, "")
        );
        // 行外的像素不变
        let out = rgba(&l.render(&tmp.0).unwrap());
        assert!(out.as_raw()[..640 * 60 * 4] == l.img.px[..640 * 60 * 4]);
        assert!(out.as_raw() != &l.img.px);
        t.sweep();
        assert!(t.views().is_empty());
    }

    /// 加字：裁块围着行尾外的空白，写得到原来那行的外面
    #[test]
    fn insert_writes_past_line_end() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (url, _h) = fake(1, model);
        t.submit(
            &tmp.0,
            &l,
            &draft("会议改到三点半", range(6, 6)),
            true,
            true,
        )
        .unwrap();
        assert!(go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        let out = rgba(&l.render(&tmp.0).unwrap());
        let hit = out
            .enumerate_pixels()
            .filter(|p| p.2.0[0] < 100 && p.0 > 430)
            .count();
        assert!(hit > 100, "{hit}");
        assert!((0..200).all(|y| (0..420).all(|x| {
            let i = ((y * 640 + x) * 4) as usize;
            out.get_pixel(x, y).0[..] == l.img.px[i..i + 4]
        })));
    }

    #[test]
    fn wrong_text_noted() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let cases: [(Ocr, &str); 4] = [
            (reads_wrong, MISREAD),
            (reads_new, ""),
            (no_engine, ""),
            (reads_nothing, ""),
        ];
        for (ocr, note) in cases {
            let (t, tmp) = (tasks(), Tmp::new());
            let l = live();
            let (url, _h) = fake(1, model);
            let (_, edit) = t
                .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
                .unwrap();
            assert!(go(t, &env(&tmp.0, ocr), &cfg(&url)));
            let e = edit_of(&l, edit);
            assert_eq!(
                (e.cands[0].note.as_str(), e.cands[0].placed, e.pick),
                (note, true, 0)
            );
        }
    }

    /// 遮罩被拒：去掉遮罩重发一次，只这一次；这个接口以后都不带，别的模型照带
    #[test]
    fn mask_rejected_resends_once() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let masked = |r: &Req| String::from_utf8_lossy(&r.body).contains("name=\"mask\"");
        let (url, h) = fake(5, move |r| {
            let m2 = String::from_utf8_lossy(&r.body).contains("\r\n\r\nm2\r\n");
            let s = if masked(r) || m2 { 400 } else { 500 };
            (s, "text/plain", b"x".to_vec())
        });
        let (id, _) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let env = env(&tmp.0, no_engine);
        assert!(!go(t, &env, &cfg(&url)));
        assert_eq!(one(t, id).reason, "生图接口出错（HTTP 500）");
        t.retry(&tmp.0, id, true, true, |_| Ok(l.clone())).unwrap();
        go(t, &env, &cfg(&url));
        t.retry(&tmp.0, id, true, true, |_| Ok(l.clone())).unwrap();
        // 另一个模型：照带，被拒后不带的也是 400，不再发
        let other = ImageCfg {
            model: "m2".into(),
            ..cfg(&url)
        };
        go(t, &env, &other);
        let seen = h.join().unwrap();
        let with: Vec<bool> = seen.iter().map(masked).collect();
        assert_eq!(with, [true, false, false, true, false]);
        assert_eq!(one(t, id).reason, "这个模型不能改图，换一个模型再试");
    }

    /// 没改出来的不自动重发：限流、网关超时、断线
    #[test]
    fn timeout_and_429_do_not_resend() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        for s in [429u16, 504, 408, 500] {
            let (t, tmp) = (tasks(), Tmp::new());
            let l = live();
            let (url, h) = fake(1, move |_| (s, "text/plain", b"x".to_vec()));
            let (id, edit) = t
                .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
                .unwrap();
            assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
            assert_eq!(h.join().unwrap().len(), 1);
            let v = one(t, id);
            // 再发就会是“连不上”
            let want = imagegen::edit_reason(&ImgFail::Base(Fail::Http(s)));
            assert_eq!((v.status, v.reason), (Status::Failed, want));
            assert_eq!(
                (edit_of(&l, edit).pick, edit_of(&l, edit).cands.len()),
                (NO_PICK, 0)
            );
        }
        // 发出去之后断线
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let srv = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = url(&srv);
        let h = std::thread::spawn(move || {
            drop(srv.accept().unwrap());
            std::thread::sleep(std::time::Duration::from_millis(300));
            srv.set_nonblocking(true).unwrap();
            srv.accept().is_err()
        });
        t.submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert!(h.join().unwrap(), "断线后又发了一次");
    }

    /// “不带遮罩再试”：发 1 次，不带
    #[test]
    fn retry_without_mask() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (url, h) = fake(2, |_| (429, "text/plain", b"x".to_vec()));
        let (id, _) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let env = env(&tmp.0, no_engine);
        go(t, &env, &cfg(&url));
        assert_eq!(
            msg(t.retry(&tmp.0, id, false, false, |_| Ok(l.clone()))),
            "生图接口还没配好"
        );
        t.retry(&tmp.0, id, false, true, |_| Ok(l.clone())).unwrap();
        assert_eq!(
            (one(t, id).status, one(t, id).reason.as_str()),
            (Status::Waiting, "")
        );
        go(t, &env, &cfg(&url));
        let seen = h.join().unwrap();
        assert_eq!(
            seen.iter().map(|r| pngs(&r.body).len()).collect::<Vec<_>>(),
            [2, 1]
        );
        // 图删了、图不给改了都不重试
        assert_eq!(
            msg(t.retry(&tmp.0, id, true, true, |_| Err(no("没了")))),
            "没了"
        );
        let bad = live_with(Read {
            blocked: Some(TeBlock::Id),
            ..read()
        });
        assert!(t.retry(&tmp.0, id, true, true, |_| Ok(bad)).is_err());
        assert_eq!(one(t, id).status, Status::Failed);
        t.drop(id);
        assert_eq!(
            msg(t.retry(&tmp.0, id, true, true, |_| panic!())),
            TASK_GONE
        );
    }

    /// 还没发就取消了：不发
    #[test]
    fn canceled_before_send_sends_nothing() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let srv = refused();
        let (id, _) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let (got, spec) = t.next().unwrap();
        assert_eq!(got, id);
        t.cancel(id);
        let env = env(&tmp.0, no_engine);
        assert!(!tauri::async_runtime::block_on(run(
            t,
            &env,
            &cfg(&url(&srv)),
            id,
            spec
        )));
        assert!(srv.accept().is_err());
        assert!(t.views().is_empty() && lock(t.q).tasks.is_empty());
    }

    // ---- 结果 ----

    /// 取消之后到的：进候选，不选中，列表里也没有；让出名额
    #[test]
    fn late_result_kept() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let (url, _h) = fake(1, move |r| {
            t.cancel(id);
            model(r)
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        let e = edit_of(&l, edit);
        assert_eq!(
            (e.pick, e.cands.len(), e.cands[0].placed),
            (NO_PICK, 1, true)
        );
        assert!(t.views().is_empty());
        assert_eq!(store::list(&tmp.0).len(), 1);
        let mut s = lock(t.slots);
        s.enqueue(Who::Meme, 1);
        s.enqueue(Who::Meme, 2);
        assert_eq!((s.take(Who::Meme), s.take(Who::Meme)), (Some(1), Some(2)));
    }

    /// 被同一个地方的后一处替换的：晚到的不选中，画面上是后一处
    #[test]
    fn canceled_late_result_does_not_select() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (_, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let (url, _h) = fake(1, {
            let (l, dir) = (l.clone(), tmp.0.clone());
            move |r| {
                put_local(
                    t,
                    &dir,
                    &l,
                    &draft("会议改到五点", range(4, 5)),
                    4,
                    [255, 0, 0],
                )
                .unwrap();
                model(r)
            }
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert_eq!(
            (edit_of(&l, edit).pick, edit_of(&l, edit).cands.len()),
            (NO_PICK, 1)
        );
        assert_eq!(at(&l, &tmp.0, 76 + 260, 90), [255, 0, 0]);
    }

    /// 开着另一张：落在发起的那张，窗口早就放开它了
    #[test]
    fn result_goes_to_its_job() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let (a, b) = (live(), live());
        let (id, edit) = t
            .submit(&tmp.0, &a, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        put_local(t, &tmp.0, &b, &draft(NEW, range(1, 2)), 1, [255, 0, 0]).unwrap();
        let job = a.id.clone();
        drop(a);
        let (url, _h) = fake(1, model);
        assert!(go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert_eq!(one(t, id).job, job);
        let back = store::load(&tmp.0, &job).unwrap();
        let e = back.edits.iter().find(|e| e.id == edit).unwrap();
        assert_eq!((e.pick, e.cands.len()), (0, 1));
        assert_eq!(
            (b.view().edits.len(), b.view().edits[0].cands.len()),
            (1, 1)
        );
        // 结束了就放开那张图
        assert!(lock(t.q).tasks.iter().all(|t| t.live.is_none()));
    }

    /// 对不上的图留在候选里，不放上画面
    #[test]
    fn unplaced_kept() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (url, _h) = fake(1, |_| {
            let mut x = 1u32;
            reply(&image::RgbaImage::from_fn(1024, 1024, |_, _| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                image::Rgba([x as u8, (x >> 8) as u8, (x >> 16) as u8, 255])
            }))
        });
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        let e = edit_of(&l, edit);
        assert_eq!(
            (e.pick, e.cands.len(), e.cands[0].placed),
            (NO_PICK, 1, false)
        );
        assert_eq!(e.cands[0].note, UNALIGNED);
        assert_eq!(
            (one(t, id).status, one(t, id).reason.as_str()),
            (Status::Failed, UNALIGNED)
        );
        assert!(rgba(&l.render(&tmp.0).unwrap()).as_raw() == &l.img.px);
        assert_eq!(
            rgba(&l.cand_png(&tmp.0, &e.cands[0].file).unwrap()).dimensions(),
            (1024, 1024)
        );
        // 选不了它
        assert!(job::pick(t, &tmp.0, &l, edit, 0).is_err());
    }

    /// “用 AI 重写”没改出来：本机的还选着
    #[test]
    fn failed_rewrite_keeps_local() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let d = draft(NEW, range(4, 5));
        let edit = put_local(t, &tmp.0, &l, &d, 4, [255, 0, 0]).unwrap();
        let again = Draft {
            edit: Some(edit),
            range: Some(edit_of(&l, edit).range),
            ..d
        };
        let (url, _h) = fake(2, {
            let n = std::sync::atomic::AtomicU32::new(0);
            move |r| match n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => (500, "text/plain", vec![]),
                _ => model(r),
            }
        });
        let (id, same) = t.submit(&tmp.0, &l, &again, true, true).unwrap();
        assert_eq!(same, edit);
        let env = env(&tmp.0, no_engine);
        assert!(!go(t, &env, &cfg(&url)));
        assert_eq!(
            (edit_of(&l, edit).pick, edit_of(&l, edit).cands.len()),
            (0, 1)
        );
        // 再来一次成了：选新的，本机那张留着
        t.retry(&tmp.0, id, true, true, |_| Ok(l.clone())).unwrap();
        assert!(go(t, &env, &cfg(&url)));
        assert_eq!(
            (edit_of(&l, edit).pick, edit_of(&l, edit).cands.len()),
            (1, 2)
        );
    }

    /// 图刚被删：结果留在内存，撤销后补存进去
    #[test]
    fn deleted_job_result_saved_on_restore() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        put_local(
            t,
            &tmp.0,
            &l,
            &draft("会政改到三点", range(1, 2)),
            1,
            [255, 0, 0],
        )
        .unwrap();
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let job = l.id.clone();
        drop(l);
        let (url, _h) = fake(1, {
            let (dir, job) = (tmp.0.clone(), job.clone());
            move |r| {
                store::remove(&dir, &job).unwrap();
                model(r)
            }
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert_eq!(
            (one(t, id).status, one(t, id).reason.as_str()),
            (Status::Failed, GONE)
        );
        assert!(store::list(&tmp.0).is_empty());
        textedit::restore(t, &tmp.0, &job).unwrap();
        let back = store::load(&tmp.0, &job).unwrap();
        let e = back.edits.iter().find(|e| e.id == edit).unwrap();
        assert_eq!((e.pick, e.cands.len()), (0, 1));
        assert_eq!(
            (one(t, id).status, one(t, id).reason.as_str()),
            (Status::Done, "")
        );
    }

    /// 出了图只是没存进去：重试先补存，不再发请求；补不上的报错，也不发
    #[cfg(unix)]
    #[test]
    fn retry_saves_instead_of_paying_again() {
        use std::os::unix::fs::PermissionsExt;
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let mode = {
            let dir = tmp.0.join(&l.id);
            move |m| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(m)).unwrap()
        };
        let (url, h) = fake(1, {
            let mode = mode.clone();
            move |r| {
                mode(0o555);
                model(r)
            }
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert_eq!(h.join().unwrap().len(), 1);
        assert_eq!(one(t, id).status, Status::Failed);
        let why = one(t, id).reason;
        assert_eq!(msg(t.retry(&tmp.0, id, true, true, |_| Ok(l.clone()))), why);
        assert!(t.next().is_none() && one(t, id).status == Status::Failed);
        mode(0o755);
        // 接口没配也能补存
        t.retry(&tmp.0, id, true, false, |_| Ok(l.clone())).unwrap();
        assert!(t.next().is_none());
        assert_eq!(
            (one(t, id).status, edit_of(&l, edit).pick),
            (Status::Done, 0)
        );
        let back = store::load(&tmp.0, &l.id).unwrap();
        assert_eq!(back.edits[0].cands.len(), 1);
    }

    /// 取消了的又没存进去：补存之后也不选中
    #[cfg(unix)]
    #[test]
    fn canceled_unsaved_result_not_selected() {
        use std::os::unix::fs::PermissionsExt;
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let dir = tmp.0.join(&l.id);
        let mode =
            |d: &Path, m| std::fs::set_permissions(d, std::fs::Permissions::from_mode(m)).unwrap();
        let (url, _h) = fake(1, {
            let dir = dir.clone();
            move |r| {
                t.cancel(id);
                mode(&dir, 0o555);
                model(r)
            }
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        assert!(edit_of(&l, edit).cands.is_empty());
        mode(&dir, 0o755);
        job::retry_late(t, &tmp.0, &l.id);
        let e = edit_of(&l, edit);
        assert_eq!((e.pick, e.cands.len()), (NO_PICK, 1));
    }

    /// 没存进去的过一会儿自己补存
    #[cfg(unix)]
    #[test]
    fn unsaved_result_saved_later() {
        use std::os::unix::fs::PermissionsExt;
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let (id, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let mode = {
            let dir = tmp.0.join(&l.id);
            move |m| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(m)).unwrap()
        };
        let (url, _h) = fake(1, {
            let mode = mode.clone();
            move |r| {
                mode(0o555);
                model(r)
            }
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        // 第一次补存还是不行，下一次成
        std::thread::sleep(std::time::Duration::from_millis(1300));
        assert_eq!(one(t, id).status, Status::Failed);
        mode(0o755);
        for _ in 0..40 {
            if one(t, id).status == Status::Done {
                assert_eq!(edit_of(&l, edit).pick, 0);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("没有补存");
    }

    /// 删了图：排队的拿掉、撤销也不恢复；轮到了还没发的不发；没改出来的不重试
    #[test]
    fn deleted_job_sends_nothing() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let (l, other) = (live(), live());
        let sub = |l: &Arc<Live>, k: usize, new: &str| {
            t.submit(&tmp.0, l, &draft(new, range(k, k + 1)), true, true)
                .unwrap()
                .0
        };
        let first = sub(&l, 4, NEW);
        let second = sub(&l, 1, "会政改到三点");
        let kept = sub(&other, 4, NEW);
        let (id, spec) = t.next().unwrap();
        assert_eq!(id, first);
        textedit::remove(t, &tmp.0, &l.id).unwrap();
        let ids = |t: Tasks| t.views().iter().map(|v| v.id).collect::<Vec<_>>();
        assert_eq!(ids(t), [first, kept]);
        let srv = refused();
        let env = env(&tmp.0, no_engine);
        assert!(!tauri::async_runtime::block_on(run(
            t,
            &env,
            &cfg(&url(&srv)),
            id,
            spec
        )));
        assert!(srv.accept().is_err(), "删了的还发");
        assert_eq!(one(t, first).reason, GONE);
        assert_eq!(
            msg(t.retry(&tmp.0, first, true, true, |_| Ok(l.clone()))),
            GONE
        );
        textedit::restore(t, &tmp.0, &l.id).unwrap();
        assert!(!ids(t).contains(&second));
        t.retry(&tmp.0, first, true, true, |_| Ok(l.clone()))
            .unwrap();
        // 清空也一样；还没出过结果的图不在清空里
        put_local(
            t,
            &tmp.0,
            &l,
            &draft("会议正到三点", range(2, 3)),
            2,
            [255, 0, 0],
        )
        .unwrap();
        textedit::clear(t, &tmp.0).unwrap();
        assert_eq!(ids(t), [kept]);
    }

    /// 连着提交两次一样的只发一次；改好之后再提交是再来一张
    #[test]
    fn same_submit_twice_is_one_task() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let d = draft("会议改到三点半", range(6, 6));
        let first = t.submit(&tmp.0, &l, &d, true, true).unwrap();
        assert_eq!(t.submit(&tmp.0, &l, &d, true, true).unwrap(), first);
        let (id, _spec) = t.next().unwrap();
        assert_eq!(t.submit(&tmp.0, &l, &d, true, true).unwrap(), first);
        assert_eq!((l.view().edits.len(), t.views().len()), (1, 1));
        t.finish(id, Ok(()));
        assert_ne!(t.submit(&tmp.0, &l, &d, true, true).unwrap().0, id);
    }

    /// 重写到一半取消：结果留着，选中的还是原来那张
    #[test]
    fn canceled_rewrite_keeps_old_pick() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let d = draft(NEW, range(4, 5));
        let edit = put_local(t, &tmp.0, &l, &d, 4, [255, 0, 0]).unwrap();
        let again = Draft {
            edit: Some(edit),
            range: Some(edit_of(&l, edit).range),
            ..d
        };
        let (id, _) = t.submit(&tmp.0, &l, &again, true, true).unwrap();
        let (url, _h) = fake(1, move |r| {
            t.cancel(id);
            model(r)
        });
        assert!(!go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        let e = edit_of(&l, edit);
        assert_eq!((e.pick, e.cands.len()), (0, 2));
        let back = store::load(&tmp.0, &l.id).unwrap();
        assert_eq!(back.edits[0].pick, 0);
    }

    /// 写出范围、碰到紧挨着的签名：不放上去，图留着
    #[test]
    fn result_touching_signature_not_placed() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let r = range(4, 5);
        let s = xpaste_pro::te_strip(&scene(), LINE);
        let right = 76.0 + r[1] * 352.0 / s.img.w as f32 + 0.5;
        let sig = [[right, 64.0], [640.0, 64.0], [640.0, 126.0], [right, 126.0]];
        for locked in [vec![], vec![sig]] {
            let l = live_with(Read {
                locked: locked.clone(),
                ..read()
            });
            let (url, _h) = fake(1, spill);
            let (id, edit) = t.submit(&tmp.0, &l, &draft(NEW, r), true, true).unwrap();
            let ok = go(t, &env(&tmp.0, no_engine), &cfg(&url));
            let e = edit_of(&l, edit);
            assert_eq!(
                (ok, e.cands[0].placed),
                (locked.is_empty(), locked.is_empty())
            );
            if !ok {
                assert_eq!((e.pick, one(t, id).reason.as_str()), (NO_PICK, LOCKED));
            }
        }
    }

    /// 清空时还在改的：结果到了重新落盘，钱不白花
    #[test]
    fn cleared_job_result_saved() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        put_local(
            t,
            &tmp.0,
            &l,
            &draft("会政改到三点", range(1, 2)),
            1,
            [255, 0, 0],
        )
        .unwrap();
        let (_, edit) = t
            .submit(&tmp.0, &l, &draft(NEW, range(4, 5)), true, true)
            .unwrap();
        let (url, _h) = fake(1, {
            let (dir, l) = (tmp.0.clone(), l.clone());
            move |r| {
                store::clear(&dir).unwrap();
                l.wiped(t, &dir);
                model(r)
            }
        });
        assert!(go(t, &env(&tmp.0, no_engine), &cfg(&url)));
        let back = store::load(&tmp.0, &l.id).unwrap();
        assert_eq!(back.edits.iter().filter(|e| e.shown().is_some()).count(), 1);
        assert_eq!(edit_of(&l, edit).pick, 0);
        assert!(l.render(&tmp.0).is_ok());
    }

    // ---- 队列 ----

    /// 三个同时改两个；“前面还有 N 个”；让出名额接上
    #[test]
    fn two_run_rest_wait() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        let (t, tmp) = (tasks(), Tmp::new());
        let l = live();
        let sub = |k: usize, new: &str| {
            t.submit(&tmp.0, &l, &draft(new, range(k, k + 1)), true, true)
                .unwrap()
                .0
        };
        let ids = [
            sub(0, "公议改到三点"),
            sub(1, "会政改到三点"),
            sub(2, "会议政到三点"),
            sub(3, "会议改道三点"),
        ];
        let next = || t.next().map(|n| n.0);
        assert_eq!((next(), next(), next()), (Some(ids[0]), Some(ids[1]), None));
        assert_eq!(ids.map(|i| one(t, i).ahead), [0, 0, 0, 1]);
        let v = one(t, ids[0]);
        assert!(v.status == Status::Running && v.started_at.is_some());
        assert_eq!((v.old.as_str(), v.new.as_str()), (OLD, "公议改到三点"));
        assert_eq!(t.finish(ids[0], Err("超时".into())), Some(false));
        assert_eq!(next(), Some(ids[2]));
        // 取消排队的直接拿掉；在改的占着名额等结果
        t.cancel(ids[3]);
        t.cancel(ids[1]);
        assert_eq!(
            t.views().iter().map(|v| v.id).collect::<Vec<_>>(),
            [ids[0], ids[2]]
        );
        let five = sub(5, "会议改到三占");
        assert_eq!(next(), None);
        assert_eq!(t.finish(ids[1], Ok(())), None);
        assert_eq!(next(), Some(five));
        // 改好的、没改出来的去掉；在改的去不掉
        t.drop(five);
        assert_eq!(t.finish(five, Ok(())), Some(true));
        t.drop(five);
        t.drop(ids[0]);
        assert_eq!(t.views().iter().map(|v| v.id).collect::<Vec<_>>(), [ids[2]]);
        // 没改出来的：被替换时从列表拿掉，候选照旧
        assert_eq!(t.finish(ids[2], Err("超时".into())), Some(false));
        put_local(
            t,
            &tmp.0,
            &l,
            &draft("会议正到三点", range(2, 3)),
            2,
            [255, 0, 0],
        )
        .unwrap();
        assert!(t.views().is_empty());
    }

    /// 给页面的字段名
    #[test]
    fn view_fields() {
        let v = TaskView {
            id: 1,
            job: "j".into(),
            edit: 2,
            status: Status::Running,
            ahead: 0,
            started_at: Some(3),
            reason: String::new(),
            old: "a".into(),
            new: "b".into(),
        };
        assert_eq!(
            serde_json::to_value(v).unwrap(),
            json!({"id": 1, "job": "j", "edit": 2, "status": "running", "ahead": 0, "startedAt": 3,
                "reason": "", "old": "a", "new": "b"})
        );
    }

    /// 走一遍成的、没成的、被拒的，日志里没有原文、新文、识别的字、地址
    #[test]
    fn no_text_in_log() {
        if !xpaste_pro::AVAILABLE {
            return eprintln!("这个构建不含 Pro，跳过");
        }
        static LOG: Mutex<String> = Mutex::new(String::new());
        struct Keep;
        impl log::Log for Keep {
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }
            fn log(&self, r: &log::Record) {
                if r.target().starts_with("xpaste") {
                    lock(&LOG).push_str(&format!("{}\n", r.args()));
                }
            }
            fn flush(&self) {}
        }
        log::set_boxed_logger(Box::new(Keep)).unwrap();
        log::set_max_level(log::LevelFilter::Info);

        let (t, tmp) = (tasks(), Tmp::new());
        let (old, new) = ("鑫淼焱垚犇骉", "鑫淼焱垚羴骉");
        fn reads(_: &[u8]) -> Got {
            Ok(Some(vec![row("鑫淼焱垚麤骉", LINE, vec![])]))
        }
        let l = crate::textedit_job::tests::unread();
        crate::textedit_job::tests::start(&l, reads);
        let d = Draft {
            old: old.into(),
            ..draft(new, range(4, 5))
        };
        l.plan(&d, false, true).unwrap();
        let (url, _h) = fake(2, {
            let n = std::sync::atomic::AtomicU32::new(0);
            move |r| match n.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => (429, "text/plain", "鑫淼".into()),
                _ => model(r),
            }
        });
        let (id, _) = t.submit(&tmp.0, &l, &d, true, true).unwrap();
        let env = env(&tmp.0, reads);
        assert!(!go(t, &env, &cfg(&url)));
        t.retry(&tmp.0, id, true, true, |_| Ok(l.clone())).unwrap();
        assert!(go(t, &env, &cfg(&url)));
        let bad = Draft {
            new: "身份证号11010519491231002X".into(),
            ..d
        };
        assert!(t.submit(&tmp.0, &l, &bad, true, true).is_err());
        let log = lock(&LOG).clone();
        assert!(
            log.contains("改字 任务") && log.contains("HTTP 429") && log.contains("改字 读字"),
            "{log}"
        );
        let port = url.rsplit(':').next().unwrap();
        for s in [
            "鑫",
            "淼",
            "犇",
            "羴",
            "麤",
            "11010519491231002X",
            port,
            &l.id,
        ] {
            assert!(!log.contains(s), "{s}");
        }
    }
}
