//! 便签提醒的调度线程（只在带晾衣绳的构建里启动）：睡到最早的提醒时刻（最多 30 s），醒来把到期的便签清掉提醒、
//! 放下绳子（`How::Remind`）、给页面发 `line-remind`，并发一条系统通知。设、取消、撤销摘下后由 `wake` 叫醒重算。
//! 页面就绪（`line_ready`）之前不发，所以关着应用时错过的提醒在页面就绪后立即补发。
//! 日志只记条数和便签 id；系统通知是给用户看的，正文里可以有便签开头几个字。

use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Manager};
use xpaste_pro::{Kind, Note};

use crate::clock::now_ms;
use crate::events::{LINE_REMIND, emit_to};
use crate::line::{How, LABEL};
use crate::state::AppState;

/// 最多睡这么久就醒来看一眼（系统睡眠、改了时钟之后不至于晚太多）
const MAX_WAIT: i64 = 30;
/// 通知里便签开头最多这么多个字符
const EXCERPT: usize = 40;

static KICK: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

fn kick() -> MutexGuard<'static, bool> {
    KICK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 提醒变了（设、取消、撤销摘下带回来的）或页面刚就绪：让调度线程马上重算
pub fn wake() {
    *kick() = true;
    WAKE.notify_one();
}

/// 到期的便签 id（`remind_at <= now`），以及下一个还没到的提醒时刻。`stuck` 是清不掉提醒的 (id, 时刻)，不再算到期
pub fn due(notes: &[Note], now: i64, stuck: &[(u64, i64)]) -> (Vec<u64>, Option<i64>) {
    let mut ids = Vec::new();
    let mut next: Option<i64> = None;
    for n in notes {
        let Some(at) = n.remind_at else { continue };
        if stuck.contains(&(n.id, at)) {
            continue;
        }
        if at <= now {
            ids.push(n.id);
        } else {
            next = Some(next.map_or(at, |t| t.min(at)));
        }
    }
    (ids, next)
}

/// 便签开头一段：有备注用备注，文字、代码用正文，链接用标题（没有就用网址），其余用文件名；空白压成一个空格
fn excerpt(n: &Note) -> String {
    let memo = n.memo.as_deref().filter(|m| !m.trim().is_empty());
    let raw = memo.unwrap_or(match n.kind {
        Kind::Link => n
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(&n.body),
        _ => &n.body,
    });
    let flat = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(EXCERPT).collect();
    if flat.chars().count() > EXCERPT {
        out.push('…');
    }
    out
}

/// 系统通知的正文：一条“提醒：<开头>”，多条合并成“N 条提醒到了”
pub fn notice(notes: &[Note]) -> String {
    match notes {
        [n] => format!("提醒：{}", excerpt(n)),
        _ => format!("{} 条提醒到了", notes.len()),
    }
}

pub fn start(app: &AppHandle) {
    let app = app.clone();
    if let Err(e) = std::thread::Builder::new()
        .name("line-remind".into())
        .spawn(move || run(&app))
    {
        log::warn!("提醒线程启动失败：{e}");
    }
}

fn run(app: &AppHandle) {
    let mut stuck = Vec::new();
    loop {
        let mut wait = MAX_WAIT;
        if crate::line::is_ready() {
            let now = now_ms() / 1000;
            let (ids, next) = due(app.state::<AppState>().line().notes(), now, &stuck);
            if !ids.is_empty() {
                fire(app, &ids, &mut stuck);
                continue;
            }
            if let Some(t) = next {
                wait = (t - now).clamp(1, MAX_WAIT);
            }
        }
        let k = kick();
        let (mut k, _) = WAKE
            .wait_timeout_while(k, Duration::from_secs(wait as u64), |k| !*k)
            .unwrap_or_else(PoisonError::into_inner);
        *k = false;
    }
}

/// 到期的一起处理：先清掉提醒（持久化），再放下绳子、让这几张高亮、发一条通知
fn fire(app: &AppHandle, ids: &[u64], stuck: &mut Vec<(u64, i64)>) {
    let state = app.state::<AppState>();
    let mut hit = Vec::new();
    {
        let mut line = state.line();
        for &id in ids {
            let Some(n) = line.notes().iter().find(|n| n.id == id).cloned() else {
                continue;
            };
            // 清不掉（写不进 line.json）也提醒这一次，本次运行不再重复
            if let Err(e) = line.remind(id, None) {
                log::warn!("便签 {id} 的提醒清不掉，本次运行不再提醒：{e:?}");
                stuck.push((id, n.remind_at.unwrap_or_default()));
            }
            hit.push(n);
        }
    }
    if hit.is_empty() {
        return;
    }
    log::info!("提醒到了 {} 条", hit.len());
    crate::commands::line::changed(app);
    crate::line::show(app, How::Remind);
    for n in &hit {
        emit_to(app, LABEL, LINE_REMIND, serde_json::json!({ "id": n.id }));
    }
    crate::notify(app, &notice(&hit));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: u64, kind: Kind, body: &str, at: Option<i64>) -> Note {
        Note {
            id,
            kind,
            body: body.into(),
            title: None,
            memo: None,
            image: None,
            size: None,
            file_kind: None,
            duration_sec: None,
            paths: Vec::new(),
            record_id: None,
            source: None,
            created_at: 0,
            remind_at: at,
        }
    }

    const NOW: i64 = 1_000_000;

    #[test]
    fn due_none_and_pending() {
        assert_eq!(due(&[], NOW, &[]), (vec![], None));
        let ns = [note(1, Kind::Text, "a", None)];
        assert_eq!(due(&ns, NOW, &[]), (vec![], None));
        let ns = [note(1, Kind::Text, "a", Some(NOW + 90))];
        assert_eq!(due(&ns, NOW, &[]), (vec![], Some(NOW + 90)));
    }

    #[test]
    fn due_now_many_and_long_overdue() {
        let ns = [
            note(1, Kind::Text, "a", Some(NOW)),
            note(2, Kind::Text, "b", Some(NOW + 600)),
            note(3, Kind::Text, "c", Some(NOW - 30 * 86_400)),
            note(4, Kind::Text, "d", Some(NOW + 60)),
            note(5, Kind::Text, "e", None),
        ];
        assert_eq!(due(&ns, NOW, &[]), (vec![1, 3], Some(NOW + 60)));
        // 清不掉的那条不再算到期，也不算下一次
        assert_eq!(
            due(&ns, NOW, &[(1, NOW), (4, NOW + 60)]),
            (vec![3], Some(NOW + 600))
        );
        // 同一张重新设了别的时刻，照常提醒
        assert_eq!(due(&ns, NOW, &[(1, NOW - 5)]).0, vec![1, 3]);
    }

    #[test]
    fn notice_uses_memo_then_body_title_name() {
        let mut t = note(1, Kind::Text, "  报销单\n本周五  截止 ", Some(NOW));
        assert_eq!(notice(std::slice::from_ref(&t)), "提醒：报销单 本周五 截止");
        t.memo = Some("交给财务".into());
        assert_eq!(notice(std::slice::from_ref(&t)), "提醒：交给财务");
        let mut l = note(2, Kind::Link, "https://a.b/c", None);
        assert_eq!(notice(std::slice::from_ref(&l)), "提醒：https://a.b/c");
        l.title = Some("设计稿".into());
        assert_eq!(notice(std::slice::from_ref(&l)), "提醒：设计稿");
        let f = note(3, Kind::File, "纪要.md", None);
        assert_eq!(notice(std::slice::from_ref(&f)), "提醒：纪要.md");
        let long = note(4, Kind::Text, &"字".repeat(41), None);
        assert_eq!(notice(&[long]), format!("提醒：{}…", "字".repeat(40)));
        assert_eq!(notice(&[t, l, f]), "3 条提醒到了");
    }
}
