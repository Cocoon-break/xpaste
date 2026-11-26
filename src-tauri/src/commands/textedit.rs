//! 改字的命令：打开、关闭窗口，图和读出来的字，范围、条带、走法，改一处，任务，最近改过。
//! 解码、识别、裁剪、贴回、存盘都在阻塞线程上；会产出改动的先过 `editable`（开关、试用期）。

use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use tauri::ipc::{InvokeBody, Request, Response};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use xpaste_pro::TeQuad;

use crate::error::AppError;
use crate::textedit::{self, Config, Recent, Source};
use crate::textedit_finish::{self as fin, Finish, FinishOut};
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

/// 贴图的“改字…”：调用方贴图的原图
#[tauri::command]
pub async fn te_open_pin(app: AppHandle, window: WebviewWindow) -> Result<(), AppError> {
    let n = crate::shot::pin_n(&window)?;
    bg(move || textedit::open(&app, Source::Pin { n })).await
}

/// 改字编在这个构建里、设置里也开着
#[tauri::command(async)]
pub fn te_on(app: AppHandle) -> bool {
    textedit::on(&app)
}

/// 拖进改字窗口的文件；多个的只用第一个图片，打开后提示“一次改一张”
#[tauri::command]
pub async fn te_open_files(app: AppHandle, paths: Vec<PathBuf>) -> Result<(), AppError> {
    bg(move || textedit::open_dropped(&app, &paths)).await
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

/// 读字的结果。不给改的图只有类别；`failed` 的图读不出来，不许改
#[derive(Serialize)]
pub struct LinesView {
    lines: Vec<LineView>,
    blocked: Option<&'static str>,
    locked: Vec<TeQuad>,
    engine: bool,
    failed: bool,
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
            failed: r.failed,
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

/// 完成，只用已改好的
#[tauri::command]
pub async fn te_finish(app: AppHandle, job: String, act: Finish) -> Result<FinishOut, AppError> {
    bg(move || {
        let act = match act {
            Finish::SaveAs(_) => match ask_path(&app, &job)? {
                Some(to) => Finish::SaveAs(to),
                None => return Ok(FinishOut::NONE),
            },
            act => act,
        };
        fin::finish(&app, &job, act)
    })
    .await
}

/// 系统的保存文件对话框；取消为 None。等用户选完
fn ask_path(app: &AppHandle, job: &str) -> Result<Option<PathBuf>, AppError> {
    let p = fin::suggest(app, job)?;
    let text = |s: Option<&std::ffi::OsStr>| s.unwrap_or_default().to_string_lossy().into_owned();
    let mut d = app
        .dialog()
        .file()
        .set_file_name(text(p.file_name()))
        .add_filter("图片", &[&text(p.extension())]);
    if let Some(dir) = p.parent() {
        d = d.set_directory(dir);
    }
    if let Some(w) = app.get_webview_window(textedit::LABEL) {
        d = d.set_parent(&w);
    }
    Ok(d.blocking_save_file().and_then(|f| f.into_path().ok()))
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

#[cfg(test)]
mod tests {
    /// 要过门的：会产出改动
    const GATED: [&str; 6] = [
        "te_plan",
        "te_run",
        "te_local_patch",
        "te_pick",
        "te_undo",
        "te_retry",
    ];
    /// 放行的：打开窗口、看图、完成、任务列表、最近改过
    const FREE: [&str; 28] = [
        "te_open",
        "te_open_record",
        "te_open_pin",
        "te_on",
        "te_open_files",
        "te_open_recent",
        "te_ready",
        "te_close",
        "te_back",
        "te_target",
        "te_config",
        "te_image",
        "te_render",
        "te_cand",
        "te_job",
        "te_lines",
        "te_ranges",
        "te_strip",
        "te_tasks",
        "te_cancel",
        "te_drop",
        "te_finish",
        "te_recent",
        "te_thumb",
        "te_recent_remove",
        "te_recent_restore",
        "te_recent_stats",
        "te_recent_clear",
    ];

    /// 顶格定义的函数的源码，一行一行，去掉缩进，到它的右花括号
    fn body<'a>(src: &'a str, name: &str) -> Vec<&'a str> {
        let at = format!("fn {name}(");
        src.lines()
            .skip_while(|l| l.starts_with(' ') || !l.contains(&at))
            .take_while(|l| !l.starts_with('}'))
            .map(str::trim)
            .collect()
    }

    /// 试用结束：会产出改动的命令都过 `editable`（它怎么判断见 textedit.rs 的测试），
    /// 其余的一律不过，“最近改过”的照样能看、复制、存成文件。新加的命令要归到一张表里
    #[test]
    fn expired_can_export_not_edit() {
        const GATE: &str = "textedit::editable(&app)?;";
        let src = include_str!("textedit.rs");
        let all: Vec<&str> = src
            .lines()
            .filter(|l| l.starts_with("pub "))
            .filter_map(|l| l.split_once("fn te_")?.1.split_once('('))
            .map(|(name, _)| name)
            .collect();
        assert_eq!(all.len(), GATED.len() + FREE.len());
        for name in all {
            let name = &*format!("te_{name}");
            let gated = body(src, name).contains(&GATE);
            assert_eq!(gated, GATED.contains(&name), "{name}");
            assert_ne!(gated, FREE.contains(&name), "{name}");
        }
        let finish = body(include_str!("../textedit_finish.rs"), "finish");
        assert!(finish.contains(&"textedit::check_on(app)?;"));
        assert!(!finish.iter().any(|l| l.contains("editable")));
        // 打开：只有“最近改过”的给图，别的来源不读图
        let open = body(include_str!("../textedit.rs"), "open_as");
        let at = |s: &str| open.iter().position(|l| l.starts_with(s)).expect(s);
        assert!(at("Source::Recent(id) => Some(find(app, id)?),") < at("_ if expired => None,"));
        assert!(at("_ if expired => None,") < at("let bytes = bytes_of("));
    }
}
