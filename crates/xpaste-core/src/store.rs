//! SQLite 存储：建库、迁移、键值与设置持久化，记录读写、去重与清理。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde_json::Value;

use crate::{
    AppSummary, CoreError, FileEntry, Flavor, ImageInfo, Kind, NewRecord, RecordDetail,
    RecordSummary, SearchHit, Settings, VideoInfo, ocr_state, pinyin,
};

/// 按版本排列的迁移脚本；第 i 项把 `user_version` 从 i 升到 i+1。
const MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, icon_path TEXT);
CREATE TABLE records (
  id INTEGER PRIMARY KEY AUTOINCREMENT, hash BLOB NOT NULL UNIQUE, kind TEXT NOT NULL, title TEXT NOT NULL,
  text TEXT, paths TEXT, link_title TEXT, lang TEXT, size INTEGER NOT NULL, meta TEXT,
  search_text TEXT NOT NULL, app_id TEXT REFERENCES apps(id), copied_at INTEGER NOT NULL,
  uses INTEGER NOT NULL DEFAULT 0, pinned INTEGER NOT NULL DEFAULT 0, deleted_at INTEGER);
CREATE INDEX records_copied_at ON records(copied_at);
CREATE TABLE flavors (
  record_id INTEGER NOT NULL REFERENCES records(id) ON DELETE CASCADE,
  item INTEGER NOT NULL CHECK (item < 65536), format TEXT NOT NULL, data BLOB, file TEXT,
  PRIMARY KEY (record_id, item, format));
CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
",
    // 阶段 9：删去颜色类型，已有颜色记录按文本处理。颜色文本是单行 HEX，去掉首尾空白（含制表、换行）即文本标题规则
    "UPDATE records SET kind = 'text', title = trim(text, ' ' || char(9, 10, 13)) WHERE kind = 'color';",
    // 阶段 9.1：标题进搜索文本（位图截图的自动标题此前搜不到）
    "UPDATE records SET search_text = search_text || char(10) || lower(title) WHERE title <> '' AND instr(search_text, lower(title)) = 0;",
    // 阶段 10：图片文字识别。已有图片默认 0（待识别）进回填队列；非图片的 0 不被消费
    "ALTER TABLE records ADD COLUMN ocr_text TEXT;
ALTER TABLE records ADD COLUMN ocr_state INTEGER NOT NULL DEFAULT 0;",
    // 搜索索引：search_text 的 trigram 全文索引（外部内容表，不复制正文），三个触发器保持同步，
    // 建表时按已有记录重建一次。≥ 3 个字符的查询走 MATCH，更短的仍用 instr 扫描（见 search_sql）
    r"
CREATE VIRTUAL TABLE fts USING fts5(search_text, content='records', content_rowid='id', tokenize='trigram');
CREATE TRIGGER records_fts_ai AFTER INSERT ON records BEGIN
  INSERT INTO fts(rowid, search_text) VALUES (new.id, new.search_text);
END;
CREATE TRIGGER records_fts_ad AFTER DELETE ON records BEGIN
  INSERT INTO fts(fts, rowid, search_text) VALUES ('delete', old.id, old.search_text);
END;
CREATE TRIGGER records_fts_au AFTER UPDATE OF search_text ON records BEGIN
  INSERT INTO fts(fts, rowid, search_text) VALUES ('delete', old.id, old.search_text);
  INSERT INTO fts(rowid, search_text) VALUES (new.id, new.search_text);
END;
INSERT INTO fts(fts) VALUES ('rebuild');
",
    // 拼音搜索：py_text 存 search_text 的拼音索引行（'' 没有汉字），fts_py 是它区分大小写的 trigram 索引
    // （首字母行大写、全拼音节前带 '，见 pinyin::index_text），触发器同步；先 rebuild 把已有行按 NULL 登记，
    // 之后补建（migrate 在同一个事务里调用 backfill_pinyin）更新时触发器的 'delete' 才对得上索引条目
    r"
ALTER TABLE records ADD COLUMN py_text TEXT;
CREATE VIRTUAL TABLE fts_py USING fts5(py_text, content='records', content_rowid='id', tokenize='trigram case_sensitive 1');
CREATE TRIGGER records_fts_py_ai AFTER INSERT ON records BEGIN
  INSERT INTO fts_py(rowid, py_text) VALUES (new.id, new.py_text);
END;
CREATE TRIGGER records_fts_py_ad AFTER DELETE ON records BEGIN
  INSERT INTO fts_py(fts_py, rowid, py_text) VALUES ('delete', old.id, old.py_text);
END;
CREATE TRIGGER records_fts_py_au AFTER UPDATE OF py_text ON records BEGIN
  INSERT INTO fts_py(fts_py, rowid, py_text) VALUES ('delete', old.id, old.py_text);
  INSERT INTO fts_py(rowid, py_text) VALUES (new.id, new.py_text);
END;
INSERT INTO fts_py(fts_py) VALUES ('rebuild');
",
];

const SUBDIRS: [&str; 4] = ["blobs", "images", "thumbs", "icons"];

/// 超过此大小的 flavor 数据写到 `blobs/<id>/<n>`。
const INLINE_MAX: usize = 1_048_576;
/// 详情返回的正文上限（字节）。
const DETAIL_TEXT_MAX: usize = 200 * 1024;
const DAY_MS: i64 = 86_400_000;

const LIVE_UNPINNED: &str = "pinned = 0 AND deleted_at IS NULL";

const SUMMARY_SQL: &str = "SELECT r.id, r.kind, r.title, a.name, a.icon_path, r.copied_at, r.uses,
  r.pinned, r.size, r.lang, r.meta, r.ocr_state FROM records r LEFT JOIN apps a ON a.id = r.app_id";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Upsert {
    Added(i64),
    Merged(i64),
}

pub struct Store {
    conn: Connection,
    pub dir: PathBuf,
}

impl Store {
    /// 打开 `dir/xpaste.db`（不存在则创建），并建好附属目录。
    pub fn open(dir: &Path) -> Result<Store, CoreError> {
        make_dirs(dir)?;
        Self::init(Connection::open(dir.join("xpaste.db"))?, dir)
    }

    /// 测试用：内存数据库，文件仍放在 `dir` 下。
    pub fn open_in_memory(dir: &Path) -> Result<Store, CoreError> {
        make_dirs(dir)?;
        Self::init(Connection::open_in_memory()?, dir)
    }

    fn init(mut conn: Connection, dir: &Path) -> Result<Store, CoreError> {
        // 内存库返回 "memory"，无害
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut conn)?;
        Ok(Store {
            conn,
            dir: dir.to_path_buf(),
        })
    }

    pub fn schema_version(&self) -> Result<u32, CoreError> {
        Ok(self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<(), CoreError> {
        self.conn.execute(
            "INSERT INTO kv(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// 缺失或整段损坏时回落到默认设置；个别项读不出或不合法时只回落那几项（`Settings::from_json_lenient`）。
    pub fn load_settings(&self) -> Settings {
        match self.kv_get("settings") {
            Ok(Some(raw)) => match Settings::from_json_lenient(&raw) {
                Ok((s, dropped)) => {
                    if !dropped.is_empty() {
                        log::warn!(
                            "已保存的设置有 {} 项读不出或不合法，这几项用默认值：{}",
                            dropped.len(),
                            dropped.join(", ")
                        );
                    }
                    s
                }
                Err(e) => {
                    log::warn!("设置解析失败，使用默认值：{e}");
                    Settings::default()
                }
            },
            Ok(None) => Settings::default(),
            Err(e) => {
                log::warn!("读取设置失败，使用默认值：{e}");
                Settings::default()
            }
        }
    }

    pub fn save_settings(&self, s: &Settings) -> Result<(), CoreError> {
        self.kv_set("settings", &serde_json::to_string(s)?)
    }

    /// 按 hash 去重入库；新增时按 `max_records` 删最旧的非置顶记录，返回被删 id。
    pub fn insert_or_merge(
        &mut self,
        r: &NewRecord,
        now_ms: i64,
        max_records: u32,
    ) -> Result<(Upsert, Vec<i64>), CoreError> {
        let tx = self.conn.transaction()?;
        let hit: Option<i64> = tx
            .query_row(
                "SELECT id FROM records WHERE hash = ?1",
                [&r.hash[..]],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(id) = hit {
            tx.execute(
                "UPDATE records SET uses = uses + 1, deleted_at = NULL,
                   copied_at = CASE WHEN pinned = 1 THEN copied_at ELSE ?1 END WHERE id = ?2",
                params![now_ms, id],
            )?;
            tx.commit()?;
            return Ok((Upsert::Merged(id), Vec::new()));
        }

        if let Some(app) = &r.app {
            tx.execute(
                "INSERT INTO apps(id, name) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name",
                [&app.id, &app.name],
            )?;
        }
        let paths = if r.paths.is_empty() {
            None
        } else {
            let v: Vec<String> = r.paths.iter().map(|p| p.display().to_string()).collect();
            Some(serde_json::to_string(&v)?)
        };
        let meta = r.meta.as_ref().map(serde_json::to_string).transpose()?;
        tx.execute(
            "INSERT INTO records(hash, kind, title, text, paths, link_title, lang, size, meta,
               search_text, app_id, copied_at, py_text) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                &r.hash[..],
                r.kind.as_str(),
                r.title,
                r.text,
                paths,
                r.link_title,
                r.lang,
                r.size as i64,
                meta,
                r.search_text,
                r.app.as_ref().map(|a| &a.id),
                now_ms,
                pinyin::index_text(&r.search_text)
            ],
        )?;
        let id = tx.last_insert_rowid();

        let mut writes: Vec<(PathBuf, &[u8])> = Vec::new();
        for (n, f) in r.flavors.iter().enumerate() {
            let (data, file) = if f.data.len() <= INLINE_MAX {
                (Some(&f.data[..]), None)
            } else {
                let rel = format!("blobs/{id}/{n}");
                writes.push((self.dir.join(&rel), &f.data));
                (None, Some(rel))
            };
            tx.execute(
                "INSERT INTO flavors(record_id, item, format, data, file) VALUES (?1,?2,?3,?4,?5)",
                params![id, i64::from(f.item), f.format, data, file],
            )?;
        }
        if let Some(b) = &r.bitmap {
            writes.push((self.dir.join("images").join(format!("{id}.png")), b));
        }

        let removed = ids_in(
            &tx,
            &format!(
                "SELECT id FROM records WHERE {LIVE_UNPINNED}
                 ORDER BY copied_at DESC, id DESC LIMIT -1 OFFSET ?1"
            ),
            &[&max_records],
        )?;
        delete_rows(&tx, &removed)?;

        // 文件最后写，失败时回滚事务并清掉已写的文件
        let written = write_all(&writes).and_then(|()| tx.commit().map_err(CoreError::from));
        if let Err(e) = written {
            self.delete_files(id);
            return Err(e);
        }
        for &rid in &removed {
            self.delete_files(rid);
        }
        Ok((Upsert::Added(id), removed))
    }

    pub fn list_summaries(&self) -> Result<Vec<RecordSummary>, CoreError> {
        let sql = format!(
            "{SUMMARY_SQL} WHERE r.deleted_at IS NULL ORDER BY r.copied_at DESC, r.id DESC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| self.summary_row(row))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// 不区分是否已软删除。
    pub fn summary(&self, id: i64) -> Result<Option<RecordSummary>, CoreError> {
        Ok(self
            .conn
            .query_row(&format!("{SUMMARY_SQL} WHERE r.id = ?1"), [id], |row| {
                self.summary_row(row)
            })
            .optional()?)
    }

    /// 未软删除的记录摘要；已删除或不存在时为 None（发给前端前用它，避免已删记录复现）。
    pub fn live_summary(&self, id: i64) -> Result<Option<RecordSummary>, CoreError> {
        Ok(self
            .conn
            .query_row(
                &format!("{SUMMARY_SQL} WHERE r.id = ?1 AND r.deleted_at IS NULL"),
                [id],
                |row| self.summary_row(row),
            )
            .optional()?)
    }

    pub fn detail(&self, id: i64) -> Result<Option<RecordDetail>, CoreError> {
        let Some(summary) = self.summary(id)? else {
            return Ok(None);
        };
        let (text, link_title, paths, meta, ocr_text): (
            Option<String>,
            _,
            Option<String>,
            Option<String>,
            _,
        ) = self.conn.query_row(
            "SELECT text, link_title, paths, meta, ocr_text FROM records WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        let meta = parse_meta(meta);
        let (text, truncated) = match text {
            Some(mut t) if t.len() > DETAIL_TEXT_MAX => {
                let mut end = DETAIL_TEXT_MAX;
                while !t.is_char_boundary(end) {
                    end -= 1;
                }
                t.truncate(end);
                (Some(t), true)
            }
            t => (t, false),
        };
        let paths: Vec<String> = match paths {
            Some(p) => serde_json::from_str(&p)?,
            None => Vec::new(),
        };
        let files: Vec<FileEntry> = paths
            .into_iter()
            .map(|path| {
                let p = Path::new(&path);
                FileEntry {
                    name: p
                        .file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    ext: p
                        .extension()
                        .map_or_else(String::new, |e| e.to_string_lossy().to_lowercase()),
                    size: std::fs::metadata(p).map_or(0, |m| m.len()),
                    exists: p.exists(),
                    path,
                }
            })
            .collect();
        let dim = |k: &str| {
            meta.as_ref()
                .and_then(|m| m.get(k)?.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(0)
        };
        let image =
            existing(self.dir.join("images").join(format!("{id}.png"))).map(|src| ImageInfo {
                src,
                w: dim("w"),
                h: dim("h"),
            });
        let video = match files.first() {
            Some(f) if summary.kind == Kind::Video => Some(VideoInfo {
                src: f.exists.then(|| f.path.clone()),
                w: dim("w"),
                h: dim("h"),
                fps: meta
                    .as_ref()
                    .and_then(|m| m.get("fps")?.as_f64())
                    .unwrap_or(0.0),
                codec: meta
                    .as_ref()
                    .and_then(|m| m.get("codec")?.as_str())
                    .unwrap_or_default()
                    .to_owned(),
            }),
            _ => None,
        };
        Ok(Some(RecordDetail {
            summary,
            text,
            truncated,
            link_title,
            files,
            image,
            video,
            ocr_text,
        }))
    }

    /// 读回原始 flavor 数据（大块从 blobs 读）。
    pub fn flavors(&self, id: i64) -> Result<Vec<Flavor>, CoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT item, format, data, file FROM flavors WHERE record_id = ?1
             ORDER BY rowid",
        )?;
        let rows = stmt
            .query_map([id], |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(item, format, data, file)| {
                let data = match (data, file) {
                    (_, Some(f)) => std::fs::read(self.dir.join(f))?,
                    (d, None) => d.unwrap_or_default(),
                };
                Ok(Flavor { item, format, data })
            })
            .collect()
    }

    /// 不区分大小写的子串匹配，纯字母查询另按拼音匹配汉字：≥ 3 个字母按全拼 / 首字母搜全文（见 `search_sql`），
    /// 正好 2 个字母只按标题的首字母（`pinyin::find_initials`），在 Rust 里逐行判断。空查询返回全部未删除记录。
    /// 命中来自识别文字（标题不含关键词）时附带片段；靠拼音命中（字面不含查询）时，标题的拼音索引满足同一查询才用
    /// 标题里被匹配的原汉字作 `mark`，否则识别文字满足时用识别文字的（片段围绕它取），都不满足时没有 `mark`。
    pub fn search_hits(&self, q: &str) -> Result<Vec<SearchHit>, CoreError> {
        let q = q.to_lowercase();
        let (clause, extra) = search_sql(&q);
        let initials = pinyin::is_pinyin_query(&q) && q.len() == 2;
        // 第 3 列：字面命中时为识别文字（没有则空串），否则 NULL，instr 只算一次；
        // 第 4 列：非字面行的识别文字，只有 ≥ 3 个字母的拼音查询用来定位高亮，其余路径不取（2 个字母时不读全表的识别全文）
        let py_ocr = if extra.iter().any(|(k, _)| *k == ":py") {
            "ocr_text"
        } else {
            "NULL"
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT id, title, CASE WHEN instr(search_text, :q) > 0 THEN coalesce(ocr_text, '') END, {py_ocr}
             FROM records WHERE deleted_at IS NULL AND {clause} ORDER BY copied_at DESC, id DESC"
        ))?;
        let mut args: Vec<(&str, &dyn ToSql)> = vec![(":q", &q)];
        args.extend(extra.iter().map(|(k, v)| (*k, v as &dyn ToSql)));
        let rows = stmt.query_map(args.as_slice(), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut hits = vec![];
        for row in rows {
            let (id, title, lit_ocr, ocr) = row?;
            let (mut snip, mut mark) = (None, None);
            if let Some(t) = lit_ocr {
                if !title.to_lowercase().contains(&q) {
                    snip = snippet(&t, &q);
                }
            } else if initials {
                match pinyin::find_initials(&title, &q) {
                    Some(m) => mark = Some(m),
                    None => continue,
                }
            } else if pinyin::index_matches(&pinyin::index_text(&title), &q) {
                mark = pinyin::find(&title, &q);
            } else if let Some(t) =
                ocr.filter(|t| pinyin::index_matches(&pinyin::index_text(t), &q))
            {
                mark = pinyin::find(&t, &q);
                snip = mark.as_deref().and_then(|m| snippet(&t, m));
            }
            hits.push(SearchHit {
                id,
                snippet: snip,
                mark,
            });
        }
        Ok(hits)
    }

    /// 写识别结果；只有 `TEXT` 保存原文并把小写文本追加进 `search_text`。返回是否更新了未删除的记录。
    pub fn set_ocr(&self, id: i64, state: u8, text: Option<&str>) -> Result<bool, CoreError> {
        let text = text.filter(|_| state == ocr_state::TEXT);
        // 识别文字没有汉字（拼音索引为空）时 py_text 不变
        let py = text.map(pinyin::index_text).filter(|p| !p.is_empty());
        // 单条 UPDATE 即一个事务
        let n = self.conn.execute(
            "UPDATE records SET ocr_state = ?1, ocr_text = ?2,
               search_text = CASE WHEN ?3 IS NULL OR ocr_text IS NOT NULL THEN search_text ELSE search_text || char(10) || ?3 END,
               py_text = CASE WHEN ?5 IS NULL OR ocr_text IS NOT NULL THEN py_text ELSE coalesce(py_text, '') || char(10) || ?5 END
             WHERE id = ?4 AND deleted_at IS NULL",
            params![state, text, text.map(str::to_lowercase), id, py],
        )?;
        Ok(n > 0)
    }

    /// 把 `ids` 里仍待识别的未删除记录一条语句改成 `state`；返回真正改了的 id（顺序不定）。
    pub fn set_ocr_many(&self, ids: &[i64], state: u8) -> Result<Vec<i64>, CoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        // u8 直接拼进 SQL，id 走占位符；队列上限约 5000，低于 SQLite 变量上限 32766
        let marks = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "UPDATE records SET ocr_state = {state}
             WHERE ocr_state = {} AND deleted_at IS NULL AND id IN ({marks}) RETURNING id",
            ocr_state::PENDING
        );
        let args: Vec<&dyn ToSql> = ids.iter().map(|id| id as &dyn ToSql).collect();
        ids_in(&self.conn, &sql, &args)
    }

    /// 未删除记录的识别状态；已删除或不存在为 None。
    pub fn ocr_state(&self, id: i64) -> Result<Option<u8>, CoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT ocr_state FROM records WHERE id = ?1 AND deleted_at IS NULL",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// 处于 `states` 之一的未删除图片，最近复制的在前。
    pub fn ocr_pending_ids(&self, states: &[u8], limit: usize) -> Result<Vec<i64>, CoreError> {
        if states.is_empty() {
            return Ok(Vec::new());
        }
        // u8 直接拼进 SQL，无注入面
        let list = states
            .iter()
            .map(u8::to_string)
            .collect::<Vec<_>>()
            .join(",");
        ids_in(
            &self.conn,
            &format!(
                "SELECT id FROM records WHERE {} AND deleted_at IS NULL
                 AND ocr_state IN ({list}) ORDER BY copied_at DESC, id DESC LIMIT ?1",
                ocr_eligible_sql()
            ),
            &[&(limit as i64)],
        )
    }

    /// (已处理：待识别以外的状态 1–5, 未删除图片总数)
    pub fn ocr_progress(&self) -> Result<(u32, u32), CoreError> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT COALESCE(SUM(ocr_state BETWEEN 1 AND 5), 0), COUNT(*) FROM records
                 WHERE {} AND deleted_at IS NULL",
                ocr_eligible_sql()
            ),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    pub fn set_pinned(&self, id: i64, on: bool) -> Result<(), CoreError> {
        self.conn.execute(
            "UPDATE records SET pinned = ?1 WHERE id = ?2",
            params![on, id],
        )?;
        Ok(())
    }

    /// 粘贴后计数；`move_to_top` 时非置顶记录移到最前。
    pub fn bump_use(&self, id: i64, move_to_top: bool, now_ms: i64) -> Result<(), CoreError> {
        self.conn.execute(
            "UPDATE records SET uses = uses + 1,
               copied_at = CASE WHEN ?1 AND pinned = 0 THEN ?2 ELSE copied_at END WHERE id = ?3",
            params![move_to_top, now_ms, id],
        )?;
        Ok(())
    }

    pub fn soft_delete(&self, id: i64, now_ms: i64) -> Result<(), CoreError> {
        self.conn.execute(
            "UPDATE records SET deleted_at = ?1 WHERE id = ?2",
            params![now_ms, id],
        )?;
        Ok(())
    }

    pub fn undo_delete(&self, id: i64) -> Result<bool, CoreError> {
        let n = self.conn.execute(
            "UPDATE records SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
            [id],
        )?;
        Ok(n > 0)
    }

    /// 仅当记录仍是那次软删除（`deleted_at` 相同）时彻底删除。
    pub fn purge_if_deleted_at(&mut self, id: i64, deleted_at: i64) -> Result<bool, CoreError> {
        let n = self.conn.execute(
            "DELETE FROM records WHERE id = ?1 AND deleted_at = ?2",
            [id, deleted_at],
        )?;
        if n > 0 {
            self.delete_files(id);
        }
        Ok(n > 0)
    }

    pub fn purge_all_soft_deleted(&mut self) -> Result<Vec<i64>, CoreError> {
        self.delete_where("deleted_at IS NOT NULL", &[])
    }

    pub fn clear_unpinned(&mut self) -> Result<Vec<i64>, CoreError> {
        self.delete_where(LIVE_UNPINNED, &[])
    }

    pub fn count_unpinned(&self) -> Result<u32, CoreError> {
        Ok(self.conn.query_row(
            &format!("SELECT COUNT(*) FROM records WHERE {LIVE_UNPINNED}"),
            [],
            |r| r.get(0),
        )?)
    }

    /// 删除早于保留时长的非置顶记录；`None`（永久）不删。
    pub fn prune_retention(
        &mut self,
        retention_days: Option<u32>,
        now_ms: i64,
    ) -> Result<Vec<i64>, CoreError> {
        let Some(cutoff) = retention_cutoff(retention_days, now_ms) else {
            return Ok(Vec::new());
        };
        self.delete_where(&format!("{LIVE_UNPINNED} AND copied_at < ?1"), &[&cutoff])
    }

    /// 两条规则（保留时长、最多条数）会删除的非置顶记录数，取并集。
    pub fn prune_preview(
        &self,
        retention_days: Option<u32>,
        max_records: u32,
        now_ms: i64,
    ) -> Result<u32, CoreError> {
        let cutoff = retention_cutoff(retention_days, now_ms).unwrap_or(i64::MIN);
        Ok(self.conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM (
                   SELECT id FROM records WHERE {LIVE_UNPINNED} AND copied_at < ?1
                   UNION
                   SELECT id FROM (SELECT id FROM records WHERE {LIVE_UNPINNED}
                     ORDER BY copied_at DESC, id DESC LIMIT -1 OFFSET ?2))"
            ),
            params![cutoff, max_records],
            |r| r.get(0),
        )?)
    }

    pub fn set_meta(&self, id: i64, meta: &Value) -> Result<(), CoreError> {
        self.conn.execute(
            "UPDATE records SET meta = ?1 WHERE id = ?2",
            params![serde_json::to_string(meta)?, id],
        )?;
        Ok(())
    }

    pub fn set_app_icon(&self, app_id: &str, path: &Path) -> Result<(), CoreError> {
        self.conn.execute(
            "UPDATE apps SET icon_path = ?1 WHERE id = ?2",
            params![path.display().to_string(), app_id],
        )?;
        Ok(())
    }

    /// 该应用的未删除记录 id（补全图标后逐条刷新前端）。
    pub fn ids_for_app(&self, app_id: &str) -> Result<Vec<i64>, CoreError> {
        ids_in(
            &self.conn,
            "SELECT id FROM records WHERE app_id = ?1 AND deleted_at IS NULL",
            &[&app_id],
        )
    }

    /// 按最多条数删最旧的非置顶记录（设置里调小上限时）。
    pub fn prune_capacity(&mut self, max_records: u32) -> Result<Vec<i64>, CoreError> {
        self.delete_where(
            &format!(
                "id IN (SELECT id FROM records WHERE {LIVE_UNPINNED}
                 ORDER BY copied_at DESC, id DESC LIMIT -1 OFFSET ?1)"
            ),
            &[&max_records],
        )
    }

    fn summary_row(&self, row: &Row) -> rusqlite::Result<RecordSummary> {
        let id: i64 = row.get(0)?;
        let kind: String = row.get(1)?;
        let kind = Kind::parse(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(1, Type::Text, "未知记录类型".into())
        })?;
        let icon: Option<String> = row.get(4)?;
        let app = row
            .get::<_, Option<String>>(3)?
            .map(|name| AppSummary { name, icon });
        let meta = parse_meta(row.get(10)?);
        Ok(RecordSummary {
            id,
            kind,
            title: row.get(2)?,
            app,
            copied_at: row.get(5)?,
            uses: row.get(6)?,
            pinned: row.get(7)?,
            size: row.get::<_, i64>(8)? as u64,
            lang: row.get(9)?,
            thumb: existing(self.dir.join("thumbs").join(format!("{id}.png"))),
            // 平台读不出时长时写 0：未知值不显示，这里统一当作没有
            duration_sec: meta
                .as_ref()
                .and_then(|m| m.get("duration")?.as_f64())
                .filter(|&d| d > 0.0),
            ocr_state: row.get(11)?,
        })
    }

    /// 所有多行删除的入口：一个事务删行，提交后删文件。
    fn delete_where(&mut self, cond: &str, args: &[&dyn ToSql]) -> Result<Vec<i64>, CoreError> {
        let tx = self.conn.transaction()?;
        let ids = ids_in(&tx, &format!("SELECT id FROM records WHERE {cond}"), args)?;
        delete_rows(&tx, &ids)?;
        tx.commit()?;
        for &id in &ids {
            self.delete_files(id);
        }
        Ok(ids)
    }

    fn delete_files(&self, id: i64) {
        let blobs = self.dir.join("blobs").join(id.to_string());
        let mut results = vec![std::fs::remove_dir_all(blobs)];
        for sub in ["images", "thumbs"] {
            results.push(std::fs::remove_file(
                self.dir.join(sub).join(format!("{id}.png")),
            ));
        }
        for e in results.into_iter().filter_map(Result::err) {
            if e.kind() != ErrorKind::NotFound {
                log::debug!("删除记录 {id} 的文件失败：{e}");
            }
        }
    }
}

/// 搜索的 WHERE 片段与它另用的命名参数（`q` 已小写；`:q` 即 `q` 本身，由 `search_hits` 统一绑定）。
/// ≥ 3 个字符走 trigram 索引（`:lit` 是 `MATCH` 的短语查询，双引号加倍转义），子串语义与 instr 相同；
/// 1–2 个字符 trigram 无法索引，仍顺序扫描 `search_text`（短词命中多，instr 遇首个命中即停，并不慢）。
/// ≥ 3 个字母时再并上 `fts_py` 的拼音匹配（`:py`，见 `pinyin::match_expr`）；
/// 正好 2 个字母的拼音查询只按标题首字母，取全部未删除行（`1`），由 `search_hits` 在 Rust 里判断。
fn search_sql(q: &str) -> (&'static str, Vec<(&'static str, String)>) {
    if q.chars().count() < 3 {
        let clause = if pinyin::is_pinyin_query(q) {
            "1"
        } else {
            "instr(search_text, :q) > 0"
        };
        return (clause, vec![]);
    }
    let lit = (":lit", format!("\"{}\"", q.replace('"', "\"\"")));
    match pinyin::match_expr(q) {
        None => (
            "id IN (SELECT rowid FROM fts WHERE search_text MATCH :lit)",
            vec![lit],
        ),
        Some(py) => (
            "(id IN (SELECT rowid FROM fts WHERE search_text MATCH :lit) OR id IN (SELECT rowid FROM fts_py WHERE py_text MATCH :py))",
            vec![lit, (":py", py)],
        ),
    }
}

/// 可识别文字的记录：图片记录，或文件列表里有图片文件的文件记录。
/// `paths` 是 JSON 字符串数组，每个路径以 `.ext"` 结束，用 LIKE 判断扩展名（文件记录只有几千行，不需要索引）。
fn ocr_eligible_sql() -> String {
    let exts = crate::detect::IMAGE_EXTS
        .iter()
        .map(|e| format!("lower(paths) LIKE '%.{e}\"%'"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("(kind = 'image' OR (kind = 'file' AND ({exts})))")
}

/// 关键词（已小写）在 `text` 中首次出现处前后各取 12 个字符，保留原文大小写；被截断的一侧加 `…`。
/// ponytail: 按字符小写定位，与整串 `to_lowercase` 只在希腊词尾 Σ 这类上下文相关规则上不同，此时不给片段。
fn snippet(text: &str, q: &str) -> Option<String> {
    const CTX: usize = 12;
    let chars: Vec<char> = text.chars().collect();
    let mut lower = String::new();
    let mut starts = Vec::with_capacity(chars.len());
    for c in &chars {
        starts.push(lower.len());
        lower.extend(c.to_lowercase());
    }
    let at = lower.find(q)?;
    let first = starts.partition_point(|&b| b <= at) - 1;
    let end = starts.partition_point(|&b| b < at + q.len());
    let (from, to) = (first.saturating_sub(CTX), (end + CTX).min(chars.len()));
    let mut out = String::new();
    if from > 0 {
        out.push('…');
    }
    out.extend(&chars[from..to]);
    if to < chars.len() {
        out.push('…');
    }
    Some(out)
}

fn ids_in(conn: &Connection, sql: &str, args: &[&dyn ToSql]) -> Result<Vec<i64>, CoreError> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(args, |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn delete_rows(conn: &Connection, ids: &[i64]) -> Result<(), CoreError> {
    let mut stmt = conn.prepare("DELETE FROM records WHERE id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

fn write_all(writes: &[(PathBuf, &[u8])]) -> Result<(), CoreError> {
    for (path, data) in writes {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
    }
    Ok(())
}

fn retention_cutoff(days: Option<u32>, now_ms: i64) -> Option<i64> {
    days.map(|d| now_ms - i64::from(d) * DAY_MS)
}

/// 损坏的 meta 视为无。
fn parse_meta(raw: Option<String>) -> Option<Value> {
    raw.and_then(|m| serde_json::from_str(&m).ok())
}

fn existing(p: PathBuf) -> Option<String> {
    p.exists().then(|| p.display().to_string())
}

fn make_dirs(dir: &Path) -> std::io::Result<()> {
    for sub in SUBDIRS {
        std::fs::create_dir_all(dir.join(sub))?;
    }
    Ok(())
}

/// 依次执行没跑过的迁移，每个版本一个事务；返回升到版本 6 时补建拼音索引的行数（没跑迁移 6 时为 0）。
fn migrate(conn: &mut Connection) -> Result<usize, CoreError> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let mut filled = 0;
    for (v, sql) in (1u32..).zip(MIGRATIONS).skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        // 迁移 6 新增 py_text：旧行在同一个事务里补建，中途退出整体回滚，下次启动从版本 5 重来，不会留下 NULL 行
        if v == 6 {
            filled = backfill_pinyin(&tx)?;
        }
        tx.pragma_update(None, "user_version", v)?;
        tx.commit()?;
    }
    Ok(filled)
}

/// 给升级前的旧记录补 `py_text`：正文前 `PY_MAX_CHARS` 个字符的索引，识别文字有汉字时再另起一行接上它的索引
/// （与新记录 `set_ocr` 的结果一致，识别文字常在长正文之后，超出上限也能搜到）。逐行读写，不把整表收进内存；返回补了多少行。
fn backfill_pinyin(conn: &Connection) -> Result<usize, CoreError> {
    let mut read = conn.prepare(
        "SELECT id, substr(search_text, 1, ?1), ocr_text FROM records WHERE py_text IS NULL",
    )?;
    let mut write = conn.prepare("UPDATE records SET py_text = ?1 WHERE id = ?2")?;
    let mut rows = read.query([pinyin::PY_MAX_CHARS as i64])?;
    let mut n = 0;
    while let Some(r) = rows.next()? {
        let (id, text, ocr): (i64, String, Option<String>) = (r.get(0)?, r.get(1)?, r.get(2)?);
        let mut py = pinyin::index_text(&text);
        if let Some(o) = ocr
            .map(|o| pinyin::index_text(&o))
            .filter(|o| !o.is_empty())
        {
            py.push('\n');
            py.push_str(&o);
        }
        write.execute(params![py, id])?;
        n += 1;
    }
    if n > 0 {
        log::info!("拼音索引补建 {n} 条");
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    pub(crate) fn tmp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "xpaste-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, SeqCst)
        ))
    }

    #[test]
    fn open_creates_schema_and_dirs() {
        let dir = tmp();
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 6);
        for sub in ["blobs", "images", "thumbs", "icons"] {
            assert!(dir.join(sub).is_dir());
        }
        let tables: Vec<String> = s
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE 'fts%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(tables, ["apps", "flavors", "kv", "records"]);
        // 搜索索引是外部内容表：只有索引影子表，没有正文副本
        let shadow: Vec<String> = s
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE name LIKE 'fts%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            shadow.contains(&"fts".to_string()) && !shadow.contains(&"fts_content".to_string()),
            "{shadow:?}"
        );
    }

    #[test]
    fn color_rows_become_text_after_migration() {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.pragma_update(None, "user_version", 1).unwrap();
        old.execute(
            "INSERT INTO records (hash, kind, title, text, copied_at, size, search_text) VALUES (x'01', 'color', '#6F8BFF', ' #6f8bff\n', 1, 9, '#6f8bff')",
            [],
        )
        .unwrap();
        drop(old);
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 6);
        let row: (String, String) = s
            .conn
            .query_row("SELECT kind, title FROM records", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, ("text".into(), "#6f8bff".into()));
        assert_eq!(s.list_summaries().unwrap()[0].kind, Kind::Text);
    }

    #[test]
    fn migration_4_adds_ocr_columns_and_opens_old_db() {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.pragma_update(None, "user_version", 3).unwrap();
        old.execute(
            "INSERT INTO records (hash, kind, title, copied_at, size, search_text) VALUES (x'03', 'image', '截图', 1, 9, '截图')",
            [],
        )
        .unwrap();
        drop(old);
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 6);
        let l = s.list_summaries().unwrap();
        assert_eq!(
            (l[0].kind, l[0].ocr_state),
            (Kind::Image, ocr_state::PENDING)
        );
        assert_eq!(s.detail(l[0].id).unwrap().unwrap().ocr_text, None);
    }

    #[test]
    fn reopen_does_not_rerun_migrations() {
        let dir = tmp();
        drop(Store::open(&dir).unwrap());
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 6);
    }

    #[test]
    fn migration_5_builds_search_index_for_existing_rows() {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        for m in &MIGRATIONS[..4] {
            old.execute_batch(m).unwrap();
        }
        old.pragma_update(None, "user_version", 4).unwrap();
        old.execute(
            "INSERT INTO records (hash, kind, title, text, copied_at, size, search_text) VALUES (x'05', 'text', '旧记录', 'x', 1, 1, '季度预算 budget 2026')",
            [],
        )
        .unwrap();
        drop(old);
        let s = Store::open(&dir).unwrap();
        // ≥ 3 字走索引，必须在重建后能命中
        assert_eq!(ids(&s, "budget").len(), 1);
        assert_eq!(ids(&s, "季度预算").len(), 1);
        assert_eq!(ids(&s, "预算").len(), 1);
    }

    #[test]
    fn search_index_follows_insert_update_and_delete() {
        let mut s = mem();
        let a = added(&mut s, &rec("Quarterly Budget 报告"), 1);
        let img = added(&mut s, &rec_image(7), 2);
        assert_eq!(ids(&s, "budget 报"), [a]);
        assert_eq!(ids(&s, "rly bud"), [a]);
        assert!(ids(&s, "预算").is_empty());
        // 识别文字追加进 search_text：触发器同步索引
        s.set_ocr(img, ocr_state::TEXT, Some("季度预算 2026"))
            .unwrap();
        assert_eq!(ids(&s, "季度预算"), [img]);
        assert_eq!(ids(&s, "2026"), [img]);
        // 软删除不改索引但被 deleted_at 过滤；硬删除后索引条目消失
        s.soft_delete(img, 3).unwrap();
        assert!(ids(&s, "季度预算").is_empty());
        assert!(s.purge_if_deleted_at(img, 3).unwrap());
        let n: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM fts WHERE search_text MATCH '\"季度预算\"'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn insert_writes_py_text() {
        let mut s = mem();
        let id = added(&mut s, &rec("周四会议纪要"), 1);
        let py = py_text(&s, id).unwrap();
        assert!(py.contains("'hui'yi") && py.contains("HYJY"), "{py:?}");
        assert_eq!(fts_py_count(&s.conn, "\"'hui'yi\""), 1);
        assert_eq!(fts_py_count(&s.conn, "\"HYJY\""), 1);
        // 区分大小写：小写的首字母不命中大写的首字母行
        assert_eq!(fts_py_count(&s.conn, "\"hyjy\""), 0);
    }

    #[test]
    fn set_ocr_appends_pinyin() {
        let mut s = mem();
        let img = added(&mut s, &rec_image(7), 1);
        s.set_ocr(img, ocr_state::TEXT, Some("季度预算")).unwrap();
        let once = py_text(&s, img).unwrap();
        assert!(once.contains("'ji'du") && once.contains("JDYS"), "{once:?}");
        // 第二次识别与 search_text 同规则：不再追加
        s.set_ocr(img, ocr_state::TEXT, Some("季度预算")).unwrap();
        assert_eq!(py_text(&s, img).unwrap(), once);
    }

    #[test]
    fn set_ocr_without_han_keeps_py_text() {
        let mut s = mem();
        let img = added(&mut s, &rec_image(7), 1);
        let before = py_text(&s, img);
        s.set_ocr(img, ocr_state::TEXT, Some("Hello World 2026"))
            .unwrap();
        assert_eq!(py_text(&s, img), before);
    }

    /// 版本 5 的旧库，含两条还没有 py_text 的记录。
    fn v5_db_with_old_record() -> PathBuf {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        for m in &MIGRATIONS[..5] {
            old.execute_batch(m).unwrap();
        }
        old.pragma_update(None, "user_version", 5).unwrap();
        old.execute(
            "INSERT INTO records (hash, kind, title, text, copied_at, size, search_text) VALUES
               (x'06', 'text', '旧记录', 'x', 1, 1, '季度预算 budget'), (x'08', 'text', '会议', 'y', 2, 1, '会议')",
            [],
        )
        .unwrap();
        dir
    }

    #[test]
    fn backfill_is_part_of_migration() {
        let dir = v5_db_with_old_record();
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 6);
        let nulls: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM records WHERE py_text IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 0);
        assert_eq!(fts_py_count(&s.conn, "\"'ji'du\""), 1);
        assert_eq!(fts_py_count(&s.conn, "\"JDYS\""), 1);
        assert_eq!(fts_py_count(&s.conn, "\"'hui'yi\""), 1);
        drop(s);
        // 已是版本 6：即使人为清空 py_text，再次迁移也不会补建（补建只在升到版本 6 的那个事务里执行）
        let mut conn = Connection::open(dir.join("xpaste.db")).unwrap();
        conn.execute("UPDATE records SET py_text = NULL", [])
            .unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), 0);
        let nulls: i64 = conn
            .query_row(
                "SELECT count(*) FROM records WHERE py_text IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(nulls, 2);
    }

    #[test]
    fn upgrade_to_6_reports_backfilled_rows() {
        let dir = v5_db_with_old_record();
        let mut conn = Connection::open(dir.join("xpaste.db")).unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), 2);
    }

    #[test]
    fn backfill_indexes_ocr_text_separately() {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        for m in &MIGRATIONS[..5] {
            old.execute_batch(m).unwrap();
        }
        old.pragma_update(None, "user_version", 5).unwrap();
        // 识别文字接在 6000 字正文之后，超出 PY_MAX_CHARS：要单独从 ocr_text 补建
        let st = format!("{}\n季度预算", "一".repeat(6000));
        old.execute(
            "INSERT INTO records (hash, kind, title, paths, copied_at, size, search_text, ocr_text, ocr_state)
             VALUES (x'07', 'file', 'a.png', '[\"/a.png\"]', 1, 1, ?1, '季度预算', 2)",
            [&st],
        )
        .unwrap();
        drop(old);
        let s = Store::open(&dir).unwrap();
        assert_eq!(ids(&s, "jidu").len(), 1);
    }

    #[test]
    fn fts_py_follows_delete() {
        let mut s = mem();
        let id = added(&mut s, &rec("周四会议纪要"), 1);
        assert_eq!(fts_py_count(&s.conn, "\"'hui'yi\""), 1);
        s.soft_delete(id, 2).unwrap();
        assert!(s.purge_if_deleted_at(id, 2).unwrap());
        assert_eq!(fts_py_count(&s.conn, "\"'hui'yi\""), 0);
    }

    #[test]
    fn search_quotes_and_short_queries() {
        let mut s = mem();
        let a = added(&mut s, &rec("say \"hi\" now"), 1);
        assert_eq!(ids(&s, "\"hi\""), [a]);
        assert_eq!(ids(&s, "y \"h"), [a]);
        assert_eq!(ids(&s, "hi"), [a]);
        assert_eq!(ids(&s, "H"), [a]);
        // 两个字母的拼音查询不拼 SQL 拼音子句，取全部行，标题首字母在 search_hits 里用 Rust 判断
        assert_eq!(search_sql("ab"), ("1", vec![]));
        assert_eq!(search_sql("a1"), ("instr(search_text, :q) > 0", vec![]));
        assert_eq!(
            search_sql("预算书"),
            (
                "id IN (SELECT rowid FROM fts WHERE search_text MATCH :lit)",
                vec![(":lit", "\"预算书\"".to_string())]
            )
        );
        let (clause, args) = search_sql("huiyi");
        assert!(clause.contains("MATCH :lit") && clause.contains("MATCH :py"));
        assert_eq!(
            args,
            [
                (":lit", "\"huiyi\"".to_string()),
                (":py", pinyin::match_expr("huiyi").unwrap())
            ]
        );
    }

    #[test]
    fn titles_become_searchable_after_migration() {
        let dir = tmp();
        make_dirs(&dir).unwrap();
        let old = Connection::open(dir.join("xpaste.db")).unwrap();
        old.execute_batch(MIGRATIONS[0]).unwrap();
        old.pragma_update(None, "user_version", 2).unwrap();
        old.execute(
            "INSERT INTO records (hash, kind, title, copied_at, size, search_text) VALUES (x'02', 'image', '截图 2026-10-20 03.20.png', 1, 9, 'ishot')",
            [],
        )
        .unwrap();
        drop(old);
        let s = Store::open(&dir).unwrap();
        assert_eq!(ids(&s, "03.20").len(), 1);
        assert_eq!(ids(&s, "iShot").len(), 1);
    }

    #[test]
    fn settings_roundtrip() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        let cfg = Settings {
            max_records: 1000,
            ..Settings::default()
        };
        s.save_settings(&cfg).unwrap();
        assert_eq!(s.load_settings(), cfg);
    }

    #[test]
    fn missing_settings_are_default() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        assert_eq!(s.load_settings(), Settings::default());
    }

    #[test]
    fn corrupt_settings_fall_back_to_default() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        s.kv_set("settings", "{not json").unwrap();
        assert_eq!(s.load_settings(), Settings::default());
    }

    #[test]
    fn invalid_settings_fall_back_to_default() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        s.kv_set("settings", r#"{"maxRecords":0}"#).unwrap();
        assert_eq!(s.load_settings(), Settings::default());
    }

    #[test]
    fn unreadable_field_falls_back_alone() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        // 新版本加的取值（降级回旧版）、类型不对、未通过校验：只回落那一项
        s.kv_set(
            "settings",
            r#"{"maxRecords":1000,"recQuality":"lossless","recFps":120,"retentionDays":"x","hotkey":"Alt+Space"}"#,
        )
        .unwrap();
        let cfg = s.load_settings();
        let d = Settings::default();
        assert_eq!(cfg.max_records, 1000);
        assert_eq!(cfg.hotkey, "Alt+Space");
        assert_eq!(cfg.rec_quality, d.rec_quality);
        assert_eq!(cfg.rec_fps, d.rec_fps);
        assert_eq!(cfg.retention_days, d.retention_days);
    }

    #[test]
    fn old_panel_hotkey_on_shot_default_keeps_settings() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        s.kv_set("settings", r#"{"hotkey":"Alt+Shift+A","maxRecords":1000}"#)
            .unwrap();
        let cfg = s.load_settings();
        assert_eq!(cfg.hotkey, "Alt+Shift+A");
        assert_eq!(cfg.shot_hotkey, "", "撞上截图默认值时停用截图快捷键");
        assert_eq!(cfg.max_records, 1000, "其余设置保留，不回落到默认值");
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        let r = s.conn.execute(
            "INSERT INTO flavors(record_id,item,format,data) VALUES (999,0,'x',x'00')",
            [],
        );
        assert!(r.is_err());
    }

    use crate::capture::build_record;
    use crate::{
        AppInfo, Flavor, Kind, NewRecord, Snapshot, VideoInfo, VideoMeta, detect, ocr_state,
    };

    fn mem() -> Store {
        Store::open_in_memory(&tmp()).unwrap()
    }
    /// `fts_py` 里匹配 FTS 查询 `m` 的行数。
    fn fts_py_count(conn: &Connection, m: &str) -> i64 {
        conn.query_row(
            "SELECT count(*) FROM fts_py WHERE py_text MATCH ?1",
            [m],
            |r| r.get(0),
        )
        .unwrap()
    }
    fn py_text(s: &Store, id: i64) -> Option<String> {
        s.conn
            .query_row("SELECT py_text FROM records WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    }
    /// `search_hits` 命中的 id，顺序不变。
    fn ids(s: &Store, q: &str) -> Vec<i64> {
        s.search_hits(q)
            .unwrap()
            .into_iter()
            .map(|h| h.id)
            .collect()
    }
    fn rec_of(snap: Snapshot) -> NewRecord {
        let kind = detect::kind(&snap);
        build_record(snap, kind, 0)
    }
    fn rec(t: &str) -> NewRecord {
        rec_of(Snapshot {
            text: Some(t.into()),
            ..Default::default()
        })
    }
    fn rec_image(seed: u8) -> NewRecord {
        rec_of(Snapshot {
            bitmap: Some(vec![0x89, b'P', b'N', b'G', seed]),
            ..Default::default()
        })
    }
    fn added(s: &mut Store, r: &NewRecord, at: i64) -> i64 {
        match s.insert_or_merge(r, at, 500).unwrap().0 {
            Upsert::Added(id) => id,
            Upsert::Merged(_) => panic!("expected new record"),
        }
    }
    fn rec_files(p: &[&str]) -> NewRecord {
        rec_of(Snapshot {
            file_paths: p.iter().map(Into::into).collect(),
            ..Default::default()
        })
    }

    #[test]
    fn detail_builds_video_info_from_meta_and_file() {
        let mut s = mem();
        let f = s.dir.join("clip.mp4");
        std::fs::write(&f, b"x").unwrap();
        let p = f.to_string_lossy().into_owned();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec_files(&[&p]), 1, 500).unwrap() else {
            panic!()
        };
        let m = VideoMeta {
            duration_sec: 83.0,
            w: 1920,
            h: 1080,
            fps: 60.0,
            codec: "H.264".into(),
        };
        s.set_meta(id, &serde_json::to_value(&m).unwrap()).unwrap();
        let d = s.detail(id).unwrap().unwrap();
        assert_eq!(d.summary.duration_sec, Some(83.0));
        assert_eq!(
            d.video,
            Some(VideoInfo {
                src: Some(p),
                w: 1920,
                h: 1080,
                fps: 60.0,
                codec: "H.264".into()
            })
        );
        std::fs::remove_file(&f).unwrap();
        assert_eq!(s.detail(id).unwrap().unwrap().video.unwrap().src, None); // AC-M08-05
    }

    #[test]
    fn zero_duration_is_unknown() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s
            .insert_or_merge(&rec_files(&["/x/a.mp4"]), 1, 500)
            .unwrap()
        else {
            panic!()
        };
        s.set_meta(id, &serde_json::json!({ "duration": 0.0 }))
            .unwrap();
        assert_eq!(s.detail(id).unwrap().unwrap().summary.duration_sec, None);
    }

    #[test]
    fn video_without_meta_is_zeroed_and_other_kinds_have_none() {
        let mut s = mem();
        let (Upsert::Added(v), _) = s
            .insert_or_merge(&rec_files(&["/definitely/missing.mov"]), 1, 500)
            .unwrap()
        else {
            panic!()
        };
        let d = s.detail(v).unwrap().unwrap();
        assert_eq!(
            d.video,
            Some(VideoInfo {
                src: None,
                w: 0,
                h: 0,
                fps: 0.0,
                codec: String::new()
            })
        );
        assert_eq!(d.summary.duration_sec, None);
        let (Upsert::Added(t), _) = s.insert_or_merge(&rec("a"), 2, 500).unwrap() else {
            panic!()
        };
        assert_eq!(s.detail(t).unwrap().unwrap().video, None);
    }

    #[test]
    fn insert_then_list() {
        let mut s = mem();
        s.insert_or_merge(&rec("a"), 1, 500).unwrap();
        let l = s.list_summaries().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].uses, 0);
        assert_eq!(l[0].kind, Kind::Text);
    }
    #[test]
    fn duplicate_merges_and_bumps() {
        let mut s = mem();
        let (u1, _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap();
        let (u2, _) = s.insert_or_merge(&rec("a"), 2, 500).unwrap();
        assert!(matches!(u1, Upsert::Added(_)));
        assert!(matches!(u2, Upsert::Merged(_)));
        let l = s.list_summaries().unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].uses, 1);
        assert_eq!(l[0].copied_at, 2);
    }
    #[test]
    fn pinned_duplicate_keeps_position() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.set_pinned(id, true).unwrap();
        s.insert_or_merge(&rec("a"), 9, 500).unwrap();
        let l = s.list_summaries().unwrap();
        assert_eq!(l[0].copied_at, 1);
        assert_eq!(l[0].uses, 1);
    }
    #[test]
    fn merge_restores_soft_deleted() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.soft_delete(id, 5).unwrap();
        assert!(s.list_summaries().unwrap().is_empty());
        s.insert_or_merge(&rec("a"), 6, 500).unwrap();
        assert_eq!(s.list_summaries().unwrap().len(), 1);
    }
    #[test]
    fn max_records_drops_oldest_unpinned() {
        let mut s = mem();
        for i in 0..3 {
            s.insert_or_merge(&rec(&format!("t{i}")), i, 500).unwrap();
        }
        let first = s.list_summaries().unwrap().last().unwrap().id;
        s.set_pinned(first, true).unwrap();
        // 上限只数非置顶：1 置顶 + 2 非置顶，再插入 new 后 limit 2 → 删最早的非置顶 1 条
        let (_, removed) = s.insert_or_merge(&rec("new"), 10, 2).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(s.list_summaries().unwrap().len(), 3);
        assert!(s.list_summaries().unwrap().iter().any(|r| r.id == first));
    }
    #[test]
    fn live_summary_hides_soft_deleted() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        assert_eq!(s.live_summary(id).unwrap().map(|r| r.id), Some(id));
        s.soft_delete(id, 2).unwrap();
        assert!(s.live_summary(id).unwrap().is_none());
        assert!(s.summary(id).unwrap().is_some());
    }
    #[test]
    fn prune_capacity_removes_oldest_unpinned() {
        let mut s = mem();
        for i in 0..5 {
            s.insert_or_merge(&rec(&format!("t{i}")), i, 500).unwrap();
        }
        let oldest: Vec<i64> = s.list_summaries().unwrap()[3..]
            .iter()
            .map(|r| r.id)
            .collect();
        let mut removed = s.prune_capacity(3).unwrap();
        removed.sort();
        let mut want = oldest;
        want.sort();
        assert_eq!(removed, want);
        assert_eq!(s.list_summaries().unwrap().len(), 3);
    }
    #[test]
    fn ids_for_app_lists_live_records_of_that_app() {
        let mut s = mem();
        let app = |id: &str| {
            Some(AppInfo {
                id: id.into(),
                name: id.into(),
            })
        };
        let mut a = rec("a");
        a.app = app("x");
        let mut b = rec("b");
        b.app = app("x");
        let mut c = rec("c");
        c.app = app("y");
        let ids: Vec<i64> = [a, b, c]
            .iter()
            .map(|r| match s.insert_or_merge(r, 1, 500).unwrap().0 {
                Upsert::Added(id) | Upsert::Merged(id) => id,
            })
            .collect();
        s.soft_delete(ids[1], 2).unwrap();
        assert_eq!(s.ids_for_app("x").unwrap(), [ids[0]]);
    }
    #[test]
    fn insert_is_atomic() {
        // 让 flavors 写入失败（例如 item 用负数触发 CHECK，或先断开外键目标），records 不应留下记录
        let mut s = mem();
        let mut r = rec("a");
        r.flavors.push(Flavor {
            item: u32::MAX,
            format: "x".into(),
            data: vec![],
        });
        // 实现时给 flavors.item 加 CHECK (item < 65536)，使该插入失败
        assert!(s.insert_or_merge(&r, 1, 500).is_err());
        assert!(s.list_summaries().unwrap().is_empty());
    }
    #[test]
    fn large_flavor_goes_to_blob_file() {
        let mut s = mem();
        let mut r = rec("a");
        r.flavors.push(Flavor {
            item: 0,
            format: "big".into(),
            data: vec![7; 1_048_577],
        });
        let (Upsert::Added(id), _) = s.insert_or_merge(&r, 1, 500).unwrap() else {
            panic!()
        };
        assert!(
            s.dir
                .join("blobs")
                .join(id.to_string())
                .read_dir()
                .unwrap()
                .count()
                >= 1
        );
        let back = s.flavors(id).unwrap();
        assert!(
            back.iter()
                .any(|f| f.format == "big" && f.data.len() == 1_048_577)
        );
    }
    #[test]
    fn soft_delete_undo_and_purge() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.set_pinned(id, true).unwrap();
        s.soft_delete(id, 100).unwrap();
        assert!(s.undo_delete(id).unwrap());
        assert!(s.list_summaries().unwrap()[0].pinned);
        s.soft_delete(id, 200).unwrap();
        assert!(!s.purge_if_deleted_at(id, 199).unwrap());
        assert!(s.purge_if_deleted_at(id, 200).unwrap());
        assert!(s.summary(id).unwrap().is_none());
    }
    #[test]
    fn startup_purges_leftover_soft_deletes() {
        let mut s = mem();
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.soft_delete(id, 1).unwrap();
        assert_eq!(s.purge_all_soft_deleted().unwrap(), vec![id]);
    }
    #[test]
    fn retention_prune_spares_pinned() {
        let mut s = mem();
        let day = 86_400_000;
        let now = 40 * day;
        let (Upsert::Added(old), _) = s.insert_or_merge(&rec("old"), 1, 500).unwrap() else {
            panic!()
        };
        let (Upsert::Added(old_pinned), _) = s.insert_or_merge(&rec("oldp"), 2, 500).unwrap()
        else {
            panic!()
        };
        s.set_pinned(old_pinned, true).unwrap();
        s.insert_or_merge(&rec("fresh"), now - day, 500).unwrap();
        assert_eq!(s.prune_preview(Some(30), 500, now).unwrap(), 1);
        assert_eq!(s.prune_retention(Some(30), now).unwrap(), vec![old]);
        assert_eq!(s.prune_retention(None, now).unwrap(), Vec::<i64>::new());
    }
    #[test]
    fn prune_preview_counts_only_unpinned_overflow() {
        let mut s = mem();
        for i in 0..5 {
            s.insert_or_merge(&rec(&format!("t{i}")), i, 500).unwrap();
        }
        let l = s.list_summaries().unwrap();
        s.set_pinned(l[4].id, true).unwrap();
        assert_eq!(s.prune_preview(None, 3, 100).unwrap(), 1);
    }
    #[test]
    fn clear_unpinned_keeps_pinned() {
        let mut s = mem();
        for i in 0..3 {
            s.insert_or_merge(&rec(&format!("t{i}")), i, 500).unwrap();
        }
        let id = s.list_summaries().unwrap()[0].id;
        s.set_pinned(id, true).unwrap();
        assert_eq!(s.count_unpinned().unwrap(), 2);
        assert_eq!(s.clear_unpinned().unwrap().len(), 2);
        assert_eq!(s.list_summaries().unwrap().len(), 1);
    }
    #[test]
    fn search_is_case_insensitive_substring() {
        let mut s = mem();
        s.insert_or_merge(&rec("Hello 会议"), 1, 500).unwrap();
        s.insert_or_merge(&rec("other"), 2, 500).unwrap();
        assert_eq!(ids(&s, "HELLO").len(), 1);
        assert_eq!(ids(&s, "会议").len(), 1);
        assert_eq!(ids(&s, "").len(), 2);
    }
    #[test]
    fn detail_truncates_long_text_and_checks_files() {
        let mut s = mem();
        let big = "x".repeat(300 * 1024);
        let (Upsert::Added(id), _) = s.insert_or_merge(&rec(&big), 1, 500).unwrap() else {
            panic!()
        };
        let d = s.detail(id).unwrap().unwrap();
        assert!(d.truncated);
        assert_eq!(d.text.unwrap().len(), 200 * 1024);
        let (Upsert::Added(fid), _) = s
            .insert_or_merge(&rec_files(&["/definitely/missing.txt"]), 2, 500)
            .unwrap()
        else {
            panic!()
        };
        let d = s.detail(fid).unwrap().unwrap();
        assert_eq!(d.files.len(), 1);
        assert!(!d.files[0].exists);
        assert_eq!(d.files[0].ext, "txt");
    }
    #[test]
    fn deleting_record_removes_its_files() {
        let mut s = mem();
        let mut r = rec("a");
        r.bitmap = Some(vec![1, 2, 3]);
        let (Upsert::Added(id), _) = s.insert_or_merge(&r, 1, 500).unwrap() else {
            panic!()
        };
        let img = s.dir.join("images").join(format!("{id}.png"));
        assert!(img.exists());
        s.soft_delete(id, 1).unwrap();
        s.purge_if_deleted_at(id, 1).unwrap();
        assert!(!img.exists());
    }
    #[test]
    fn flavors_keep_insertion_order() {
        let mut s = mem();
        let mut r = rec("a");
        r.flavors = ["b", "a"]
            .map(|f| Flavor {
                item: 0,
                format: f.into(),
                data: vec![],
            })
            .into();
        let (Upsert::Added(id), _) = s.insert_or_merge(&r, 1, 500).unwrap() else {
            panic!()
        };
        let got: Vec<String> = s
            .flavors(id)
            .unwrap()
            .into_iter()
            .map(|f| f.format)
            .collect();
        assert_eq!(got, ["b", "a"]);
    }
    #[test]
    fn ids_are_not_reused() {
        let mut s = mem();
        let (Upsert::Added(a), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.soft_delete(a, 1).unwrap();
        s.purge_if_deleted_at(a, 1).unwrap();
        let (Upsert::Added(b), _) = s.insert_or_merge(&rec("b"), 2, 500).unwrap() else {
            panic!()
        };
        assert!(b > a, "{b} <= {a}");
    }

    #[test]
    fn three_external_copies_count_two_uses() {
        // AC-M11-02 口径：首次复制生成记录不计使用，第 4 次外部复制才进入“常用”
        let mut s = mem();
        for t in 1..=3 {
            s.insert_or_merge(&rec("a"), t, 500).unwrap();
        }
        assert_eq!(s.list_summaries().unwrap()[0].uses, 2);
        s.insert_or_merge(&rec("a"), 4, 500).unwrap();
        assert_eq!(s.list_summaries().unwrap()[0].uses, 3);
    }

    #[test]
    fn bump_use_moves_only_unpinned_when_enabled() {
        let mut s = mem();
        let (Upsert::Added(a), _) = s.insert_or_merge(&rec("a"), 1, 500).unwrap() else {
            panic!()
        };
        s.insert_or_merge(&rec("b"), 2, 500).unwrap();
        // AC-M09-08：关闭“复制后移到顶部”，次数 +1 但位置不变
        s.bump_use(a, false, 9).unwrap();
        let l = s.list_summaries().unwrap();
        assert_eq!((l[1].id, l[1].uses, l[1].copied_at), (a, 1, 1));
        // M09 规则 5：开启时回到最前
        s.bump_use(a, true, 10).unwrap();
        let l = s.list_summaries().unwrap();
        assert_eq!((l[0].id, l[0].uses, l[0].copied_at), (a, 2, 10));
        // 置顶记录位置不变
        s.set_pinned(a, true).unwrap();
        s.bump_use(a, true, 20).unwrap();
        let l = s.list_summaries().unwrap();
        assert_eq!((l[0].id, l[0].uses, l[0].copied_at), (a, 3, 10));
    }

    #[test]
    fn set_ocr_appends_lowercased_text_to_search_text() {
        let mut s = mem();
        let id = added(&mut s, &rec_image(1), 1);
        assert!(s.set_ocr(id, ocr_state::TEXT, Some("Q3 季度预算")).unwrap());
        assert_eq!(ids(&s, "季度预算"), [id]);
        assert_eq!(ids(&s, "q3"), [id]);
        let d = s.detail(id).unwrap().unwrap();
        assert_eq!(
            (d.summary.ocr_state, d.ocr_text.as_deref()),
            (ocr_state::TEXT, Some("Q3 季度预算"))
        );

        let other = added(&mut s, &rec_image(2), 2);
        let before: String = s
            .conn
            .query_row(
                "SELECT search_text FROM records WHERE id = ?1",
                [other],
                |r| r.get(0),
            )
            .unwrap();
        assert!(s.set_ocr(other, ocr_state::EMPTY, Some("ignored")).unwrap());
        let after: String = s
            .conn
            .query_row(
                "SELECT search_text FROM records WHERE id = ?1",
                [other],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(before, after);
        let d = s.detail(other).unwrap().unwrap();
        assert_eq!((d.summary.ocr_state, d.ocr_text), (ocr_state::EMPTY, None));

        s.soft_delete(other, 3).unwrap();
        assert!(!s.set_ocr(other, ocr_state::TEXT, Some("x")).unwrap());
    }

    #[test]
    fn set_ocr_twice_appends_search_text_once() {
        let mut s = mem();
        let id = added(&mut s, &rec_image(1), 1);
        for _ in 0..2 {
            assert!(s.set_ocr(id, ocr_state::TEXT, Some("xyzocr")).unwrap());
        }
        let st: String = s
            .conn
            .query_row("SELECT search_text FROM records WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(st.matches("xyzocr").count(), 1);
    }

    #[test]
    fn search_by_pinyin_and_initials() {
        let mut s = mem();
        let meeting = added(&mut s, &rec("周四下午会议纪要"), 1);
        let bank = added(&mut s, &rec("招商银行 6225"), 2);
        let en = added(&mut s, &rec("budget"), 3);
        assert_eq!(ids(&s, "huiyi"), [meeting]);
        assert_eq!(ids(&s, "hy"), [meeting]);
        assert_eq!(ids(&s, "yinhang"), [bank]);
        assert_eq!(ids(&s, "budget"), [en]);
    }

    #[test]
    fn search_pinyin_is_case_insensitive() {
        let mut s = mem();
        let meeting = added(&mut s, &rec("周四下午会议纪要"), 1);
        assert_eq!(ids(&s, "HuiYi"), [meeting]);
    }

    #[test]
    fn pinyin_query_keeps_literal_hits() {
        let mut s = mem();
        let literal = added(&mut s, &rec("js 工具函数"), 1);
        let py = added(&mut s, &rec("技术方案"), 2);
        added(&mut s, &rec("周报\n本周进展\n技术债清理"), 3);
        assert_eq!(ids(&s, "js"), [py, literal]);
    }

    #[test]
    fn two_letter_pinyin_matches_title_initials_only() {
        let mut s = mem();
        let title = added(&mut s, &rec("周四会议纪要\n地点：三楼"), 1);
        added(&mut s, &rec("周报\n本周进展\n下周会议安排"), 2);
        assert_eq!(ids(&s, "hy"), [title]);
        let h = s.search_hits("hy").unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].mark.as_deref(), Some("会议"));
        assert_eq!(h[0].snippet, None);
    }

    #[test]
    fn search_full_pinyin_across_polyphones() {
        let mut s = mem();
        let bank = added(&mut s, &rec("招商银行 6225 8888"), 1);
        assert_eq!(ids(&s, "zhaoshangyinhang"), [bank]);
    }

    #[test]
    fn search_rejects_mid_syllable() {
        let mut s = mem();
        let sh = added(&mut s, &rec("上海"), 1);
        added(&mut s, &rec("北京"), 2);
        assert!(ids(&s, "hang").is_empty());
        assert!(ids(&s, "ing").is_empty());
        assert_eq!(ids(&s, "shanghai"), [sh]);
    }

    #[test]
    fn mark_only_when_title_matches() {
        let mut s = mem();
        let trip = added(&mut s, &rec("行程\n下午去银行办卡"), 1);
        let bank = added(&mut s, &rec("招商银行 6225"), 2);
        // 标题“乐行”按 find 能找到 yuehang，但标题自身的索引不满足，命中来自正文“越航”，不该高亮
        let body = added(&mut s, &rec("乐行\n越航"), 3);
        let h = s.search_hits("yuehang").unwrap();
        assert_eq!(h.iter().find(|x| x.id == body).unwrap().mark, None);
        let h = s.search_hits("yinhang").unwrap();
        let get = |id| h.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get(trip).mark, None);
        assert_eq!(get(bank).mark.as_deref(), Some("银行"));
    }

    #[test]
    fn search_hits_marks_matched_han() {
        let mut s = mem();
        let han = added(&mut s, &rec("周四会议纪要"), 1);
        let lit = added(&mut s, &rec("huiyi notes"), 2);
        let img = added(&mut s, &rec_image(1), 3);
        s.set_ocr(img, ocr_state::TEXT, Some("会议号：482 913 776"))
            .unwrap();
        let h = s.search_hits("huiyi").unwrap();
        let get = |id| h.iter().find(|x| x.id == id).unwrap();
        assert_eq!(get(han).mark.as_deref(), Some("会议"));
        assert_eq!(get(han).snippet, None);
        assert_eq!(get(lit).mark, None);
        let h = s.search_hits("huiyihao").unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].mark.as_deref(), Some("会议号"));
        assert!(h[0].snippet.as_deref().unwrap().contains("会议号"));
        // 非拼音查询：mark 恒为 None
        assert!(
            s.search_hits("会议")
                .unwrap()
                .iter()
                .all(|x| x.mark.is_none())
        );
    }

    #[test]
    fn search_hits_returns_snippet_only_for_ocr_matches() {
        let mut s = mem();
        let titled = added(&mut s, &rec("预算报告"), 1);
        let img = added(&mut s, &rec_image(1), 2);
        s.set_ocr(
            img,
            ocr_state::TEXT,
            Some("第一行说明文字很长很长很长 Q3 季度预算 2026 年度计划与执行情况汇总"),
        )
        .unwrap();
        let short = added(&mut s, &rec_image(2), 3);
        s.set_ocr(short, ocr_state::TEXT, Some("预算")).unwrap();
        let hits: Vec<(i64, Option<String>)> = s
            .search_hits("预算")
            .unwrap()
            .into_iter()
            .map(|h| (h.id, h.snippet))
            .collect();
        assert_eq!(
            hits,
            [
                (short, Some("预算".into())),
                (
                    img,
                    Some("…很长很长很长 Q3 季度预算 2026 年度计划与执…".into())
                ),
                (titled, None),
            ]
        );
        // 大小写不敏感，片段保留原文大小写
        assert_eq!(
            s.search_hits("q3").unwrap()[0].snippet.as_deref(),
            Some("…行说明文字很长很长很长 Q3 季度预算 2026 年…")
        );
    }

    #[test]
    fn ocr_pending_ids_orders_recent_first_and_filters_kind() {
        let mut s = mem();
        let a = added(&mut s, &rec_image(1), 1);
        let b = added(&mut s, &rec_image(2), 2);
        let c = added(&mut s, &rec_image(3), 3);
        added(&mut s, &rec("text"), 4);
        let p = ocr_state::PENDING;
        assert_eq!(s.ocr_pending_ids(&[p], 10).unwrap(), [c, b, a]);
        assert_eq!(s.ocr_pending_ids(&[p], 1).unwrap(), [c]);
        s.set_ocr(a, ocr_state::FAILED, None).unwrap();
        s.set_ocr(b, ocr_state::TEXT, Some("x")).unwrap();
        s.soft_delete(c, 5).unwrap();
        assert!(s.ocr_pending_ids(&[p], 10).unwrap().is_empty());
        assert_eq!(s.ocr_pending_ids(&[p, ocr_state::FAILED], 10).unwrap(), [a]);
        // a 失败、b 有文字：都算已处理
        assert_eq!(s.ocr_progress().unwrap(), (2, 2));
    }

    #[test]
    fn ocr_pending_includes_file_records_that_contain_images() {
        let mut s = mem();
        let mixed = added(&mut s, &rec_files(&["/d/报告.pdf", "/d/图.JPG"]), 1);
        added(&mut s, &rec_files(&["/d/报告.pdf", "/d/数据.csv"]), 2);
        added(&mut s, &rec_files(&["/v/a.mp4"]), 3);
        let img = added(&mut s, &rec_image(1), 4);
        let p = ocr_state::PENDING;
        assert_eq!(s.ocr_pending_ids(&[p], 10).unwrap(), [img, mixed]);
        assert_eq!(s.ocr_progress().unwrap(), (0, 2));
    }

    #[test]
    fn set_ocr_many_changes_only_live_pending_rows() {
        let mut s = mem();
        let a = added(&mut s, &rec_image(1), 1);
        let b = added(&mut s, &rec_image(2), 2);
        let c = added(&mut s, &rec_image(3), 3);
        let d = added(&mut s, &rec_image(4), 4);
        s.set_ocr(b, ocr_state::TEXT, Some("x")).unwrap();
        s.soft_delete(c, 5).unwrap();
        assert!(s.set_ocr_many(&[], ocr_state::SKIPPED).unwrap().is_empty());
        let mut changed = s.set_ocr_many(&[a, b, c, d], ocr_state::SKIPPED).unwrap();
        changed.sort_unstable();
        assert_eq!(changed, [a, d]);
        assert_eq!(s.ocr_state(a).unwrap(), Some(ocr_state::SKIPPED));
        assert_eq!(s.ocr_state(b).unwrap(), Some(ocr_state::TEXT));
        assert_eq!(s.ocr_progress().unwrap(), (3, 3));
    }

    #[test]
    fn ocr_state_reads_live_rows_only() {
        let mut s = mem();
        let a = added(&mut s, &rec_image(1), 1);
        assert_eq!(s.ocr_state(a).unwrap(), Some(ocr_state::PENDING));
        s.set_ocr(a, ocr_state::FAILED, None).unwrap();
        assert_eq!(s.ocr_state(a).unwrap(), Some(ocr_state::FAILED));
        s.soft_delete(a, 2).unwrap();
        assert_eq!(s.ocr_state(a).unwrap(), None);
        assert_eq!(s.ocr_state(a + 100).unwrap(), None);
    }
}
