//! 空实现：没有私有代码时的构建。`open` 不碰磁盘，所有写操作返回 `Unavailable`。

use std::path::{Path, PathBuf};

use crate::types::*;

pub const AVAILABLE: bool = false;

pub struct Line;

impl Line {
    pub fn open(_data_dir: &Path, _now: i64, _app_major: u32) -> Line {
        Line
    }

    pub fn status(&self, _now: i64) -> Pro {
        Pro::Unavailable
    }

    pub fn activate(&mut self, _code: &str, _now: i64) -> Result<Pro, LicenseError> {
        Err(LicenseError::Unavailable)
    }

    pub fn notes(&self) -> &[Note] {
        &[]
    }

    pub fn add(&mut self, _n: NewNote, _at: Option<usize>, _now: i64) -> Result<Note, LineError> {
        Err(LineError::Unavailable)
    }

    pub fn edit(
        &mut self,
        _id: u64,
        _body: Option<String>,
        _memo: Option<String>,
    ) -> Result<Note, LineError> {
        Err(LineError::Unavailable)
    }

    pub fn set_image(&mut self, _id: u64, _png: &[u8], _now: i64) -> Result<Note, LineError> {
        Err(LineError::Unavailable)
    }

    pub fn reorder(&mut self, _ids: &[u64]) -> Result<(), LineError> {
        Err(LineError::Unavailable)
    }

    pub fn remove(&mut self, _id: u64) -> Result<(Note, usize), LineError> {
        Err(LineError::Unavailable)
    }

    pub fn restore(&mut self, _note: Note, _at: usize) -> Result<(), LineError> {
        Err(LineError::Unavailable)
    }

    pub fn hung_records(&self) -> Vec<i64> {
        Vec::new()
    }

    pub fn image_path(&self, _note: &Note) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_has_no_pro() {
        let dir = std::env::temp_dir().join("xpaste-pro-stub-test");
        let mut l = Line::open(&dir, 0, 1);
        // black_box：避免 clippy 对常量断言报错
        assert!(!std::hint::black_box(AVAILABLE));
        assert!(matches!(l.status(0), Pro::Unavailable));
        assert!(l.notes().is_empty());
        assert!(matches!(
            l.activate("XP1-a.b", 0),
            Err(LicenseError::Unavailable)
        ));
        assert!(matches!(l.remove(1), Err(LineError::Unavailable)));
        assert!(!dir.join("line.json").exists());
    }
}
