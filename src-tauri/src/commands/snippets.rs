//! 片段的导出与导入（M25 规则 11）：只有文字类片段进文件，JSON，同名同内容的跳过。

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};

use crate::clock::now_ms;
use crate::error::AppError;
use crate::events::{RECORD_ADDED, emit_summary};
use crate::state::AppState;

/// 导出文件的内容。`app`、`kind` 用来认出这是不是 xPaste 导出的片段文件。
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetFile {
    pub app: String,
    pub kind: String,
    pub version: u32,
    pub exported_at: i64,
    pub snippets: Vec<SnippetItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnippetItem {
    pub name: String,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    /// 写进文件的片段数
    pub exported: u32,
    /// 图片、文件片段：依赖本机的文件，不导出
    pub skipped: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: u32,
    /// 名字和内容都一样的已有片段
    pub skipped: u32,
    /// 名字不合规（空、超过 100 个字）或内容为空的，没有导入
    pub invalid: u32,
}

/// 解析导出文件；不是 xPaste 导出的片段文件时报错（M25 规则 11）。
pub fn parse_file(bytes: &[u8]) -> Result<SnippetFile, AppError> {
    let bad = || AppError::Invalid("这不是 xPaste 导出的片段文件".into());
    let f: SnippetFile = serde_json::from_slice(bytes).map_err(|_| bad())?;
    if f.app != "xPaste" || f.kind != "snippets" {
        return Err(bad());
    }
    Ok(f)
}

/// 挑出要新建的：名字和内容都与已有片段相同的跳过；文件里重复的只建一次。返回（要新建的, 跳过数）。
pub fn plan_import(items: &[SnippetItem], existing: &[SnippetItem]) -> (Vec<SnippetItem>, u32) {
    let mut have: Vec<SnippetItem> = existing.to_vec();
    let mut create = Vec::new();
    let mut skipped = 0;
    // 名字按存入时的口径去掉首尾空白再比（create_snippet 经 snippet_name 会 trim）
    let items: Vec<SnippetItem> = items
        .iter()
        .map(|it| SnippetItem {
            name: it.name.trim().into(),
            text: it.text.clone(),
        })
        .collect();
    for it in &items {
        if have.iter().any(|e| e.name == it.name && e.text == it.text) {
            skipped += 1;
        } else {
            create.push(it.clone());
            have.push(it.clone());
        }
    }
    (create, skipped)
}

fn existing(state: &AppState) -> Result<Vec<SnippetItem>, AppError> {
    Ok(state
        .store()
        .text_snippets()?
        .into_iter()
        .map(|(name, text)| SnippetItem { name, text })
        .collect())
}

/// 把文字类片段写成 JSON 文件；`path` 由前端的系统保存窗口给出。
#[tauri::command(async)]
pub fn export_snippets(state: State<'_, AppState>, path: String) -> Result<ExportReport, AppError> {
    let (items, total) = {
        let store = state.store();
        (store.text_snippets()?, store.count_snippets()?)
    };
    let file = SnippetFile {
        app: "xPaste".into(),
        kind: "snippets".into(),
        version: 1,
        exported_at: now_ms(),
        snippets: items
            .into_iter()
            .map(|(name, text)| SnippetItem { name, text })
            .collect(),
    };
    let exported = file.snippets.len() as u32;
    let json = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    std::fs::write(&path, json)?;
    log::info!("导出片段 {exported} 个");
    Ok(ExportReport {
        exported,
        skipped: total.saturating_sub(exported),
    })
}

/// 读入导出文件，新建没有的片段；名字和内容都一样的跳过。
#[tauri::command(async)]
pub fn import_snippets(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<ImportReport, AppError> {
    let bytes = std::fs::read(&path)?;
    let file = parse_file(&bytes)?;
    let (create, skipped) = plan_import(&file.snippets, &existing(&state)?);
    let (mut imported, mut invalid) = (0, 0);
    for it in create {
        let r = state.store().create_snippet(&it.name, &it.text, now_ms());
        match r {
            Ok(id) => {
                imported += 1;
                emit_summary(&app, RECORD_ADDED, id);
            }
            Err(xpaste_core::CoreError::InvalidSettings(_)) => invalid += 1,
            Err(e) => return Err(e.into()),
        }
    }
    log::info!("导入片段 {imported} 个，跳过 {skipped} 个已有，{invalid} 个不合规");
    Ok(ImportReport {
        imported,
        skipped,
        invalid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn it(name: &str, text: &str) -> SnippetItem {
        SnippetItem {
            name: name.into(),
            text: text.into(),
        }
    }

    #[test]
    fn parse_rejects_other_files() {
        assert!(parse_file(b"hello").is_err());
        assert!(
            parse_file(
                br#"{"app":"other","kind":"snippets","version":1,"exportedAt":0,"snippets":[]}"#
            )
            .is_err()
        );
        let f = parse_file(br#"{"app":"xPaste","kind":"snippets","version":1,"exportedAt":0,"snippets":[{"name":"a","text":"b"}]}"#).unwrap();
        assert_eq!(f.snippets, [it("a", "b")]);
    }

    #[test]
    fn plan_import_skips_same_name_and_text() {
        let existing = [it("地址", "上海"), it("电话", "138")];
        let (create, skipped) = plan_import(
            &[
                it("地址", "上海"),
                it("地址", "北京"),
                it("新", "x"),
                it("新", "x"),
            ],
            &existing,
        );
        // 同名不同内容照常导入；文件里重复的只建一次
        assert_eq!(create, [it("地址", "北京"), it("新", "x")]);
        assert_eq!(skipped, 2);
        // 名字首尾空白按存入时的口径去掉再比（create_snippet 会 trim）
        let (create, skipped) = plan_import(
            &[it(" 地址 ", "上海"), it("新 ", "y"), it("新", "y")],
            &existing,
        );
        assert_eq!(create, [it("新", "y")]);
        assert_eq!(skipped, 2);
    }

    #[test]
    fn export_then_import_round_trip() {
        let state = AppState::for_test();
        let (a, b) = {
            let mut s = state.store();
            (
                s.create_snippet("地址", "上海市徐汇区", 1).unwrap(),
                s.create_snippet("命令", "pnpm i", 2).unwrap(),
            )
        };
        assert!(a > 0 && b > 0);
        let existing = existing(&state).unwrap();
        assert_eq!(existing, [it("地址", "上海市徐汇区"), it("命令", "pnpm i")]);
        let (create, skipped) = plan_import(&existing, &existing);
        assert!(create.is_empty());
        assert_eq!(skipped, 2);
    }
}
