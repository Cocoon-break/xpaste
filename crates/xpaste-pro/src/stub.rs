//! 空实现：没有私有代码时的构建。`open` 不碰磁盘，所有写操作返回 `Unavailable`。

use std::path::{Path, PathBuf};

use xpaste_core::model::Recipe;

use crate::types::*;

pub const AVAILABLE: bool = false;

/// 空实现不含 Pro，签名是否官方无从谈起
pub fn genuine() -> bool {
    false
}

/// 空实现不认任何授权码
pub fn normalize_code(_input: &str) -> Result<String, LicenseError> {
    Err(LicenseError::Unavailable)
}

pub struct Line;

impl Line {
    pub fn open(
        _data_dir: &Path,
        _now: i64,
        _app_major: u32,
        _hardware_id: fn() -> Option<String>,
    ) -> Line {
        Line
    }

    pub fn status(&self, _now: i64) -> Pro {
        Pro::Unavailable
    }

    pub fn device(&self) -> Result<String, LicenseError> {
        Err(LicenseError::Unavailable)
    }

    pub fn accept(
        &mut self,
        _code: &str,
        _credential: &str,
        _now: i64,
    ) -> Result<Pro, LicenseError> {
        Err(LicenseError::Unavailable)
    }

    pub fn code(&self) -> Option<String> {
        None
    }

    pub fn renewal(&self, _now: i64) -> Option<String> {
        None
    }

    pub fn reject(&mut self, _code: &str, _why: Rejection) {}

    pub fn forget(&mut self) -> Result<(), LicenseError> {
        Err(LicenseError::Unavailable)
    }

    pub fn notes(&self) -> &[Note] {
        &[]
    }

    pub fn check_add(&self, _n: &NewNote, _now: i64) -> Result<(), LineError> {
        Err(LineError::Unavailable)
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

    pub fn set_cover(&mut self, _record_id: i64, _src: &Path) -> Result<usize, LineError> {
        Err(LineError::Unavailable)
    }

    pub fn remind(&mut self, _id: u64, _at: Option<i64>) -> Result<Note, LineError> {
        Err(LineError::Unavailable)
    }

    pub fn reorder(&mut self, _ids: &[u64]) -> Result<(), LineError> {
        Err(LineError::Unavailable)
    }

    pub fn place(&mut self, _places: &[(u64, Option<f64>)]) -> Result<(), LineError> {
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

    pub fn recovered(&self) -> bool {
        false
    }
}

/// 空实现的传送带：放不上任何东西
#[derive(Default)]
pub struct Belt;

impl Belt {
    pub fn new() -> Belt {
        Belt
    }

    pub fn items(&self) -> &[BeltItem] {
        &[]
    }

    pub fn next(&self) -> Option<&BeltItem> {
        None
    }

    pub fn can_back(&self) -> bool {
        false
    }

    pub fn push(
        &mut self,
        _kind: Kind,
        _title: String,
        _record_id: Option<i64>,
        _text: Option<String>,
    ) -> Result<u64, BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn split(&mut self, _id: u64, _parts: Vec<String>) -> Result<(), BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn pick(&mut self, _id: u64) -> Result<(), BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn reorder(&mut self, _ids: &[u64]) -> Result<(), BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn remove(&mut self, _id: u64) -> Result<Removed, BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn restore(&mut self, _r: Removed) -> Result<(), BeltError> {
        Err(BeltError::NotFound)
    }

    pub fn take_next(&mut self) -> Option<BeltItem> {
        None
    }

    pub fn back(&mut self) -> Option<u64> {
        None
    }

    pub fn clear(&mut self) -> Cleared {
        Cleared {
            items: Vec::new(),
            next: None,
        }
    }

    pub fn unclear(&mut self, _c: Cleared) {}

    pub fn forget_done(&mut self) {}
}

pub fn split_offer(_text: &str, _room: usize) -> Option<Split> {
    None
}

/// 空实现的杯子：放不进任何东西
#[derive(Default)]
pub struct Cup;

impl Cup {
    pub fn new() -> Cup {
        Cup
    }

    pub fn items(&self) -> &[Ingredient] {
        &[]
    }

    pub fn push(
        &mut self,
        _kind: IngredientKind,
        _title: String,
        _record_id: Option<i64>,
        _body: Body,
    ) -> Result<u64, CupError> {
        Err(CupError::NotFound)
    }

    pub fn room(&self) -> usize {
        0
    }

    pub fn remove(&mut self, _id: u64) -> Result<Taken, CupError> {
        Err(CupError::NotFound)
    }

    pub fn restore(&mut self, _t: Taken) {}

    pub fn clear(&mut self) -> Vec<Ingredient> {
        Vec::new()
    }

    pub fn refill(&mut self, _items: Vec<Ingredient>) {}

    pub fn text_chars(&self) -> usize {
        0
    }
}

pub fn default_recipes() -> Vec<Recipe> {
    Vec::new()
}

pub fn normalize_recipes(_list: Vec<Recipe>) -> Vec<Recipe> {
    Vec::new()
}

pub fn saved_name(_text: &str, _list: &[Recipe]) -> String {
    String::new()
}

pub fn chat_body(
    _model: &str,
    _prompt: Option<&str>,
    _extra: &str,
    _materials: &[Material],
) -> serde_json::Value {
    serde_json::Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_has_no_pro() {
        let dir = std::env::temp_dir().join("xpaste-pro-stub-test");
        let mut l = Line::open(&dir, 0, 1, || None);
        // black_box：避免 clippy 对常量断言报错
        assert!(!std::hint::black_box(AVAILABLE));
        assert!(matches!(l.status(0), Pro::Unavailable));
        assert!(l.notes().is_empty());
        assert_eq!(
            normalize_code("XP-Y3PZC-XY28W-3Q5B5-J92PF"),
            Err(LicenseError::Unavailable)
        );
        assert_eq!(l.device(), Err(LicenseError::Unavailable));
        assert!(matches!(
            l.accept("XP-Y3PZC-XY28W-3Q5B5-J92PF", "a.b", 0),
            Err(LicenseError::Unavailable)
        ));
        l.reject("XP-Y3PZC-XY28W-3Q5B5-J92PF", Rejection::Revoked);
        assert!(l.code().is_none() && l.renewal(0).is_none());
        assert_eq!(l.forget(), Err(LicenseError::Unavailable));
        assert!(!dir.join("pro.json").exists());
        assert!(matches!(l.remove(1), Err(LineError::Unavailable)));
        assert!(matches!(l.remind(1, Some(0)), Err(LineError::Unavailable)));
        assert!(matches!(l.place(&[]), Err(LineError::Unavailable)));
        assert!(!dir.join("line.json").exists());
    }
}
