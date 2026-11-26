//! 表情包制作的 AI 任务队列：每次生成是一个任务，不跟窗口走；同时最多 2 个在画，其余排队
//! （名额和排队的先后与改字共用，见 `gen_slots`）。
//! 状态一变就把整份任务列表发给制作窗口和面板。只在内存里，不存盘。
//!
//! `Queue` 是纯数据结构（不联网、不碰窗口），运行时是下面薄薄一层：全局锁、`pump`、异步任务。

use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::gen_slots::{self, Slots, Who};
use crate::meme_gen::{self, GenOut, GenReq};
use crate::state::AppState;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Queue,
    Run,
    Done,
    Fail,
}

/// 发给前端的一个任务。带用户写的标题和做法，`Debug` 里不出现它们。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    pub id: u64,
    /// one | set | act
    pub kind: &'static str,
    pub state: TaskState,
    /// 页面给的那句话，原样带回
    pub title: String,
    /// 内置风格的键名；自定义时为“自定义”
    pub style: String,
    /// 开始画的 Unix 毫秒，秒数由前端自己走；还没开始为 None
    pub started_ms: Option<u64>,
    /// 排在它前面、还在排队的个数（改字的也算）；不在排队的为 0
    pub ahead: usize,
    /// 画好的结果在表情包库里的 id
    pub ids: Vec<i64>,
    /// 网格没画规整：整张图存成的那一个（这时 `ids` 为空）
    pub irregular: Option<i64>,
    /// 没画出来的原因
    pub error: String,
    /// 发起时的做法 JSON，原样带回
    pub scene: String,
}

impl std::fmt::Debug for TaskView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskView")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("state", &self.state)
            .field("ahead", &self.ahead)
            .field("ids", &self.ids)
            .field("irregular", &self.irregular)
            .finish_non_exhaustive()
    }
}

struct Task {
    view: TaskView,
    req: GenReq,
    /// 在画时被取消：不再显示，等结果回来（照样进库）再拿掉、让出名额
    cancelled: bool,
}

/// 任务按排队先后排（重试的挪到最后）
#[derive(Default)]
pub struct Queue {
    last_id: u64,
    tasks: Vec<Task>,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.tasks.iter().map(|t| &t.view))
            .finish()
    }
}

/// 一套的网格没画规整、用户选了“分 4 次重画”：一个任务里顺序发 4 次 `set_part`
pub const SET_PARTS: &str = "set_parts";
/// 分 4 次里某一次又没画规整：不再弹选择，任务算没画出来
const PART_SHAKY: &str = "这次没画规整，再试一次";

/// 列表里显示的种类；不认识的生成方式为 None
fn kind_of(req: &GenReq) -> Option<&'static str> {
    Some(match req.kind.as_str() {
        "one" => "one",
        "set" | "set_part" | SET_PARTS => "set",
        "act" => "act",
        _ => return None,
    })
}

/// 分 4 次重画：依次跑第 0–3 部分，结果 id 按顺序拼起来。某一部分失败或没画规整就停下、返回原因
/// （已画好的几部分在生成时就进了库）；被取消了不再发后面的部分
pub async fn run_parts<F, Fut>(mut part: F, cancelled: impl Fn() -> bool) -> Result<GenOut, String>
where
    F: FnMut(u8, Option<i64>) -> Fut,
    Fut: std::future::Future<Output = Result<GenOut, String>>,
{
    let mut ids = Vec::new();
    for k in 0..4 {
        if cancelled() {
            return Err(meme_gen::LATE.into());
        }
        // 后面几次带上第一格，画同一个角色
        match part(k, ids.first().copied()).await? {
            GenOut::Done { ids: got } => ids.extend(got),
            GenOut::Irregular { .. } => return Err(PART_SHAKY.into()),
        }
    }
    Ok(GenOut::Done { ids })
}

impl Queue {
    pub const fn new() -> Self {
        Self {
            last_id: 0,
            tasks: Vec::new(),
        }
    }

    fn at(&self, id: u64) -> Option<usize> {
        self.tasks.iter().position(|t| t.view.id == id)
    }

    /// 入队，返回任务号；不认识的生成方式不入队
    /// 领一个任务号：页面先领号、记成“这个窗口在等的”，再拿它入队，任务列表一来就认得是不是自己的
    pub fn reserve(&mut self) -> u64 {
        self.last_id += 1;
        self.last_id
    }

    /// 拿领来的号入队；不认识的生成方式、不是领来的号、这个号已经在队里都不入队
    pub fn push(
        &mut self,
        slots: &mut Slots,
        id: u64,
        req: GenReq,
        title: String,
        scene: String,
    ) -> Result<(), String> {
        let kind = kind_of(&req).ok_or("不认识的生成方式")?;
        if id == 0 || id > self.last_id || self.at(id).is_some() {
            return Err("任务号不对".into());
        }
        let style = if req.style.is_empty() {
            "自定义".to_owned()
        } else {
            req.style.clone()
        };
        self.tasks.push(Task {
            view: TaskView {
                id,
                kind,
                state: TaskState::Queue,
                title,
                style,
                started_ms: None,
                ahead: 0,
                ids: Vec::new(),
                irregular: None,
                error: String::new(),
                scene,
            },
            req,
            cancelled: false,
        });
        slots.enqueue(Who::Meme, id);
        Ok(())
    }

    /// 轮到自己且有名额时，把排头标成在画并返回它（取消了还没回来的也占名额）。
    /// 排着的号在列表里已经没有了的，让出刚占的名额、看下一个
    pub fn next(&mut self, slots: &mut Slots) -> Option<u64> {
        let (id, i) = loop {
            let id = slots.take(Who::Meme)?;
            match self.at(id) {
                Some(i) => break (id, i),
                None => slots.release(),
            }
        };
        let v = &mut self.tasks[i].view;
        v.state = TaskState::Run;
        v.started_ms = Some(u64::try_from(crate::clock::now_ms()).unwrap_or(0));
        Some(id)
    }

    /// 在画时被取消了（或已经不在了）：分 4 次重画据此不再发后面的部分
    pub fn cancelled(&self, id: u64) -> bool {
        self.at(id).is_none_or(|i| self.tasks[i].cancelled)
    }

    /// 这个任务的生成请求（交给 `meme_gen::generate`）
    pub fn req(&self, id: u64) -> Option<GenReq> {
        self.at(id).map(|i| self.tasks[i].req.clone())
    }

    /// 在画的结束了（结果已由 `generate` 存进库），让出名额。返回该发的通知正文；已取消、不在了的返回 None
    pub fn finish(
        &mut self,
        slots: &mut Slots,
        id: u64,
        r: Result<GenOut, String>,
    ) -> Option<String> {
        let i = self.at(id)?;
        slots.release();
        if self.tasks[i].cancelled {
            self.tasks.remove(i);
            return None;
        }
        let v = &mut self.tasks[i].view;
        Some(match r {
            Ok(out) => {
                v.state = TaskState::Done;
                match out {
                    GenOut::Done { ids } => v.ids = ids,
                    GenOut::Irregular { id } => v.irregular = Some(id),
                }
                format!("“{}”画好了，已存进表情包", v.title)
            }
            Err(e) => {
                v.state = TaskState::Fail;
                v.error = e;
                // 内部的原因标记不露给用户（页面的 reasonText 同一说法）
                let why = meme_gen::reason_text(&v.error);
                format!("“{}”没画出来：{}", v.title, why)
            }
        })
    }

    /// 取消：排队的直接拿掉；在画的不再显示，请求不中止（名额等它回来才让），结果回来照样进库
    pub fn cancel(&mut self, slots: &mut Slots, id: u64) {
        let Some(i) = self.at(id) else { return };
        match self.tasks[i].view.state {
            TaskState::Queue => {
                self.tasks.remove(i);
                slots.remove(Who::Meme, id);
            }
            TaskState::Run => self.tasks[i].cancelled = true,
            TaskState::Done | TaskState::Fail => {}
        }
    }

    /// 没画出来的重试：重新入队，回到队尾
    /// 返回重试上了没有：任务已经不在、不是没画出来的都不动
    pub fn retry(&mut self, slots: &mut Slots, id: u64) -> bool {
        let Some(i) = self.at(id) else { return false };
        if self.tasks[i].view.state != TaskState::Fail {
            return false;
        }
        let mut t = self.tasks.remove(i);
        t.view.state = TaskState::Queue;
        t.view.error.clear();
        t.view.started_ms = None;
        self.tasks.push(t);
        slots.enqueue(Who::Meme, id);
        true
    }

    /// 去掉画好的、没画出来的（在画、排队的用 `cancel`）
    pub fn drop(&mut self, id: u64) {
        if let Some(i) = self.at(id)
            && matches!(self.tasks[i].view.state, TaskState::Done | TaskState::Fail)
        {
            self.tasks.remove(i);
        }
    }

    /// 回到那次：这个任务、要摆进预览的结果（网格没画规整的是那一张）、看过之后收不收下。
    /// 画好又画规整的才收下；没画规整的要在三选一里选过才走，免得三选一再也拿不回来
    pub fn reopen(&self, id: u64) -> Option<(TaskView, Vec<i64>, bool)> {
        let v = &self.tasks[self.at(id)?].view;
        let ids = v.irregular.map_or_else(|| v.ids.clone(), |x| vec![x]);
        let take = v.state == TaskState::Done && v.irregular.is_none();
        Some((v.clone(), ids, take))
    }

    /// 整份列表，新的在前；取消了还没回来的不在里面
    pub fn views(&self, slots: &Slots) -> Vec<TaskView> {
        self.tasks
            .iter()
            .rev()
            .filter(|t| !t.cancelled)
            .map(|t| TaskView {
                ahead: slots.ahead(Who::Meme, t.view.id),
                ..t.view.clone()
            })
            .collect()
    }
}

// ---- 运行时 ----

static Q: Mutex<Queue> = Mutex::new(Queue::new());
/// 发列表时持有：取快照和发出去要成对，免得两处同时变化时旧快照晚于新快照发出、前端拿旧列表覆盖新的。
/// 和 `Q` 分开，发事件时不占着队列；`emit_to` 不会回调进这个模块，不会死锁
static EMIT: Mutex<()> = Mutex::new(());

fn q() -> MutexGuard<'static, Queue> {
    Q.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 任务列表变了：叫醒共用名额的每一家（自己在 `pump` 里接上排队的、发列表）
fn changed(app: &AppHandle) {
    gen_slots::freed(app);
}

/// 空出来的名额接上排队的，再把整份列表发给制作窗口和面板。启动时登记给 `gen_slots::on_free`
pub fn pump(app: &AppHandle) {
    loop {
        // 取出要跑的就放锁，再起异步任务
        let next = {
            let mut q = q();
            let id = q.next(&mut gen_slots::slots());
            id.and_then(|id| q.req(id).map(|r| (id, r)))
        };
        let Some((id, req)) = next else { break };
        log::info!("表情包 任务 {id} 开始画");
        tauri::async_runtime::spawn(run(app.clone(), id, req));
    }
    let _emit = EMIT.lock().unwrap_or_else(PoisonError::into_inner);
    let views = views();
    for label in [crate::meme::LABEL, "panel"] {
        crate::events::emit_to(app, label, crate::events::MEME_TASKS, views.clone());
    }
}

async fn run(app: AppHandle, id: u64, req: GenReq) {
    let state = app.state::<AppState>();
    let cfg = meme_gen::image_cfg(&state.settings());
    let added = |rid| crate::events::emit_summary(&app, crate::events::RECORD_ADDED, rid);
    // 任务的取消记在队列里（`Task.cancelled`）
    let env = meme_gen::Env {
        state: &state,
        cutout: xpaste_platform::foreground_cutout,
        added: &added,
    };
    let r = if req.kind == SET_PARTS {
        // 4 部分拼成的 16 张共用一个套编号
        let set = Some(meme_gen::uuid());
        let part = |k, anchor| {
            let r = GenReq {
                kind: "set_part".into(),
                part: Some(k),
                set: set.clone(),
                anchor,
                ..req.clone()
            };
            meme_gen::generate(&env, &cfg, r)
        };
        run_parts(part, || q().cancelled(id)).await
    } else {
        meme_gen::generate(&env, &cfg, req).await
    };
    log::info!(
        "表情包 任务 {id} {}",
        if r.is_ok() {
            "画好了"
        } else {
            "没画出来"
        }
    );
    let note = q().finish(&mut gen_slots::slots(), id, r);
    if let Some(body) = note
        && !crate::meme::window_open()
    {
        crate::notify(&app, &body);
    }
    changed(&app);
}

pub fn reserve() -> u64 {
    q().reserve()
}

pub fn start(
    app: &AppHandle,
    id: u64,
    req: GenReq,
    title: String,
    scene: String,
) -> Result<(), String> {
    q().push(&mut gen_slots::slots(), id, req, title, scene)?;
    log::info!("表情包 任务 {id} 入队");
    changed(app);
    Ok(())
}

pub fn cancel(app: &AppHandle, id: u64) {
    q().cancel(&mut gen_slots::slots(), id);
    log::info!("表情包 任务 {id} 取消");
    changed(app);
}

/// 重试；任务已经不在（去掉了）或不是没画出来的时返回 `TASK_GONE`，页面据此不再等它
pub fn retry(app: &AppHandle, id: u64) -> Result<(), String> {
    if !q().retry(&mut gen_slots::slots(), id) {
        return Err(meme_gen::TASK_GONE.into());
    }
    log::info!("表情包 任务 {id} 重试");
    changed(app);
    Ok(())
}

pub fn dismiss(app: &AppHandle, id: u64) {
    Queue::drop(&mut q(), id);
    changed(app);
}

pub fn views() -> Vec<TaskView> {
    q().views(&gen_slots::slots())
}

/// 按任务里的做法打开制作窗口，结果摆进预览；画好又画规整的随之拿掉（试用结束等没真打开时不拿）
pub fn open(app: &AppHandle, id: u64) -> Result<(), AppError> {
    let Some((task, ids, take)) = q().reopen(id) else {
        return Err(AppError::Invalid("这个任务已经不在了".into()));
    };
    if crate::meme::open_task(app, &task, ids)? && take {
        Queue::drop(&mut q(), id);
        changed(app);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req(kind: &str) -> GenReq {
        serde_json::from_value(json!({
            "kind": kind, "part": null, "act": null, "style": "clay", "custom": null,
            "idea": "不想上班的念头", "caption": "不想上班的字", "refs": [], "sketch": null, "base": null,
        }))
        .unwrap()
    }

    fn push(q: &mut Queue, s: &mut Slots) -> u64 {
        let id = q.reserve();
        q.push(s, id, req("one"), "标题".into(), "{}".into())
            .unwrap();
        id
    }

    /// 入队只认领来的号，一个号只能用一次
    #[test]
    fn push_needs_a_reserved_id() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let push = |q: &mut Queue, s: &mut Slots, id| {
            q.push(s, id, req("one"), "标题".into(), "{}".into())
        };
        assert!(push(&mut q, &mut s, 1).is_err());
        let id = q.reserve();
        assert!(push(&mut q, &mut s, id).is_ok());
        assert!(push(&mut q, &mut s, id).is_err());
        assert!(!q.retry(&mut s, 99));
    }

    fn state(q: &Queue, s: &Slots, id: u64) -> Option<TaskState> {
        q.views(s).into_iter().find(|v| v.id == id).map(|v| v.state)
    }

    #[test]
    fn two_run_rest_wait() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let (a, b, c) = (
            push(&mut q, &mut s),
            push(&mut q, &mut s),
            push(&mut q, &mut s),
        );
        assert_eq!(q.next(&mut s), Some(a));
        assert_eq!(q.next(&mut s), Some(b));
        assert_eq!(q.next(&mut s), None);
        assert!(
            q.finish(&mut s, a, Ok(GenOut::Done { ids: vec![7] }))
                .is_some()
        );
        assert_eq!(q.next(&mut s), Some(c));
        assert_eq!(state(&q, &s, c), Some(TaskState::Run));
        assert!(
            q.views(&s)
                .iter()
                .find(|v| v.id == c)
                .unwrap()
                .started_ms
                .is_some()
        );
    }

    /// 排着的号在列表里没有了：不占名额，接着跑后面的
    #[test]
    fn stale_id_frees_its_slot() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        s.enqueue(Who::Meme, 99);
        let (a, b) = (push(&mut q, &mut s), push(&mut q, &mut s));
        assert_eq!(q.next(&mut s), Some(a));
        assert_eq!(q.next(&mut s), Some(b));
        assert_eq!(q.next(&mut s), None);
    }

    #[test]
    fn ahead_counts_waiting_before() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        push(&mut q, &mut s);
        push(&mut q, &mut s);
        let c = push(&mut q, &mut s);
        q.next(&mut s);
        q.next(&mut s);
        let ahead =
            |q: &Queue, s: &Slots, id| q.views(s).into_iter().find(|v| v.id == id).unwrap().ahead;
        assert_eq!(ahead(&q, &s, c), 0);
        let d = push(&mut q, &mut s);
        assert_eq!(ahead(&q, &s, d), 1);
        assert_eq!(ahead(&q, &s, c), 0);
        // 新的在前
        assert_eq!(q.views(&s)[0].id, d);
    }

    #[test]
    fn cancel_waiting_and_running() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let (a, b, c) = (
            push(&mut q, &mut s),
            push(&mut q, &mut s),
            push(&mut q, &mut s),
        );
        q.next(&mut s);
        q.next(&mut s);
        q.cancel(&mut s, c);
        assert_eq!(state(&q, &s, c), None);
        q.cancel(&mut s, a);
        assert_eq!(state(&q, &s, a), None);
        // 在画的取消了还占名额，回来之前不接新的
        let d = push(&mut q, &mut s);
        assert_eq!(q.next(&mut s), None);
        // 结果回来：不 panic，不发通知（结果已由 generate 进库）
        assert_eq!(q.finish(&mut s, a, Ok(GenOut::Done { ids: vec![9] })), None);
        assert_eq!(state(&q, &s, a), None);
        assert_eq!(q.next(&mut s), Some(d));
        assert_eq!(state(&q, &s, b), Some(TaskState::Run));
    }

    /// 和改字共用名额和队列：别家在跑、在排的都算，取消排队的也从共用队列里去掉
    #[test]
    fn slots_shared_with_other() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        s.enqueue(Who::Edit, 1);
        let a = push(&mut q, &mut s);
        s.enqueue(Who::Edit, 2);
        let b = push(&mut q, &mut s);
        let ahead =
            |q: &Queue, s: &Slots, id| q.views(s).into_iter().find(|v| v.id == id).unwrap().ahead;
        assert_eq!((ahead(&q, &s, a), ahead(&q, &s, b)), (1, 3));
        assert_eq!(q.next(&mut s), None);
        assert_eq!(s.take(Who::Edit), Some(1));
        assert_eq!(q.next(&mut s), Some(a));
        assert_eq!((ahead(&q, &s, a), ahead(&q, &s, b)), (0, 1));
        assert!(q.finish(&mut s, a, Err("超时".into())).is_some());
        // 排头是改字的
        assert_eq!(q.next(&mut s), None);
        q.retry(&mut s, a);
        assert_eq!(ahead(&q, &s, a), 2);
        q.cancel(&mut s, b);
        assert_eq!(ahead(&q, &s, a), 1);
        assert_eq!(s.take(Who::Edit), Some(2));
        assert_eq!(q.next(&mut s), None);
        s.release();
        assert_eq!(q.next(&mut s), Some(a));
    }

    #[test]
    fn unreadable_ref_note_hides_internal_reason() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let a = push(&mut q, &mut s);
        q.next(&mut s);
        let note = q.finish(&mut s, a, Err("unreadable:5:0".into())).unwrap();
        assert!(note.ends_with("没画出来：这张参考图读不出来"));
        assert!(!note.contains("unreadable"));
    }

    #[test]
    fn fail_then_retry_goes_to_back() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let (a, b) = (push(&mut q, &mut s), push(&mut q, &mut s));
        q.next(&mut s);
        let note = q.finish(&mut s, a, Err("服务商限流".into())).unwrap();
        assert!(note.contains("没画出来：服务商限流"));
        let v = q.views(&s).into_iter().find(|v| v.id == a).unwrap();
        assert_eq!((v.state, v.error.as_str()), (TaskState::Fail, "服务商限流"));
        q.retry(&mut s, a);
        let v = q.views(&s).into_iter().find(|v| v.id == a).unwrap();
        assert_eq!(
            (v.state, v.error.as_str(), v.ahead),
            (TaskState::Queue, "", 1)
        );
        assert_eq!(q.views(&s)[0].id, a);
        assert_eq!(q.next(&mut s), Some(b));
        assert_eq!(q.next(&mut s), Some(a));
    }

    #[test]
    fn drop_done() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let a = push(&mut q, &mut s);
        q.next(&mut s);
        q.finish(&mut s, a, Ok(GenOut::Irregular { id: 5 }));
        let v = q.views(&s).into_iter().find(|v| v.id == a).unwrap();
        assert_eq!(
            (v.state, v.ids.len(), v.irregular),
            (TaskState::Done, 0, Some(5))
        );
        // 没画规整的回去看过不收下：三选一里选过才走
        let (t, ids, take) = q.reopen(a).unwrap();
        assert_eq!((t.scene.as_str(), ids, take), ("{}", vec![5], false));
        q.drop(a);
        assert!(q.views(&s).is_empty());
    }

    #[test]
    fn reopen_takes_only_regular_done() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let (a, b) = (push(&mut q, &mut s), push(&mut q, &mut s));
        q.next(&mut s);
        q.next(&mut s);
        q.finish(&mut s, a, Ok(GenOut::Done { ids: vec![3, 4] }));
        q.finish(&mut s, b, Err("超时".into()));
        let (_, ids, take) = q.reopen(a).unwrap();
        assert_eq!((ids, take), (vec![3, 4], true));
        let (_, ids, take) = q.reopen(b).unwrap();
        assert_eq!((ids, take), (vec![], false));
    }

    #[test]
    fn title_scene_kept_not_logged() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let a = q.reserve();
        q.push(
            &mut s,
            a,
            req("set_part"),
            "不想上班".into(),
            r#"{"secret":"做法"}"#.into(),
        )
        .unwrap();
        let b = q.reserve();
        q.push(
            &mut s,
            b,
            GenReq {
                style: String::new(),
                ..req("act")
            },
            "x".into(),
            "{}".into(),
        )
        .unwrap();
        let v = &q.views(&s)[1];
        assert_eq!((v.id, v.kind, v.style.as_str()), (a, "set", "clay"));
        assert_eq!(v.title, "不想上班");
        assert_eq!(v.scene, r#"{"secret":"做法"}"#);
        assert_eq!(
            (q.views(&s)[0].kind, q.views(&s)[0].style.as_str()),
            ("act", "自定义")
        );
        let logged = format!("{q:?} {v:?}");
        for s in ["不想上班", "做法", "secret"] {
            assert!(!logged.contains(s), "{logged}");
        }
    }

    #[test]
    fn unknown_kind_not_queued() {
        let (mut q, mut s) = (Queue::new(), Slots::new());
        let a = q.reserve();
        assert!(
            q.push(&mut s, a, req("blah"), "x".into(), "{}".into())
                .is_err()
        );
        assert!(q.views(&s).is_empty());
        q.push(&mut s, a, req(SET_PARTS), "x".into(), "{}".into())
            .unwrap();
        assert_eq!(q.views(&s)[0].kind, "set");
        // 分 4 次重画只占一个名额
        q.next(&mut s);
        assert!(!q.cancelled(a));
        q.cancel(&mut s, a);
        assert!(q.cancelled(a));
    }

    /// 跑一部分：第 `fail_at` 次失败、第 `shaky_at` 次没画规整，其余给 4 个 id；记下调了几次
    fn parts(
        calls: &std::cell::Cell<u8>,
        fail_at: Option<u8>,
        shaky_at: Option<u8>,
    ) -> impl FnMut(u8, Option<i64>) -> std::future::Ready<Result<GenOut, String>> + '_ {
        move |k, anchor| {
            // 第一次没有可参考的，后面几次都带第一格
            assert_eq!(anchor, (k > 0).then_some(0));
            calls.set(calls.get() + 1);
            std::future::ready(if Some(k) == fail_at {
                Err("服务商限流".into())
            } else if Some(k) == shaky_at {
                Ok(GenOut::Irregular { id: 99 })
            } else {
                let b = i64::from(k) * 4;
                Ok(GenOut::Done {
                    ids: (b..b + 4).collect(),
                })
            })
        }
    }

    #[test]
    fn set_parts_joins_16() {
        let calls = std::cell::Cell::new(0);
        let out = tauri::async_runtime::block_on(run_parts(parts(&calls, None, None), || false));
        assert_eq!(
            out,
            Ok(GenOut::Done {
                ids: (0..16).collect()
            })
        );
        assert_eq!(calls.get(), 4);

        calls.set(0);
        let out = tauri::async_runtime::block_on(run_parts(parts(&calls, Some(2), None), || false));
        assert_eq!(out, Err("服务商限流".into()));
        assert_eq!(calls.get(), 3);

        calls.set(0);
        let out = tauri::async_runtime::block_on(run_parts(parts(&calls, None, Some(1)), || false));
        assert_eq!(out, Err(PART_SHAKY.into()));
        assert_eq!(calls.get(), 2);

        // 画完第 1 部分后取消：不再发后面的
        calls.set(0);
        let out = tauri::async_runtime::block_on(run_parts(parts(&calls, None, None), || {
            calls.get() >= 1
        }));
        assert!(out.is_err());
        assert_eq!(calls.get(), 1);
    }
}
