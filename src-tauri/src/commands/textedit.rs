//! 改字的命令：打开、关闭窗口，图和读出来的字，范围、条带、走法，改一处，任务，最近改过。
//! 解码、识别、裁剪、贴回、存盘都在阻塞线程上；会产出改动的先过 `editable`（开关、试用期）。

use serde::Deserialize;
use serde::Serialize;
use tauri::AppHandle;
use tauri::ipc::{InvokeBody, Request, Response};
use xpaste_pro::TeQuad;

use crate::error::AppError;
use crate::textedit::{self, Config, Recent, Source};
use crate::textedit_job::{self as te, Draft, Extend, JobView, LineRef, Plan, Ranges};
use crate::textedit_store as store;
use crate::textedit_tasks::{self as tasks, TaskView, tasks};

/// 到阻塞线程上做
async fn bg<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tauri::async_runtime::spawn_blocking(f).await?
}

/// 面板的“改字”：这条记录的图（多个文件时是第一个图片文件）。改不了的返回原因
#[tauri::command]
pub async fn te_open(app: AppHandle, record: i64) -> Result<(), AppError> {
    let src = Source::Panel { record, file: None };
    bg(move || textedit::open(&app, src)).await
}

/// 改字窗口里 `⌘V`、最近的图
#[tauri::command]
pub async fn te_open_record(
    app: AppHandle,
    record: i64,
    file: Option<u32>,
) -> Result<(), AppError> {
    let src = Source::Panel { record, file };
    bg(move || textedit::open_inside(&app, src, "blank")).await
}

/// 拖进改字窗口的文件
#[tauri::command]
pub async fn te_open_file(app: AppHandle, path: String) -> Result<(), AppError> {
    let src = Source::File(path.into());
    bg(move || textedit::open_inside(&app, src, "file")).await
}

/// 改字窗口里点一张“最近改过”
#[tauri::command]
pub async fn te_open_recent(app: AppHandle, id: String) -> Result<(), AppError> {
    let src = Source::Recent(id);
    bg(move || textedit::open_inside(&app, src, "recent")).await
}

/// 页面把事件都听上了：这之前的打开事件现在补发
#[tauri::command(async)]
pub fn te_ready(app: AppHandle) {
    textedit::ready(&app);
}

#[tauri::command(async)]
pub fn te_close(app: AppHandle) {
    textedit::close(&app);
}

#[tauri::command(async)]
pub fn te_back(app: AppHandle) {
    textedit::back(&app);
}

/// 窗口重新拿到焦点：粘贴目标的名字
#[tauri::command(async)]
pub fn te_target() -> Option<String> {
    textedit::refresh_target()
}

/// 设置的“怎么改”、生图接口配好没有
#[tauri::command(async)]
pub fn te_config(app: AppHandle) -> Config {
    textedit::config(&app)
}

/// 原图的 PNG
#[tauri::command]
pub async fn te_image(app: AppHandle, job: String) -> Result<Response, AppError> {
    bg(move || Ok(textedit::find(&app, &job)?.orig()?))
        .await
        .map(Response::new)
}

/// 画上选中候选的 PNG
#[tauri::command]
pub async fn te_render(app: AppHandle, job: String) -> Result<Response, AppError> {
    bg(move || Ok(textedit::find(&app, &job)?.render(&textedit::dir(&app))?))
        .await
        .map(Response::new)
}

/// 一张候选的 PNG
#[tauri::command]
pub async fn te_cand(app: AppHandle, job: String, file: String) -> Result<Response, AppError> {
    bg(move || Ok(textedit::find(&app, &job)?.cand_png(&textedit::dir(&app), &file)?))
        .await
        .map(Response::new)
}

/// 一张图此刻的内容
#[tauri::command]
pub async fn te_job(app: AppHandle, job: String) -> Result<JobView, AppError> {
    bg(move || Ok(textedit::find(&app, &job)?.view())).await
}

#[derive(Serialize)]
pub struct LineView {
    quad: TeQuad,
    text: String,
    chars: Vec<[f32; 2]>,
}

/// 读字的结果。不给改的图只有类别
#[derive(Serialize)]
pub struct LinesView {
    lines: Vec<LineView>,
    blocked: Option<&'static str>,
    locked: Vec<TeQuad>,
    engine: bool,
}

/// 读字的结果，没读完就等
#[tauri::command]
pub async fn te_lines(app: AppHandle, job: String) -> Result<LinesView, AppError> {
    bg(move || {
        let live = textedit::find(&app, &job)?;
        let r = live.read();
        let blocked = r.blocked.map(|b| b.label());
        let lines = r.lines.iter().filter(|_| blocked.is_none());
        Ok(LinesView {
            lines: lines
                .map(|l| LineView {
                    quad: l.quad,
                    text: l.text.clone(),
                    chars: l.chars.clone(),
                })
                .collect(),
            blocked,
            locked: r.locked.clone(),
            engine: r.engine,
        })
    })
    .await
}

/// 第 `from`..`to` 个字的改动范围，和它右边的空白到哪儿
#[tauri::command]
pub async fn te_ranges(
    app: AppHandle,
    job: String,
    line: LineRef,
    n: usize,
    chars: Vec<[f32; 2]>,
    from: usize,
    to: usize,
) -> Result<Ranges, AppError> {
    bg(move || {
        let live = textedit::find(&app, &job)?;
        te::ranges(&live, &line, n, chars, from, to)
    })
    .await
}

/// 原尺寸的条带：一行 JSON（`StripHead`）、换行、RGBA
#[tauri::command]
pub async fn te_strip(
    app: AppHandle,
    job: String,
    line: LineRef,
    extend: Option<Extend>,
) -> Result<Response, AppError> {
    bg(move || {
        let live = textedit::find(&app, &job)?;
        let (head, px) = te::strip(&live, &line, extend.unwrap_or_default())?;
        let mut out = serde_json::to_vec(&head).map_err(std::io::Error::from)?;
        out.push(b'\n');
        out.extend(px);
        Ok(Response::new(out))
    })
    .await
}

/// 这一处会怎么改
#[tauri::command]
pub async fn te_plan(app: AppHandle, job: String, edit: Draft) -> Result<Plan, AppError> {
    textedit::editable(&app)?;
    bg(move || {
        let c = textedit::config(&app);
        textedit::find(&app, &job)?.plan(&edit, c.mode == "ai", c.api)
    })
    .await
}

#[derive(Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Way {
    Local,
    Ai,
}

/// 提交了一处：任务号（本机改的没有）、那一处的 id、这张图此刻的内容
#[derive(Serialize)]
pub struct RunOut {
    task: Option<u64>,
    edit: u64,
    job: JobView,
}

/// 改一处。AI 写的入队、马上返回；本机的改好才返回。`mask` 只管 AI
#[tauri::command]
pub async fn te_run(
    app: AppHandle,
    job: String,
    edit: Draft,
    way: Way,
    mask: bool,
) -> Result<RunOut, AppError> {
    textedit::editable(&app)?;
    bg(move || {
        let (live, dir) = (textedit::find(&app, &job)?, textedit::dir(&app));
        let (task, id) = match way {
            Way::Local => (None, te::run_local(tasks(), &dir, &live, &edit)?),
            Way::Ai => {
                let api = textedit::config(&app).api;
                let (task, id) = tasks().submit(&dir, &live, &edit, mask, api)?;
                log::info!("改字 任务 {task} 入队");
                (Some(task), id)
            }
        };
        tasks::changed(&app);
        Ok(RunOut {
            task,
            edit: id,
            job: live.view(),
        })
    })
    .await
}

#[derive(Deserialize)]
struct PatchHead {
    job: String,
    edit: Draft,
    quad: TeQuad,
}

/// 页面重排好的一条：请求体是一行 JSON {job, edit, quad}、换行、RGBA（尺寸同 `te_strip` 给的那条，`quad` 也是它给的）
#[tauri::command]
pub async fn te_local_patch(app: AppHandle, request: Request<'_>) -> Result<RunOut, AppError> {
    textedit::editable(&app)?;
    let InvokeBody::Raw(body) = request.body() else {
        return Err(te::no("请求体应为条带数据"));
    };
    let body = body.clone();
    bg(move || {
        let bad = || te::no("条带数据不对");
        let at = body.iter().position(|&b| b == b'\n').ok_or_else(bad)?;
        let head: PatchHead = serde_json::from_slice(&body[..at]).map_err(|_| bad())?;
        let (live, dir) = (textedit::find(&app, &head.job)?, textedit::dir(&app));
        let px = &body[at + 1..];
        let id = te::local_patch(tasks(), &dir, &live, &head.edit, &head.quad, px)?;
        log::info!("改字 本机重排 改好一处");
        tasks::changed(&app);
        Ok(RunOut {
            task: None,
            edit: id,
            job: live.view(),
        })
    })
    .await
}

/// 选中第 `i` 个候选
#[tauri::command]
pub async fn te_pick(
    app: AppHandle,
    job: String,
    edit: u64,
    i: usize,
) -> Result<JobView, AppError> {
    textedit::editable(&app)?;
    bg(move || {
        let live = textedit::find(&app, &job)?;
        te::pick(tasks(), &textedit::dir(&app), &live, edit, i)?;
        tasks::changed(&app);
        Ok(live.view())
    })
    .await
}

/// 改回去
#[tauri::command]
pub async fn te_undo(app: AppHandle, job: String, edit: u64) -> Result<JobView, AppError> {
    textedit::editable(&app)?;
    bg(move || {
        let live = textedit::find(&app, &job)?;
        te::undo(tasks(), &textedit::dir(&app), &live, edit)?;
        tasks::changed(&app);
        Ok(live.view())
    })
    .await
}

/// 整份任务列表
#[tauri::command(async)]
pub fn te_tasks() -> Vec<TaskView> {
    tasks().views()
}

/// 取消：排队的直接拿掉；在改的请求不中止，结果回来存成候选、不选中
#[tauri::command(async)]
pub fn te_cancel(app: AppHandle, task: u64) {
    tasks().cancel(task);
    log::info!("改字 任务 {task} 取消");
    tasks::changed(&app);
}

/// 没改出来的重试，发 1 次请求；`mask` 为 false 是“不带遮罩再试”
#[tauri::command]
pub async fn te_retry(app: AppHandle, task: u64, mask: bool) -> Result<(), AppError> {
    textedit::editable(&app)?;
    bg(move || {
        let api = textedit::config(&app).api;
        tasks().retry(&textedit::dir(&app), task, mask, api, |id| {
            textedit::find(&app, id)
        })?;
        log::info!(
            "改字 任务 {task} 重试{}",
            if mask { "" } else { "（不带遮罩）" }
        );
        tasks::changed(&app);
        Ok(())
    })
    .await
}

/// 去掉改好的、没改出来的
#[tauri::command(async)]
pub fn te_drop(app: AppHandle, task: u64) {
    tasks().drop(task);
    tasks::changed(&app);
}

/// “最近改过”，最近 12 张
#[tauri::command]
pub async fn te_recent(app: AppHandle) -> Result<Vec<Recent>, AppError> {
    bg(move || Ok(textedit::recent(&textedit::dir(&app)))).await
}

/// “最近改过”一张的缩略图
#[tauri::command]
pub async fn te_thumb(app: AppHandle, id: String) -> Result<Response, AppError> {
    bg(move || textedit::thumb(&textedit::dir(&app), &id))
        .await
        .map(Response::new)
}

/// 删一张，5 秒内可撤销
#[tauri::command]
pub async fn te_recent_remove(app: AppHandle, id: String) -> Result<(), AppError> {
    bg(move || {
        textedit::remove(tasks(), &textedit::dir(&app), &id)?;
        tasks::changed(&app);
        Ok(())
    })
    .await
}

/// 撤销删除
#[tauri::command]
pub async fn te_recent_restore(app: AppHandle, id: String) -> Result<(), AppError> {
    bg(move || {
        textedit::restore(tasks(), &textedit::dir(&app), &id)?;
        tasks::changed(&app);
        Ok(())
    })
    .await
}

/// “最近改过”有几张、占多少字节
#[tauri::command]
pub async fn te_recent_stats(app: AppHandle) -> Result<(u64, u64), AppError> {
    bg(move || Ok(store::stats(&textedit::dir(&app)))).await
}

/// 清空“最近改过”
#[tauri::command]
pub async fn te_recent_clear(app: AppHandle) -> Result<(), AppError> {
    bg(move || {
        textedit::clear(tasks(), &textedit::dir(&app))?;
        log::info!("改字 清空最近改过");
        tasks::changed(&app);
        Ok(())
    })
    .await
}
