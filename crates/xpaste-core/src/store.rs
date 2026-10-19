//! SQLite 存储：建库、迁移、键值与设置持久化。

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::{CoreError, Settings};

/// 按版本排列的迁移脚本；第 i 项把 `user_version` 从 i 升到 i+1。
const MIGRATIONS: &[&str] = &[r"
CREATE TABLE apps (id TEXT PRIMARY KEY, name TEXT NOT NULL, icon_path TEXT);
CREATE TABLE records (
  id INTEGER PRIMARY KEY, hash BLOB NOT NULL UNIQUE, kind TEXT NOT NULL, title TEXT NOT NULL,
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
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering::SeqCst};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn tmp() -> PathBuf {
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
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
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
}
