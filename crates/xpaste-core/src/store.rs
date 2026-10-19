//! SQLite 存储：建库、迁移、键值与设置持久化，记录读写、去重与清理。

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde_json::Value;

use crate::{
    AppSummary, CoreError, FileEntry, Flavor, ImageInfo, Kind, NewRecord, RecordDetail,
    RecordSummary, Settings,
};

/// 按版本排列的迁移脚本；第 i 项把 `user_version` 从 i 升到 i+1。
const MIGRATIONS: &[&str] = &[r"
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
"];

const SUBDIRS: [&str; 4] = ["blobs", "images", "thumbs", "icons"];

/// 超过此大小的 flavor 数据写到 `blobs/<id>/<n>`。
const INLINE_MAX: usize = 1_048_576;
/// 详情返回的正文上限（字节）。
const DETAIL_TEXT_MAX: usize = 200 * 1024;
const DAY_MS: i64 = 86_400_000;

const LIVE_UNPINNED: &str = "pinned = 0 AND deleted_at IS NULL";

const SUMMARY_SQL: &str = "SELECT r.id, r.kind, r.title, a.name, a.icon_path, r.copied_at, r.uses,
  r.pinned, r.size, r.lang, r.meta FROM records r LEFT JOIN apps a ON a.id = r.app_id";

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

    /// 缺失或损坏时回落到默认设置。
    pub fn load_settings(&self) -> Settings {
        match self.kv_get("settings") {
            Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_else(|e| {
                log::warn!("设置解析失败，使用默认值：{e}");
                Settings::default()
            }),
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
               search_text, app_id, copied_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
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
                now_ms
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

    pub fn detail(&self, id: i64) -> Result<Option<RecordDetail>, CoreError> {
        let Some(summary) = self.summary(id)? else {
            return Ok(None);
        };
        let (text, link_title, paths, meta): (Option<String>, _, Option<String>, Option<String>) =
            self.conn.query_row(
                "SELECT text, link_title, paths, meta FROM records WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
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
        let files = paths
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
        let video = meta.as_ref().and_then(|m| m.get("video").cloned());
        Ok(Some(RecordDetail {
            summary,
            text,
            truncated,
            link_title,
            files,
            image,
            video,
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

    /// 不区分大小写的子串匹配；空查询返回全部未删除记录。
    pub fn search_ids(&self, q: &str) -> Result<Vec<i64>, CoreError> {
        let order = "ORDER BY copied_at DESC, id DESC";
        if q.trim().is_empty() {
            let sql = format!("SELECT id FROM records WHERE deleted_at IS NULL {order}");
            return ids_in(&self.conn, &sql, &[]);
        }
        let sql = format!(
            "SELECT id FROM records WHERE deleted_at IS NULL AND instr(search_text, ?1) > 0 {order}"
        );
        ids_in(&self.conn, &sql, &[&q.to_lowercase()])
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
            duration_sec: meta.as_ref().and_then(|m| m.get("duration")?.as_f64()),
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

fn migrate(conn: &mut Connection) -> Result<(), CoreError> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (v, sql) in (1u32..).zip(MIGRATIONS).skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", v)?;
        tx.commit()?;
    }
    Ok(())
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
        assert_eq!(s.schema_version().unwrap(), 1);
        for sub in ["blobs", "images", "thumbs", "icons"] {
            assert!(dir.join(sub).is_dir());
        }
        let tables: Vec<String> = s
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(tables, ["apps", "flavors", "kv", "records"]);
    }

    #[test]
    fn reopen_does_not_rerun_migrations() {
        let dir = tmp();
        drop(Store::open(&dir).unwrap());
        let s = Store::open(&dir).unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
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
    fn foreign_keys_are_enforced() {
        let s = Store::open_in_memory(&tmp()).unwrap();
        let r = s.conn.execute(
            "INSERT INTO flavors(record_id,item,format,data) VALUES (999,0,'x',x'00')",
            [],
        );
        assert!(r.is_err());
    }

    use crate::capture::build_record;
    use crate::{AppInfo, Flavor, Kind, NewRecord, Snapshot, detect};

    fn mem() -> Store {
        Store::open_in_memory(&tmp()).unwrap()
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
    fn rec_files(p: &[&str]) -> NewRecord {
        rec_of(Snapshot {
            file_paths: p.iter().map(Into::into).collect(),
            ..Default::default()
        })
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
        assert_eq!(s.search_ids("HELLO").unwrap().len(), 1);
        assert_eq!(s.search_ids("会议").unwrap().len(), 1);
        assert_eq!(s.search_ids("").unwrap().len(), 2);
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
}
