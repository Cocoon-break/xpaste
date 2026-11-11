//! 晾衣绳便签的命令：记录、拖进来的文件转成便签，复制、打开，以及 Pro 状态与授权码。
//! 日志只记便签 id、记录 id、类型和字节数，不记正文、标题、备注和路径。

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::Serialize;
use tauri::ipc::{InvokeBody, Request};
use tauri::{AppHandle, Manager, State};
use xpaste_core::detect::{VIDEO_EXTS, is_image_file};
use xpaste_core::{Kind, LineSide, LineStyle, RecordDetail};
use xpaste_pro::{Line, LineError, NewNote, Note, Pro};

use crate::assets::asset_url;
use crate::clock::now_ms;
use crate::commands::records::{precheck, write_marked, write_text_plain, write_then_paste};
use crate::error::AppError;
use crate::events::{LINE_CHANGED, LINE_DROP, emit, emit_to};
use crate::state::AppState;

/// 便签正文的上限，记录正文超出时截断
const MAX_BODY: usize = 64 * 1024;

/// 便签与试用都按 Unix 秒计时
fn now() -> i64 {
    now_ms() / 1000
}

/// 便签加上图片的 asset URL（原图或视频封面在 `line/` 里）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDto {
    #[serde(flatten)]
    pub note: Note,
    pub image_url: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    /// 这个构建带不带晾衣绳
    pub available: bool,
    pub pro: Pro,
    pub notes: Vec<NoteDto>,
    /// 已挂着的记录 id，面板据此画刻线
    pub hung: Vec<i64>,
    /// 便签快捷键的显示标签（如 `⌥ + Shift + N`），没启用为空串；空绳子的提示用它
    pub hotkey: String,
    /// 生效的样式：试用结束后一律是默认样式
    pub style: LineStyle,
    /// 用户开关；与 `available`（构建带不带）是两回事
    pub on: bool,
    pub side: LineSide,
    /// 一直显示：页面不自己收起，报能点的地方
    pub pinned: bool,
}

#[derive(Debug, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum CopyNote {
    Copied,
    /// 原记录没了，复制的是文件路径文字
    CopiedPath,
}

pub enum CopyPlan {
    Text(String),
    Record(i64),
    Image(PathBuf),
    Paths(String),
}

/// 记录详情 → NewNote，样子跟面板里的同类记录一致；正文超过 64 KB 时截断
pub fn note_from_detail(d: &RecordDetail, source: Option<String>) -> NewNote {
    let s = &d.summary;
    let mut n = NewNote {
        kind: s.kind,
        body: s.title.clone(),
        title: None,
        memo: None,
        size: None,
        file_kind: None,
        duration_sec: None,
        paths: Vec::new(),
        record_id: Some(s.id),
        source,
        image_from: None,
    };
    let paths = || d.files.iter().map(|f| f.path.clone());
    match s.kind {
        Kind::Text | Kind::Code => n.body = clip(d.text.as_deref().unwrap_or_default()),
        Kind::Link => {
            n.body = clip(d.text.as_deref().or(s.url.as_deref()).unwrap_or_default());
            n.title = d.link_title.clone();
        }
        Kind::Image => {
            // 位图在 images/<id>.png；图片文件引用的原图就是那个文件
            let src = d.image.as_ref().map(|i| i.src.clone());
            n.image_from = src.or_else(|| paths().next()).map(PathBuf::from);
            n.size = Some(s.size);
        }
        Kind::Video => {
            n.image_from = s.thumb.as_ref().map(PathBuf::from);
            n.paths = paths().take(1).collect();
            n.duration_sec = s.duration_sec;
            n.size = Some(s.size);
        }
        Kind::File => {
            n.file_kind = d.files.first().map(|f| f.ext.to_uppercase());
            n.paths = paths().collect();
            n.size = Some(s.size);
        }
    }
    n
}

/// 不超过 64 KB 的前缀，截在字符边界
fn clip(t: &str) -> String {
    let mut end = MAX_BODY.min(t.len());
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].to_owned()
}

/// 拖进来的路径 → NewNote：图片扩展名为图片（原图就是这个文件），视频扩展名为视频，其余为文件
pub fn note_from_path(p: &Path, meta_len: u64) -> NewNote {
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned());
    let kind = if is_image_file(p) {
        Kind::Image
    } else if ext
        .as_deref()
        .is_some_and(|e| VIDEO_EXTS.iter().any(|x| e.eq_ignore_ascii_case(x)))
    {
        Kind::Video
    } else {
        Kind::File
    };
    NewNote {
        kind,
        body: p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        title: None,
        memo: None,
        size: Some(meta_len),
        file_kind: ext.filter(|_| kind == Kind::File).map(|e| e.to_uppercase()),
        duration_sec: None,
        paths: vec![p.to_string_lossy().into_owned()],
        record_id: None,
        source: None,
        image_from: (kind == Kind::Image).then(|| p.to_path_buf()),
    }
}

/// 复制方案：文字类复制便签正文；图片、文件、视频原记录还在就按记录复制，否则图片复制图片，文件和视频复制路径
pub fn copy_plan(note: &Note, record_alive: bool, image: Option<PathBuf>) -> CopyPlan {
    if matches!(note.kind, Kind::Text | Kind::Code | Kind::Link) {
        return CopyPlan::Text(note.body.clone());
    }
    if let (true, Some(id)) = (record_alive, note.record_id) {
        return CopyPlan::Record(id);
    }
    match image.filter(|_| note.kind == Kind::Image) {
        Some(p) => CopyPlan::Image(p),
        None => CopyPlan::Paths(note.paths.join("\n")),
    }
}

/// 自己写的空白文字便签
fn blank_note() -> NewNote {
    NewNote {
        kind: Kind::Text,
        body: String::new(),
        title: None,
        memo: None,
        size: None,
        file_kind: None,
        duration_sec: None,
        paths: Vec::new(),
        record_id: None,
        source: Some("自己写".into()),
        image_from: None,
    }
}

fn dto(line: &Line, note: Note) -> NoteDto {
    let image_url = line.image_path(&note).map(|p| asset_url(&p));
    NoteDto { note, image_url }
}

/// 试用结束后不再按所选样式画，回到默认样式；所选的仍留在设置里，激活后恢复
pub fn effective_style(pro: &Pro, chosen: &LineStyle) -> LineStyle {
    match pro {
        Pro::Expired => LineStyle::default(),
        _ => chosen.clone(),
    }
}

pub fn view(state: &AppState) -> LineView {
    // 先取设置再锁 line，两把锁不同时持有
    let (hotkey, chosen, on, side, pinned) = {
        let s = state.settings();
        let hotkey = match s.effective_line_hotkey() {
            "" => String::new(),
            k => crate::hotkey::label(k),
        };
        (
            hotkey,
            s.line_style.clone(),
            s.line_enabled,
            s.line_side,
            s.line_pinned,
        )
    };
    let line = state.line();
    let pro = line.status(now());
    LineView {
        available: crate::line::enabled(),
        style: effective_style(&pro, &chosen),
        pro,
        notes: line.notes().iter().map(|n| dto(&line, n.clone())).collect(),
        hung: line.hung_records(),
        hotkey,
        on,
        side,
        pinned,
    }
}

/// 写命令成功后广播完整视图；调用时不能持有 `line`、`settings` 锁
pub fn changed(app: &AppHandle) {
    emit(app, LINE_CHANGED, view(&app.state::<AppState>()));
}

fn find(line: &Line, id: u64) -> Result<Note, AppError> {
    line.notes()
        .iter()
        .find(|n| n.id == id)
        .cloned()
        .ok_or_else(|| LineError::NotFound.into())
}

/// 原图不在了就挂不了图片；视频封面还没生成时先不带封面
fn check_source(n: &mut NewNote) -> Result<(), AppError> {
    if n.image_from.as_ref().is_some_and(|p| !p.exists()) {
        if n.kind == Kind::Image {
            return Err(AppError::FileMissing);
        }
        n.image_from = None;
    }
    Ok(())
}

fn add(state: &AppState, mut n: NewNote, at: Option<usize>) -> Result<NoteDto, AppError> {
    check_source(&mut n)?;
    let mut line = state.line();
    let note = line.add(n, at, now())?;
    Ok(dto(&line, note))
}

#[tauri::command]
pub fn line_view(state: State<'_, AppState>) -> LineView {
    view(&state)
}

/// 把一条记录挂上绳子，`at` 为插入位置（None 挂在末尾）
#[tauri::command(async)]
pub fn line_hang_record(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    at: Option<usize>,
) -> Result<NoteDto, AppError> {
    hang_record(&app, &state, id, at)
}

/// `line_hang_record` 与便签快捷键共用：读出记录、挂上、通知面板
pub fn hang_record(
    app: &AppHandle,
    state: &AppState,
    id: i64,
    at: Option<usize>,
) -> Result<NoteDto, AppError> {
    // 先读 store 再锁 line，两把锁不同时持有
    let d = {
        let store = state.store();
        match store.live_summary(id)? {
            Some(_) => store.detail(id)?,
            None => None,
        }
    }
    .ok_or_else(|| AppError::Invalid("记录不存在".into()))?;
    let source = d.summary.app.as_ref().map(|a| a.name.clone());
    let note = add(state, note_from_detail(&d, source), at)?;
    log::info!(
        "便签 挂上 id={} 记录={id} kind={:?}",
        note.note.id,
        note.note.kind
    );
    changed(app);
    Ok(note)
}

/// 面板的挂钩按钮、面板可见时的便签快捷键：挂上 `id`（None 为最新一条）后放下绳子亮一下，没挂成由晾衣绳页面提示
#[tauri::command(async)]
pub fn line_hang_from_panel(app: AppHandle, id: Option<i64>) {
    crate::line::hang_and_flash(&app, id, false);
}

/// 面板里把行拖出顶边，每帧最多一次：`x`、`y` 为光标的屏幕逻辑坐标，`item` 是拖着的记录（页面画影子便签）
#[tauri::command]
pub fn line_drag_hover(app: AppHandle, x: f64, y: f64, item: crate::line::DragItem) {
    crate::line::drag_hover(&app, x, y, item);
}

/// 面板里拖着行到绳子那一侧松手：转给晾衣绳页面，页面按 `x`（顶部）或 `y`（左右两侧）算出位置后调 `line_hang_record`
#[tauri::command]
pub fn line_drag_drop(app: AppHandle, id: i64, x: f64, y: f64) {
    emit_to(
        &app,
        crate::line::LABEL,
        LINE_DROP,
        serde_json::json!({ "id": id, "x": x, "y": y }),
    );
}

/// 拖进来的文件，每个路径各成一张，从 `at` 起依次插入；碰到错误就停，已挂上的保留
#[tauri::command(async)]
pub fn line_hang_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    at: Option<usize>,
) -> Result<Vec<NoteDto>, AppError> {
    let mut out = Vec::with_capacity(paths.len());
    let mut failed = None;
    for p in &paths {
        let p = Path::new(p);
        let len = std::fs::metadata(p).map_or(0, |m| m.len());
        match add(&state, note_from_path(p, len), at.map(|a| a + out.len())) {
            Ok(n) => out.push(n),
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    log::info!("便签 拖入 n={} 挂上 {}", paths.len(), out.len());
    if !out.is_empty() {
        changed(&app);
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(out),
    }
}

/// 新写一张空白文字便签
#[tauri::command(async)]
pub fn line_add(
    app: AppHandle,
    state: State<'_, AppState>,
    at: Option<usize>,
) -> Result<NoteDto, AppError> {
    let note = add(&state, blank_note(), at)?;
    log::info!("便签 新写 id={}", note.note.id);
    changed(&app);
    Ok(note)
}

/// 文字、代码改 `body`，其余改 `memo`。文字、代码改成空等于摘下，返回 None
#[tauri::command(async)]
pub fn line_edit(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    body: Option<String>,
    memo: Option<String>,
) -> Result<Option<NoteDto>, AppError> {
    let edited = {
        let mut line = state.line();
        line.edit(id, body, memo).map(|n| dto(&line, n))
    };
    let out = match edited {
        Ok(n) => Some(n),
        Err(LineError::NotFound) => None,
        Err(e) => return Err(e.into()),
    };
    log::info!("便签 修改 id={id} 还在={}", out.is_some());
    changed(&app);
    Ok(out)
}

/// 往自己写的便签里贴图：请求体是 PNG 原始字节，便签 id 在请求头 `x-note-id`
#[tauri::command]
pub async fn line_paste_image(
    app: AppHandle,
    state: State<'_, AppState>,
    request: Request<'_>,
) -> Result<NoteDto, AppError> {
    let id = request
        .headers()
        .get("x-note-id")
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
        .ok_or_else(|| AppError::Invalid("缺少便签 id".into()))?;
    let InvokeBody::Raw(png) = request.body() else {
        return Err(AppError::Invalid("请求体应为 PNG 字节".into()));
    };
    check_png(png)?;
    let note = {
        let mut line = state.line();
        let n = line.set_image(id, png, now())?;
        dto(&line, n)
    };
    log::info!("便签 贴图 id={id} bytes={}", png.len());
    changed(&app);
    Ok(note)
}

/// 字节来自页面，先看 PNG 文件头
fn check_png(b: &[u8]) -> Result<(), AppError> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(())
    } else {
        Err(AppError::Invalid("粘贴的不是图片".into()))
    }
}

#[tauri::command(async)]
pub fn line_reorder(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<u64>,
) -> Result<(), AppError> {
    state.line().reorder(&ids)?;
    changed(&app);
    Ok(())
}

/// 高度来自页面：只收有限的非负数
fn check_places(places: &[(u64, Option<f64>)]) -> Result<(), AppError> {
    if places
        .iter()
        .any(|p| p.1.is_some_and(|y| !y.is_finite() || y < 0.0))
    {
        return Err(AppError::Invalid("便签高度不对".into()));
    }
    Ok(())
}

/// 左右两侧拖完：`places` 是新顺序，每张带它在绳子上的高度（None 按默认往下排），一次写入
#[tauri::command(async)]
pub fn line_place(
    app: AppHandle,
    state: State<'_, AppState>,
    places: Vec<(u64, Option<f64>)>,
) -> Result<(), AppError> {
    check_places(&places)?;
    state.line().place(&places)?;
    changed(&app);
    Ok(())
}

/// 设（`at` 为 Unix 秒）或取消（None）提醒，过去的时刻马上到期；试用结束后照常能改。成功后叫醒提醒线程重算
#[tauri::command(async)]
pub fn line_remind(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    at: Option<i64>,
) -> Result<NoteDto, AppError> {
    let note = {
        let mut line = state.line();
        let n = line.remind(id, at)?;
        dto(&line, n)
    };
    log::info!("便签 提醒 id={id} 设上={}", at.is_some());
    crate::remind::wake();
    changed(&app);
    Ok(note)
}

/// 最近摘下的便签和它原来的位置，撤销时按 id 取回。只留在内存里，最多 20 张，多了丢最早的；
/// 撤销不收页面交回的便签，页面改不了要恢复的内容（比如把路径换成别的文件）
#[derive(Default)]
struct Removed(Vec<(Note, usize)>);

const REMOVED_MAX: usize = 20;

impl Removed {
    fn put(&mut self, note: Note, at: usize) {
        self.0.retain(|(n, _)| n.id != note.id);
        self.0.push((note, at));
        if self.0.len() > REMOVED_MAX {
            self.0.remove(0);
        }
    }

    fn take(&mut self, id: u64) -> Option<(Note, usize)> {
        let i = self.0.iter().position(|(n, _)| n.id == id)?;
        Some(self.0.remove(i))
    }
}

/// 与 AppState 的锁不同时持有
static REMOVED: Mutex<Removed> = Mutex::new(Removed(Vec::new()));

fn removed() -> std::sync::MutexGuard<'static, Removed> {
    REMOVED.lock().unwrap_or_else(PoisonError::into_inner)
}

fn take_removed(id: u64) -> Result<(Note, usize), AppError> {
    removed()
        .take(id)
        .ok_or_else(|| AppError::Invalid("没有可撤销的便签".into()))
}

/// 摘下，返回便签和它原来的位置；撤销时把 id 交给 `line_restore`
#[tauri::command(async)]
pub fn line_remove(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
) -> Result<(NoteDto, usize), AppError> {
    let (note, at) = {
        let mut line = state.line();
        let (n, at) = line.remove(id)?;
        (dto(&line, n), at)
    };
    removed().put(note.note.clone(), at);
    log::info!("便签 摘下 id={id}");
    changed(&app);
    Ok((note, at))
}

/// 撤销摘下：放回后端留的那份，`at` 不给放回原来的位置
#[tauri::command(async)]
pub fn line_restore(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    at: Option<usize>,
) -> Result<(), AppError> {
    let (note, saved_at) = take_removed(id)?;
    // 没放回去（比如绳子挂满了）就留着，还能再撤销
    let back = note.clone();
    let r = state.line().restore(note, at.unwrap_or(saved_at));
    if let Err(e) = r {
        removed().put(back, saved_at);
        return Err(e.into());
    }
    log::info!("便签 撤销摘下 id={id}");
    // 带回来的提醒可能已经过了
    crate::remind::wake();
    changed(&app);
    Ok(())
}

/// 复制便签，见 `copy_plan`；都标记为自身写入，不进历史
#[tauri::command(async)]
pub async fn line_copy(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
) -> Result<CopyNote, AppError> {
    let (note, image) = {
        let line = state.line();
        let n = find(&line, id)?;
        let image = line.image_path(&n);
        (n, image)
    };
    // 原记录还在、文件也都在才按记录复制
    let flavors = match note.record_id {
        Some(rid) if !matches!(note.kind, Kind::Text | Kind::Code | Kind::Link) => {
            let alive = state.store().live_summary(rid)?.is_some();
            alive.then(|| precheck(&state, rid).ok()).flatten()
        }
        _ => None,
    };
    let plan = copy_plan(&note, flavors.is_some(), image);
    let how = match plan {
        CopyPlan::Text(t) => {
            write_text_plain(&state, &t)?;
            CopyNote::Copied
        }
        CopyPlan::Record(rid) => {
            let flavors = flavors.unwrap_or_default();
            write_then_paste(&app, &state, &flavors, &[rid], false).await?;
            CopyNote::Copied
        }
        CopyPlan::Image(p) => {
            let png = png_bytes(&p, id)?;
            write_marked(&state, || {
                xpaste_platform::write(&xpaste_platform::png_flavors(png))
            })?;
            CopyNote::Copied
        }
        CopyPlan::Paths(s) if s.is_empty() => return Err(AppError::FileMissing),
        CopyPlan::Paths(s) => {
            write_text_plain(&state, &s)?;
            CopyNote::CopiedPath
        }
    };
    log::info!("便签 复制 id={id} kind={:?} 方式={how:?}", note.kind);
    Ok(how)
}

/// 便签里的图片转成 PNG 字节写剪切板（原图可能是 JPEG、GIF 等）
fn png_bytes(p: &Path, id: u64) -> Result<Vec<u8>, AppError> {
    let img = image::open(p).map_err(|e| {
        log::warn!("便签图片读取失败 id={id}");
        log::debug!("图片错误：{e}");
        AppError::FileMissing
    })?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|_| AppError::ClipboardWrite)?;
    Ok(out.into_inner())
}

/// 链接用默认浏览器打开；文件、视频在访达 / 资源管理器中显示
#[tauri::command(async)]
pub fn line_open(app: AppHandle, state: State<'_, AppState>, id: u64) -> Result<(), AppError> {
    let note = find(&state.line(), id)?;
    if note.kind == Kind::Link {
        return crate::commands::panel::open_url(app, note.body);
    }
    match note.paths.into_iter().next() {
        Some(p) => crate::commands::panel::reveal_file(app, p),
        None => Err(AppError::Invalid("这张便签没有可打开的文件".into())),
    }
}

/// 大卡片里要播放的源视频：只有视频便签，扩展名是视频，且源文件还在
fn video_path(note: &Note) -> Option<&Path> {
    (note.kind == Kind::Video)
        .then(|| note.paths.first())
        .flatten()
        .map(Path::new)
        .filter(|p| note_from_path(p, 0).kind == Kind::Video && p.is_file())
}

/// 大卡片里播放视频：只把这张便签的源视频授权给 asset 协议，返回它的 asset URL；源文件已移动或删除时为 None
#[tauri::command(async)]
pub fn line_preview_src(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
) -> Result<Option<String>, AppError> {
    let note = find(&state.line(), id)?;
    let Some(p) = video_path(&note) else {
        return Ok(None);
    };
    app.asset_protocol_scope().allow_file(p)?;
    Ok(Some(asset_url(p)))
}

/// 页面开始监听 line-show 后调用；之前不检测顶端
#[tauri::command]
pub fn line_ready(app: AppHandle) {
    crate::line::ready(&app);
}

/// 晾衣绳页面按内容报窗口尺寸：顶部是高，左右两侧是宽
#[tauri::command]
pub fn line_resize(app: AppHandle, extent: f64) {
    crate::line::resize(&app, extent);
}

/// 一直显示时页面报能点的地方：`[x, y, w, h]`，窗口内逻辑像素；变了才报
#[tauri::command]
pub fn line_hit_rects(rects: Vec<[f64; 4]>) {
    crate::line::set_hits(rects);
}

/// 设置里“位置”下面的程序坞提示；程序坞不在这一侧、或碰边照常能放下时为空串
#[tauri::command(async)]
pub fn line_side_hint(app: AppHandle, side: xpaste_core::LineSide) -> String {
    crate::line::side_hint(&app, side)
}

/// 页面要收起（离开便签区域、Esc、快捷键挂上后停够时间）：发 `line-hide`，页面收完再调 `line_hidden`
#[tauri::command]
pub fn line_hide(app: AppHandle) {
    crate::line::hide(&app);
}

/// 页面播完收起，窗口真正隐藏
#[tauri::command]
pub fn line_hidden(app: AppHandle) {
    crate::line::hidden(&app);
}

/// 页面要键盘（`true`）或交还（`false`）
#[tauri::command]
pub fn line_keyboard(app: AppHandle, on: bool) {
    crate::line::keyboard(&app, on);
}

#[tauri::command(async)]
pub fn pro_status(state: State<'_, AppState>) -> Pro {
    state.line().status(now())
}

/// 激活授权码；成功后广播视图（里面带新的 Pro 状态）。日志不带授权码和邮箱
#[tauri::command(async)]
pub fn pro_activate(
    app: AppHandle,
    state: State<'_, AppState>,
    code: String,
) -> Result<Pro, AppError> {
    let pro = state.line().activate(code.trim(), now())?;
    log::info!("授权码已激活");
    changed(&app);
    Ok(pro)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xpaste_core::{FileEntry, ImageInfo, RecordSummary, VideoInfo};
    use xpaste_pro::LicenseError;

    fn detail(kind: Kind, title: &str) -> RecordDetail {
        RecordDetail {
            summary: RecordSummary {
                id: 7,
                kind,
                title: title.into(),
                app: None,
                copied_at: 0,
                uses: 0,
                pinned: false,
                size: 0,
                lang: None,
                thumb: None,
                duration_sec: None,
                ocr_state: 0,
                snippet_body: None,
                lines: None,
                url: None,
                file_count: None,
            },
            text: None,
            truncated: false,
            link_title: None,
            files: Vec::new(),
            image: None,
            video: None,
            ocr_text: None,
            translation: None,
            translation_lang: None,
            translation_engine: None,
        }
    }

    fn file(path: &str) -> FileEntry {
        let p = Path::new(path);
        FileEntry {
            path: path.into(),
            name: p.file_name().unwrap().to_string_lossy().into(),
            ext: p.extension().unwrap().to_string_lossy().to_lowercase(),
            size: 3,
            exists: true,
        }
    }

    fn note(kind: Kind, body: &str) -> Note {
        Note {
            id: 1,
            kind,
            body: body.into(),
            title: None,
            memo: None,
            image: None,
            size: None,
            file_kind: None,
            duration_sec: None,
            paths: Vec::new(),
            record_id: Some(7),
            source: None,
            created_at: 0,
            remind_at: None,
            side_y: None,
        }
    }

    #[test]
    fn preview_src_only_for_existing_video() {
        let f =
            std::env::temp_dir().join(format!("xpaste-line-preview-{}.mov", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        let mut v = note(Kind::Video, "录屏.mov");
        v.paths = vec![f.to_string_lossy().into()];
        assert_eq!(video_path(&v), Some(f.as_path()));
        // 视频便签的路径不是视频文件，不授权
        let txt =
            std::env::temp_dir().join(format!("xpaste-line-preview-{}.txt", std::process::id()));
        std::fs::write(&txt, b"x").unwrap();
        let fake = Note {
            paths: vec![txt.to_string_lossy().into()],
            ..v.clone()
        };
        assert_eq!(video_path(&fake), None);
        std::fs::remove_file(&txt).unwrap();
        // 文件便签不授权
        let file = Note {
            kind: Kind::File,
            ..v.clone()
        };
        assert_eq!(video_path(&file), None);
        // 源文件不在了、没有路径
        std::fs::remove_file(&f).unwrap();
        assert_eq!(video_path(&v), None);
        assert_eq!(video_path(&note(Kind::Video, "a.mov")), None);
    }

    #[test]
    fn link_detail_maps_title_and_url() {
        let mut d = detail(Kind::Link, "https://example.com");
        d.text = Some("https://example.com/a".into());
        d.summary.url = Some("https://example.com/a".into());
        d.link_title = Some("示例".into());
        let n = note_from_detail(&d, Some("Safari".into()));
        assert_eq!(n.kind, Kind::Link);
        assert_eq!(n.body, "https://example.com/a");
        assert_eq!(n.title.as_deref(), Some("示例"));
        assert_eq!(n.record_id, Some(7));
        assert_eq!(n.source.as_deref(), Some("Safari"));
    }

    #[test]
    fn text_detail_truncates_at_64k() {
        let mut d = detail(Kind::Text, "x");
        d.text = Some("字".repeat(30_000)); // 90 000 字节
        let n = note_from_detail(&d, None);
        assert!(n.body.len() <= 64 * 1024);
        assert!(n.body.chars().all(|c| c == '字'));
    }

    #[test]
    fn image_detail_copies_original() {
        // 位图记录：原图在 images/<id>.png
        let mut d = detail(Kind::Image, "截图 1200×800");
        d.image = Some(ImageInfo {
            src: "/data/images/7.png".into(),
            w: 1200,
            h: 800,
        });
        let n = note_from_detail(&d, None);
        assert_eq!(n.kind, Kind::Image);
        assert_eq!(n.image_from, Some(PathBuf::from("/data/images/7.png")));
        assert_eq!(n.body, "截图 1200×800");
        // 图片文件引用：原图是那个文件
        let mut d = detail(Kind::Image, "a.jpg");
        d.files = vec![file("/u/a.jpg")];
        let n = note_from_detail(&d, None);
        assert_eq!(n.image_from, Some(PathBuf::from("/u/a.jpg")));
    }

    #[test]
    fn video_detail_uses_thumb_and_source() {
        let mut d = detail(Kind::Video, "片.mov");
        d.summary.thumb = Some("/data/thumbs/7.png".into());
        d.summary.duration_sec = Some(12.5);
        d.files = vec![file("/u/片.mov")];
        d.video = Some(VideoInfo {
            src: Some("/u/片.mov".into()),
            w: 0,
            h: 0,
            fps: 0.0,
            codec: String::new(),
        });
        let n = note_from_detail(&d, None);
        assert_eq!(n.image_from, Some(PathBuf::from("/data/thumbs/7.png")));
        assert_eq!(n.paths, ["/u/片.mov"]);
        assert_eq!(n.duration_sec, Some(12.5));
    }

    #[test]
    fn file_detail_keeps_all_paths() {
        let mut d = detail(Kind::File, "a.md 等 2 个文件");
        d.summary.size = 99;
        d.files = vec![file("/u/a.md"), file("/u/b.txt")];
        let n = note_from_detail(&d, None);
        assert_eq!(n.kind, Kind::File);
        assert_eq!(n.paths, ["/u/a.md", "/u/b.txt"]);
        assert_eq!(n.file_kind.as_deref(), Some("MD"));
        assert_eq!(n.size, Some(99));
        assert!(n.image_from.is_none());
    }

    #[test]
    fn path_kind_by_extension() {
        let a = note_from_path(Path::new("/u/a.PNG"), 5);
        assert_eq!(a.kind, Kind::Image);
        assert_eq!(a.image_from, Some(PathBuf::from("/u/a.PNG")));
        assert_eq!(a.body, "a.PNG");
        assert_eq!(a.size, Some(5));
        let b = note_from_path(Path::new("/u/b.mov"), 5);
        assert_eq!(b.kind, Kind::Video);
        assert_eq!(b.paths, ["/u/b.mov"]);
        let c = note_from_path(Path::new("/u/c.xlsx"), 5);
        assert_eq!(c.kind, Kind::File);
        assert_eq!(c.file_kind.as_deref(), Some("XLSX"));
        assert!(c.record_id.is_none());
    }

    #[test]
    fn copy_plan_text_uses_note_body() {
        // 改过的文字便签复制便签里的正文，不管原记录在不在
        for alive in [true, false] {
            assert!(matches!(
                copy_plan(&note(Kind::Text, "改过的"), alive, None),
                CopyPlan::Text(t) if t == "改过的"
            ));
        }
        assert!(matches!(
            copy_plan(&note(Kind::Link, "https://a.b"), true, None),
            CopyPlan::Text(t) if t == "https://a.b"
        ));
    }

    #[test]
    fn copy_plan_falls_back_when_record_gone() {
        let img = PathBuf::from("/data/line/1.png");
        let n = note(Kind::Image, "截图");
        assert!(matches!(
            copy_plan(&n, true, Some(img.clone())),
            CopyPlan::Record(7)
        ));
        assert!(matches!(
            copy_plan(&n, false, Some(img.clone())),
            CopyPlan::Image(p) if p == img
        ));
        let mut f = note(Kind::File, "a 等 2 个文件");
        f.paths = vec!["a".into(), "b".into()];
        assert!(matches!(copy_plan(&f, false, None), CopyPlan::Paths(s) if s == "a\nb"));
        f.kind = Kind::Video;
        assert!(matches!(copy_plan(&f, true, None), CopyPlan::Record(7)));
    }

    #[test]
    fn stub_commands_report_unavailable() {
        let state = AppState::for_test();
        let v = view(&state);
        assert_eq!(v.available, crate::line::enabled());
        if !crate::line::enabled() {
            assert!(matches!(v.pro, Pro::Unavailable));
            assert!(v.notes.is_empty() && v.hung.is_empty());
            let e = AppError::from(state.line().add(blank_note(), None, 0).err().unwrap());
            assert_eq!(e.to_string(), "这个版本不含晾衣绳");
            let e = AppError::from(state.line().activate("XP1-a.b", 0).err().unwrap());
            assert_eq!(e.to_string(), "这个版本不含晾衣绳");
        }
    }

    #[test]
    fn expired_trial_uses_default_style() {
        let mine = LineStyle {
            rope: "lights".into(),
            ..LineStyle::default()
        };
        assert_eq!(effective_style(&Pro::Expired, &mine), LineStyle::default());
        assert_eq!(effective_style(&Pro::Trial { days_left: 3 }, &mine), mine);
    }

    #[test]
    fn view_carries_hotkey_label() {
        let state = AppState::for_test();
        state.settings().line_hotkey = "Alt+Shift+N".into();
        assert_eq!(view(&state).hotkey, crate::hotkey::label("Alt+Shift+N"));
        state.settings().line_hotkey = String::new();
        assert_eq!(view(&state).hotkey, "");
    }

    #[test]
    fn removed_keeps_server_copy_for_undo() {
        let mut r = Removed::default();
        // 不认识的 id 撤销不了
        assert!(r.take(9).is_none());
        let mut n = note(Kind::Text, "a");
        n.id = 9;
        r.put(n, 3);
        let (back, at) = r.take(9).unwrap();
        assert_eq!((back.id, back.body.as_str(), at), (9, "a", 3));
        // 取过一次就没了
        assert!(r.take(9).is_none());
        // 最多留 20 张，丢最早的
        for id in 1..=21 {
            let mut n = note(Kind::Text, "x");
            n.id = id;
            r.put(n, 0);
        }
        assert!(r.take(1).is_none());
        assert!(r.take(2).is_some() && r.take(21).is_some());
    }

    #[test]
    fn restore_unknown_id_fails() {
        assert!(matches!(take_removed(u64::MAX), Err(AppError::Invalid(_))));
    }

    #[test]
    fn place_heights_must_be_finite_and_non_negative() {
        assert!(check_places(&[(1, Some(0.0)), (2, Some(120.5)), (3, None)]).is_ok());
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(matches!(
                check_places(&[(1, None), (2, Some(bad))]),
                Err(AppError::Invalid(_))
            ));
        }
    }

    #[test]
    fn paste_image_needs_png_signature() {
        assert!(check_png(b"\x89PNG\r\n\x1a\n\0\0").is_ok());
        for bad in [&b""[..], b"\x89PNG", b"GIF89a\0\0\0\0"] {
            assert_eq!(check_png(bad).unwrap_err().to_string(), "粘贴的不是图片");
        }
    }

    #[test]
    fn errors_serialize_with_kind() {
        let v = |e: AppError| serde_json::to_value(e).unwrap();
        assert_eq!(
            v(LineError::Duplicate(3).into()),
            serde_json::json!({"kind": "duplicate", "message": "已经挂着了", "id": 3})
        );
        assert_eq!(v(LineError::Expired.into())["kind"], "lineExpired");
        assert_eq!(
            v(LineError::Full.into())["message"],
            "绳子挂满了（12 张），先摘下几张"
        );
        assert_eq!(
            v(LicenseError::OldMajor(1).into())["message"],
            "这个授权码只适用于 1.x"
        );
        assert_eq!(
            v(LicenseError::Io("PermissionDenied".into()).into())["message"],
            "授权码保存失败，请重试"
        );
    }
}
