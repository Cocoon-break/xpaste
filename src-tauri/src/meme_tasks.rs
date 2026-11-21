//! 表情包制作的 AI 任务队列：每次生成是一个任务，不跟窗口走；同时最多 2 个在画，其余排队。
//! 状态一变就把整份任务列表发给制作窗口和面板。只在内存里，不存盘。
//!
//! `Queue` 是纯数据结构（不联网、不碰窗口），运行时是下面薄薄一层：全局锁、`pump`、异步任务。

use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::error::AppError;
use crate::meme_gen::{self, GenOut, GenReq};
use crate::state::AppState;

/// 同时在画的最多几个
pub const MAX_RUNNING: usize = 2;

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
    /// 排在它前面、还在排队的个数；不在排队的为 0
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
    /// 在画时被取消：不再显示，等结果回来（照样进库）再拿掉；仍占一个在画的名额
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
        f.debug_list().entries(self.views()).finish()
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
    F: FnMut(u8) -> Fut,
    Fut: std::future::Future<Output = Result<GenOut, String>>,
{
    let mut ids = Vec::new();
    for k in 0..4 {
        if cancelled() {
            return Err(meme_gen::LATE.into());
        }
        match part(k).await? {
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
    pub fn push(&mut self, req: GenReq, title: String, scene: String) -> Result<u64, String> {
        let kind = kind_of(&req).ok_or("不认识的生成方式")?;
        self.last_id += 1;
        let style = if req.style.is_empty() {
            "自定义".to_owned()
        } else {
            req.style.clone()
        };
        self.tasks.push(Task {
            view: TaskView {
                id: self.last_id,
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
        Ok(self.last_id)
    }

    /// 在画的不到 `MAX_RUNNING` 个时，把最早排队的标成在画并返回它（取消了还没回来的也占名额）
    #[allow(clippy::should_implement_trait)] // 不是迭代器：取出的任务留在队列里
    pub fn next(&mut self) -> Option<u64> {
        let running = self
            .tasks
            .iter()
            .filter(|t| t.view.state == TaskState::Run)
            .count();
        if running >= MAX_RUNNING {
            return None;
        }
        let t = self
            .tasks
            .iter_mut()
            .find(|t| t.view.state == TaskState::Queue)?;
        t.view.state = TaskState::Run;
        t.view.started_ms = Some(u64::try_from(crate::clock::now_ms()).unwrap_or(0));
        Some(t.view.id)
    }

    /// 在画时被取消了（或已经不在了）：分 4 次重画据此不再发后面的部分
    pub fn cancelled(&self, id: u64) -> bool {
        self.at(id).is_none_or(|i| self.tasks[i].cancelled)
    }

    /// 这个任务的生成请求（交给 `meme_gen::generate`）
    pub fn req(&self, id: u64) -> Option<GenReq> {
        self.at(id).map(|i| self.tasks[i].req.clone())
    }

    /// 在画的结束了（结果已由 `generate` 存进库）。返回该发的通知正文；已取消、不在了的返回 None
    pub fn finish(&mut self, id: u64, r: Result<GenOut, String>) -> Option<String> {
        let i = self.at(id)?;
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
                format!("“{}”没画出来：{}", v.title, v.error)
            }
        })
    }

    /// 取消：排队的直接拿掉；在画的不再显示，请求不中止，结果回来照样进库
    pub fn cancel(&mut self, id: u64) {
        let Some(i) = self.at(id) else { return };
        match self.tasks[i].view.state {
            TaskState::Queue => {
                self.tasks.remove(i);
            }
            TaskState::Run => self.tasks[i].cancelled = true,
            TaskState::Done | TaskState::Fail => {}
        }
    }

    /// 没画出来的重试：回到队尾排队
    pub fn retry(&mut self, id: u64) {
        let Some(i) = self.at(id) else { return };
        if self.tasks[i].view.state != TaskState::Fail {
            return;
        }
        let mut t = self.tasks.remove(i);
        t.view.state = TaskState::Queue;
        t.view.error.clear();
        t.view.started_ms = None;
        self.tasks.push(t);
    }

    /// 去掉画好的、没画出来的（在画、排队的用 `cancel`）
    pub fn drop(&mut self, id: u64) {
        if let Some(i) = self.at(id)
            && matches!(self.tasks[i].view.state, TaskState::Done | TaskState::Fail)
        {
            self.tasks.remove(i);
        }
    }

    /// 回到那次：做法、要摆进预览的结果（网格没画规整的是那一张）、是不是画好了
    pub fn reopen(&self, id: u64) -> Option<(String, Vec<i64>, bool)> {
        let v = &self.tasks[self.at(id)?].view;
        let ids = v.irregular.map_or_else(|| v.ids.clone(), |x| vec![x]);
        Some((v.scene.clone(), ids, v.state == TaskState::Done))
    }

    /// 整份列表，新的在前；取消了还没回来的不在里面
    pub fn views(&self) -> Vec<TaskView> {
        let mut ahead = 0;
        let mut out: Vec<TaskView> = Vec::new();
        for t in self.tasks.iter().filter(|t| !t.cancelled) {
            let mut v = t.view.clone();
            if v.state == TaskState::Queue {
                v.ahead = ahead;
                ahead += 1;
            }
            out.push(v);
        }
        out.reverse();
        out
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

/// 空出来的名额接上排队的，再把整份列表发给制作窗口和面板
fn changed(app: &AppHandle) {
    loop {
        // 取出要跑的就放锁，再起异步任务
        let next = {
            let mut q = q();
            q.next().and_then(|id| q.req(id).map(|r| (id, r)))
        };
        let Some((id, req)) = next else { break };
        log::info!("表情包 任务 {id} 开始画");
        tauri::async_runtime::spawn(run(app.clone(), id, req));
    }
    let _emit = EMIT.lock().unwrap_or_else(PoisonError::into_inner);
    let views = q().views();
    for label in [crate::meme::LABEL, "panel"] {
        crate::events::emit_to(app, label, crate::events::MEME_TASKS, views.clone());
    }
}

async fn run(app: AppHandle, id: u64, req: GenReq) {
    let state = app.state::<AppState>();
    let cfg = meme_gen::image_cfg(&state.settings());
    let added = |rid| crate::events::emit_summary(&app, crate::events::RECORD_ADDED, rid);
    // 任务的取消记在队列里，不用这个序号：给 generate 一个只属于这次的，它就永远不算晚到
    let seq = AtomicU64::new(0);
    let env = meme_gen::Env {
        state: &state,
        seq: &seq,
        cutout: xpaste_platform::foreground_cutout,
        added: &added,
    };
    let r = if req.kind == SET_PARTS {
        // 4 部分拼成的 16 张共用一个套编号
        let set = Some(meme_gen::uuid());
        let part = |k| {
            let r = GenReq {
                kind: "set_part".into(),
                part: Some(k),
                set: set.clone(),
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
    let note = q().finish(id, r);
    if let Some(body) = note
        && !crate::meme::window_open()
    {
        crate::notify(&app, &body);
    }
    changed(&app);
}

pub fn start(app: &AppHandle, req: GenReq, title: String, scene: String) -> Result<u64, String> {
    let id = q().push(req, title, scene)?;
    log::info!("表情包 任务 {id} 入队");
    changed(app);
    Ok(id)
}

pub fn cancel(app: &AppHandle, id: u64) {
    q().cancel(id);
    log::info!("表情包 任务 {id} 取消");
    changed(app);
}

pub fn retry(app: &AppHandle, id: u64) {
    q().retry(id);
    log::info!("表情包 任务 {id} 重试");
    changed(app);
}

pub fn dismiss(app: &AppHandle, id: u64) {
    Queue::drop(&mut q(), id);
    changed(app);
}

pub fn views() -> Vec<TaskView> {
    q().views()
}

/// 按任务里的做法打开制作窗口，结果摆进预览；画好的随之拿掉（试用结束等没真打开时不拿）
pub fn open(app: &AppHandle, id: u64) -> Result<(), AppError> {
    let Some((scene, ids, done)) = q().reopen(id) else {
        return Err(AppError::Invalid("这个任务已经不在了".into()));
    };
    if crate::meme::open_task(app, id, scene, ids)? && done {
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

    fn push(q: &mut Queue) -> u64 {
        q.push(req("one"), "标题".into(), "{}".into()).unwrap()
    }

    fn state(q: &Queue, id: u64) -> Option<TaskState> {
        q.views().into_iter().find(|v| v.id == id).map(|v| v.state)
    }

    #[test]
    fn two_run_rest_wait() {
        let mut q = Queue::new();
        let (a, b, c) = (push(&mut q), push(&mut q), push(&mut q));
        assert_eq!(q.next(), Some(a));
        assert_eq!(q.next(), Some(b));
        assert_eq!(q.next(), None);
        assert!(q.finish(a, Ok(GenOut::Done { ids: vec![7] })).is_some());
        assert_eq!(q.next(), Some(c));
        assert_eq!(state(&q, c), Some(TaskState::Run));
        assert!(
            q.views()
                .iter()
                .find(|v| v.id == c)
                .unwrap()
                .started_ms
                .is_some()
        );
    }

    #[test]
    fn ahead_counts_waiting_before() {
        let mut q = Queue::new();
        push(&mut q);
        push(&mut q);
        let c = push(&mut q);
        q.next();
        q.next();
        let ahead = |q: &Queue, id| q.views().into_iter().find(|v| v.id == id).unwrap().ahead;
        assert_eq!(ahead(&q, c), 0);
        let d = push(&mut q);
        assert_eq!(ahead(&q, d), 1);
        assert_eq!(ahead(&q, c), 0);
        // 新的在前
        assert_eq!(q.views()[0].id, d);
    }

    #[test]
    fn cancel_waiting_and_running() {
        let mut q = Queue::new();
        let (a, b, c) = (push(&mut q), push(&mut q), push(&mut q));
        q.next();
        q.next();
        q.cancel(c);
        assert_eq!(state(&q, c), None);
        q.cancel(a);
        assert_eq!(state(&q, a), None);
        // 在画的取消了还占名额，回来之前不接新的
        let d = push(&mut q);
        assert_eq!(q.next(), None);
        // 结果回来：不 panic，不发通知（结果已由 generate 进库）
        assert_eq!(q.finish(a, Ok(GenOut::Done { ids: vec![9] })), None);
        assert_eq!(state(&q, a), None);
        assert_eq!(q.next(), Some(d));
        assert_eq!(state(&q, b), Some(TaskState::Run));
    }

    #[test]
    fn fail_then_retry_goes_to_back() {
        let mut q = Queue::new();
        let (a, b) = (push(&mut q), push(&mut q));
        q.next();
        let note = q.finish(a, Err("服务商限流".into())).unwrap();
        assert!(note.contains("没画出来：服务商限流"));
        let v = q.views().into_iter().find(|v| v.id == a).unwrap();
        assert_eq!((v.state, v.error.as_str()), (TaskState::Fail, "服务商限流"));
        q.retry(a);
        let v = q.views().into_iter().find(|v| v.id == a).unwrap();
        assert_eq!(
            (v.state, v.error.as_str(), v.ahead),
            (TaskState::Queue, "", 1)
        );
        assert_eq!(q.views()[0].id, a);
        assert_eq!(q.next(), Some(b));
        assert_eq!(q.next(), Some(a));
    }

    #[test]
    fn drop_done() {
        let mut q = Queue::new();
        let a = push(&mut q);
        q.next();
        q.finish(a, Ok(GenOut::Irregular { id: 5 }));
        let v = q.views().into_iter().find(|v| v.id == a).unwrap();
        assert_eq!(
            (v.state, v.ids.len(), v.irregular),
            (TaskState::Done, 0, Some(5))
        );
        assert_eq!(q.reopen(a), Some(("{}".into(), vec![5], true)));
        q.drop(a);
        assert!(q.views().is_empty());
    }

    #[test]
    fn title_scene_kept_not_logged() {
        let mut q = Queue::new();
        let a = q
            .push(
                req("set_part"),
                "不想上班".into(),
                r#"{"secret":"做法"}"#.into(),
            )
            .unwrap();
        q.push(
            GenReq {
                style: String::new(),
                ..req("act")
            },
            "x".into(),
            "{}".into(),
        )
        .unwrap();
        let v = &q.views()[1];
        assert_eq!((v.id, v.kind, v.style.as_str()), (a, "set", "clay"));
        assert_eq!(v.title, "不想上班");
        assert_eq!(v.scene, r#"{"secret":"做法"}"#);
        assert_eq!(
            (q.views()[0].kind, q.views()[0].style.as_str()),
            ("act", "自定义")
        );
        let logged = format!("{q:?} {v:?}");
        for s in ["不想上班", "做法", "secret"] {
            assert!(!logged.contains(s), "{logged}");
        }
    }

    #[test]
    fn unknown_kind_not_queued() {
        let mut q = Queue::new();
        assert!(q.push(req("blah"), "x".into(), "{}".into()).is_err());
        assert!(q.views().is_empty());
        let a = q.push(req(SET_PARTS), "x".into(), "{}".into()).unwrap();
        assert_eq!(q.views()[0].kind, "set");
        // 分 4 次重画只占一个名额
        q.next();
        assert!(!q.cancelled(a));
        q.cancel(a);
        assert!(q.cancelled(a));
    }

    /// 跑一部分：第 `fail_at` 次失败、第 `shaky_at` 次没画规整，其余给 4 个 id；记下调了几次
    fn parts(
        calls: &std::cell::Cell<u8>,
        fail_at: Option<u8>,
        shaky_at: Option<u8>,
    ) -> impl FnMut(u8) -> std::future::Ready<Result<GenOut, String>> + '_ {
        move |k| {
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
