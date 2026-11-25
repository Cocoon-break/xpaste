//! 改字的留存和存成文件。数据目录的 `textedit/` 下一张图一个文件夹（文件夹名是 id）：`orig.png` 原图、
//! `job.json` 每处改动、每个候选一张 PNG。只有用户能删：不跟保留天数、最多条数、清空历史走。

use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

use image::{ImageFormat, ImageReader};
use serde::{Deserialize, Serialize};
use xpaste_pro::{TeImg, TePaste, TeQuad};

use crate::{housekeeping, img_meta, shot};

/// 没有选中任何候选（改回去了，或结果到得太晚）。页面拿得到，所以不超过 JS 的安全整数
#[allow(dead_code)] // 改字的任务接入后删
pub const NO_PICK: usize = u32::MAX as usize;

/// 一张改过字的图。带用户文字，`Job`、`Edit`、`Cand` 都不派生 Debug
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub w: u32,
    pub h: u32,
    pub created: i64,
    /// 先后就是画的先后
    pub edits: Vec<Edit>,
}

/// 一处改动
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Edit {
    pub id: u64,
    pub line: TeQuad,
    pub range: [f32; 2],
    pub old: String,
    pub new: String,
    /// 选中第几个候选；没选中时为 `NO_PICK`。被后来的改动替换的也写成 `NO_PICK` 留着，不从列表里去掉：
    /// 候选都要留
    pub pick: usize,
    pub cands: Vec<Cand>,
}

/// 一个候选
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cand {
    /// 图所在文件夹里的文件名
    pub file: String,
    pub x: u32,
    pub y: u32,
    pub by: By,
    pub soft: bool,
    pub note: String,
    /// 没贴好的只留着看，不画
    pub placed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum By {
    Local,
    Ai,
}

impl Edit {
    /// 画上去的那个候选
    fn shown(&self) -> Option<&Cand> {
        self.cands.get(self.pick).filter(|c| c.placed)
    }
}

impl Job {
    /// 画上去的里面有没有 AI 出的（决定元数据怎么写）
    #[allow(dead_code)] // 改字的任务接入后删
    pub fn ai(&self) -> bool {
        self.edits
            .iter()
            .filter_map(Edit::shown)
            .any(|c| c.by == By::Ai)
    }

    /// 出过结果的才留在“最近改过”。没贴好的也算：花过钱
    fn kept(&self) -> bool {
        self.edits.iter().any(|e| !e.cands.is_empty())
    }
}

const ORIG: &str = "orig.png";
const JOB: &str = "job.json";
const TRASH: &str = "trash";

fn bad(e: image::ImageError) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, e)
}

/// id 会从页面传回来：只认自己起的（32 位十六进制），不让它指到别的地方
fn job_dir(dir: &Path, id: &str) -> io::Result<PathBuf> {
    let ok = id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| dir.join(id))
        .ok_or_else(|| ErrorKind::InvalidInput.into())
}

fn write_png(path: &Path, img: &TeImg) -> io::Result<()> {
    let png = img_meta::encode_png(img.w, img.h, &img.px).map_err(bad)?;
    fs::write(path, png)
}

fn read_png(path: &Path) -> io::Result<TeImg> {
    let file = io::BufReader::new(fs::File::open(path)?);
    let mut r = ImageReader::with_format(file, ImageFormat::Png);
    // 自己存的，尺寸打开时查过
    r.no_limits();
    let img = r.decode().map_err(bad)?.into_rgba8();
    Ok(TeImg {
        w: img.width(),
        h: img.height(),
        px: img.into_raw(),
    })
}

/// 存下原图，起一个新的
#[allow(dead_code)] // 改字的任务接入后删
pub fn create(dir: &Path, img: &TeImg) -> io::Result<Job> {
    let job = Job {
        id: crate::meme_gen::uuid(),
        w: img.w,
        h: img.h,
        created: crate::clock::now_ms(),
        edits: vec![],
    };
    let d = job_dir(dir, &job.id)?;
    fs::create_dir_all(&d)?;
    // 先写 job.json：中途退出的，启动时认得出是空的
    save(dir, &job)
        .and_then(|()| write_png(&d.join(ORIG), img))
        .inspect_err(|_| {
            let _ = fs::remove_dir_all(&d);
        })?;
    Ok(job)
}

/// 先写临时文件再改名。同一个 job 的几次保存由调用方排队；文件夹不在了（删了）就报错，不重新建
pub fn save(dir: &Path, job: &Job) -> io::Result<()> {
    let nums = |e: &Edit| {
        e.line
            .iter()
            .flatten()
            .chain(&e.range)
            .all(|v| v.is_finite())
    };
    // 无穷大和 NaN 写成 JSON 就读不回来了
    if !job.edits.iter().all(nums) {
        return Err(ErrorKind::InvalidInput.into());
    }
    let path = job_dir(dir, &job.id)?.join(JOB);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec(job)?)
        .and_then(|()| fs::rename(&tmp, &path))
        .inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
}

fn read(dir: &Path, id: &str) -> io::Result<Job> {
    let job: Job = serde_json::from_slice(&fs::read(job_dir(dir, id)?.join(JOB))?)?;
    if job.id != id {
        return Err(ErrorKind::InvalidData.into());
    }
    Ok(job)
}

#[allow(dead_code)] // 改字的任务接入后删
pub fn load(dir: &Path, id: &str) -> io::Result<Job> {
    let job = read(dir, id)?;
    fs::metadata(job_dir(dir, id)?.join(ORIG))?;
    Ok(job)
}

/// 各张图的 id，坏了的也在内
fn ids(dir: &Path) -> Vec<String> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|id| job_dir(dir, id).is_ok())
        .collect()
}

/// “最近改过”，新的在前；坏了的跳过
pub fn list(dir: &Path) -> Vec<Job> {
    let mut jobs: Vec<Job> = ids(dir)
        .iter()
        .filter_map(|id| load(dir, id).ok())
        .filter(Job::kept)
        .collect();
    jobs.sort_by(|a, b| (b.created, &b.id).cmp(&(a.created, &a.id)));
    jobs
}

/// 给 id 为 `edit` 的那处加一个候选并存盘，返回它是第几个。不改选中的；没贴好的由调用方改 `placed` 再存
#[allow(dead_code)] // 改字的任务接入后删
pub fn add_cand(
    dir: &Path,
    job: &mut Job,
    edit: u64,
    paste: &TePaste,
    by: By,
    note: &str,
) -> io::Result<usize> {
    let at = job
        .edits
        .iter()
        .position(|e| e.id == edit)
        .ok_or(ErrorKind::NotFound)?;
    // 名字不用序号：候选都要留，不能撞上旧的
    let file = format!("{}.png", crate::meme_gen::uuid());
    let path = job_dir(dir, &job.id)?.join(&file);
    write_png(&path, &paste.patch)?;
    job.edits[at].cands.push(Cand {
        file,
        x: paste.x,
        y: paste.y,
        by,
        soft: paste.soft,
        note: note.into(),
        placed: true,
    });
    if let Err(e) = save(dir, job) {
        job.edits[at].cands.pop();
        let _ = fs::remove_file(&path);
        return Err(e);
    }
    Ok(job.edits[at].cands.len() - 1)
}

/// 原图上依次画各处选中的候选。候选是按原图算的矩形，含没变的像素：只画和原图不同的，
/// 两处的矩形叠在一起时，后画的不会把先画的盖回原样
#[allow(dead_code)] // 改字的任务接入后删
pub fn render(dir: &Path, job: &Job) -> io::Result<TeImg> {
    let d = job_dir(dir, &job.id)?;
    let mut img = read_png(&d.join(ORIG))?;
    if (img.w, img.h) != (job.w, job.h) {
        return Err(ErrorKind::InvalidData.into());
    }
    // 先都和原图比完再画，原图可能有 400 MB，不另存一份
    let mut diff = Vec::new();
    for c in job.edits.iter().filter_map(Edit::shown) {
        diffs(&img, &read_png(&d.join(&c.file))?, c, &mut diff);
    }
    for (at, p) in diff {
        img.px[at..at + 4].copy_from_slice(&p);
    }
    Ok(img)
}

/// `patch` 放在 `c` 的位置上，和原图不同的像素：在原图里的下标和新值。伸出图外的不要
fn diffs(img: &TeImg, patch: &TeImg, c: &Cand, out: &mut Vec<(usize, [u8; 4])>) {
    let [w, h, pw, x0, y0] = [img.w, img.h, patch.w, c.x, c.y].map(|v| v as usize);
    for (i, p) in patch.px.as_chunks::<4>().0.iter().enumerate() {
        let (x, y) = (x0 + i % pw, y0 + i / pw);
        let at = (y * w + x) * 4;
        if x < w && y < h && img.px[at..at + 4] != p[..] {
            out.push((at, *p));
        }
    }
}

/// 删一张：移进 `trash/`，撤销期过后彻底删
#[allow(dead_code)] // 改字的任务接入后删
pub fn remove(dir: &Path, id: &str) -> io::Result<()> {
    let path = trash(dir, id)?;
    housekeeping::spawn("xpaste-textedit-purge", move || {
        std::thread::sleep(housekeeping::UNDO_WINDOW);
        // 撤销过的已经不在原处
        match purge(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => log::warn!("改字 清除失败：{e}"),
            _ => {}
        }
    });
    Ok(())
}

/// 移进 `trash/`，返回新位置。每次的名字都不同：撤销后又删的，前一次到点时不会删到它
fn trash(dir: &Path, id: &str) -> io::Result<PathBuf> {
    let from = job_dir(dir, id)?;
    let to = dir.join(TRASH);
    fs::create_dir_all(&to)?;
    let to = to.join(format!("{id}-{}", crate::meme_gen::uuid()));
    fs::rename(from, &to)?;
    Ok(to)
}

/// 先改名再删：正好这时撤销，要么整个回来，要么回不来
fn purge(path: &Path) -> io::Result<()> {
    let gone = path.with_extension("gone");
    fs::rename(path, &gone)?;
    fs::remove_dir_all(gone)
}

/// 撤销删除
#[allow(dead_code)] // 改字的任务接入后删
pub fn restore(dir: &Path, id: &str) -> io::Result<()> {
    let to = job_dir(dir, id)?;
    let prefix = format!("{id}-");
    let from = fs::read_dir(dir.join(TRASH))?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            let name = p.file_name().and_then(|n| n.to_str());
            name.is_some_and(|n| n.starts_with(&prefix)) && p.extension().is_none()
        })
        .ok_or(ErrorKind::NotFound)?;
    fs::rename(from, to)
}

/// 启动时：清掉 `trash/`（撤销期已过）和打开过、一处都没出过结果的。坏了的不动，里面可能有花钱出的图。
/// 这时改字窗口还没开
pub fn clean(dir: &Path) {
    let mut gone: Vec<PathBuf> = ids(dir)
        .iter()
        .filter(|id| read(dir, id).is_ok_and(|j| !j.kept()))
        .map(|id| dir.join(id))
        .collect();
    gone.push(dir.join(TRASH));
    for p in gone.iter().filter(|p| p.exists()) {
        if let Err(e) = fs::remove_dir_all(p) {
            log::warn!("改字 启动清理失败：{e}");
        }
    }
}

/// “最近改过”有几张、占多少字节
#[allow(dead_code)] // 改字的任务接入后删
pub fn stats(dir: &Path) -> (u64, u64) {
    let jobs = list(dir);
    let bytes = jobs
        .iter()
        .filter_map(|j| fs::read_dir(dir.join(&j.id)).ok())
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum();
    (jobs.len() as u64, bytes)
}

/// 清空“最近改过”，坏了的和 `trash/` 一起删；打开着还没出过结果的不在里面，不删
#[allow(dead_code)] // 改字的任务接入后删
pub fn clear(dir: &Path) -> io::Result<()> {
    let mut gone: Vec<PathBuf> = ids(dir)
        .iter()
        .filter(|id| read(dir, id).map_or(true, |j| j.kept()))
        .map(|id| dir.join(id))
        .collect();
    gone.push(dir.join(TRASH));
    // 有一张删不掉，别的照删
    let mut out = Ok(());
    for p in gone.iter().filter(|p| p.exists()) {
        out = out.and(fs::remove_dir_all(p));
    }
    out
}

/// 存不成的原因
#[allow(dead_code)] // 改字的任务接入后删
pub enum SaveFail {
    /// 文件夹写不进去（没有权限、只读、不在了、名字太长）：改走另存为
    Dir,
    Io(io::Error),
}

/// 存在原图旁边的名字：`〈原名〉 改字.〈扩展名〉`，PNG、JPEG 的扩展名照原样，其余 `.png`；重名加序号。
/// 点后面不是图片的扩展名时，是名字的一部分
pub fn out_path(src: &Path) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new(""));
    let format = ImageFormat::from_path(src).ok();
    let stem = if format.is_some() {
        src.file_stem()
    } else {
        src.file_name()
    };
    let ext = match format {
        Some(ImageFormat::Png | ImageFormat::Jpeg) => src.extension(),
        _ => None,
    };
    let [stem, ext] = [stem.unwrap_or_default(), ext.unwrap_or("png".as_ref())]
        .map(|s| s.to_string_lossy().into_owned());
    dir.join(shot::unique_file_name(&stem, "改字", &ext, |n| {
        dir.join(n).symlink_metadata().is_ok()
    }))
}

/// 在原图旁边另存一份，绝不覆盖；`ai` 是图里有没有 AI 出的（`Job::ai`）
#[allow(dead_code)] // 改字的任务接入后删
pub fn write_beside(src: &Path, img: &TeImg, ai: bool) -> Result<PathBuf, SaveFail> {
    let bytes = if ImageFormat::from_path(src).is_ok_and(|f| f == ImageFormat::Jpeg) {
        img_meta::encode_jpeg(img.w, img.h, &img.px).map(|b| img_meta::mark_jpeg(b, ai))
    } else {
        img_meta::encode_png(img.w, img.h, &img.px).map(|b| img_meta::mark_png(b, ai))
    }
    .map_err(|e| SaveFail::Io(bad(e)))?;
    use ErrorKind::*;
    shot::write_new(|| out_path(src), |f| f.write_all(&bytes)).map_err(|e| match e.kind() {
        PermissionDenied | NotFound | NotADirectory | ReadOnlyFilesystem | InvalidFilename => {
            SaveFail::Dir
        }
        _ => SaveFail::Io(e),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的文件夹，用完删掉
    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Self {
            let d = std::env::temp_dir().join(format!("xpaste-te-{}", crate::meme_gen::uuid()));
            fs::create_dir_all(&d).unwrap();
            Self(d)
        }
        fn te(&self) -> PathBuf {
            self.0.join("textedit")
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// 每个像素都不一样；第一个全透明，RGB 不是 0
    fn img(w: u32, h: u32) -> TeImg {
        let mut px: Vec<u8> = (0..w * h)
            .flat_map(|i| [(i % 256) as u8, (i / 256) as u8, 77, 255])
            .collect();
        px[..4].copy_from_slice(&[9, 8, 7, 0]);
        TeImg { w, h, px }
    }

    /// 原图 `(x, y)` 起 w × h 的矩形，其中 `hit`（原图坐标）涂成 `c`
    fn paste(
        base: &TeImg,
        (x, y, w, h): (u32, u32, u32, u32),
        hit: &[(u32, u32)],
        c: [u8; 4],
    ) -> TePaste {
        let mut px = Vec::new();
        for j in y..y + h {
            for i in x..x + w {
                let at = ((j * base.w + i) * 4) as usize;
                px.extend(if hit.contains(&(i, j)) {
                    &c
                } else {
                    &base.px[at..at + 4]
                });
            }
        }
        TePaste {
            patch: TeImg { w, h, px },
            x,
            y,
            exact: false,
            soft: false,
        }
    }

    fn edit(id: u64) -> Edit {
        Edit {
            id,
            line: [[0.5, 1.25], [9.0, 1.0], [9.0, 4.0], [0.1, 4.0]],
            range: [1.5, 3.0],
            old: "三".into(),
            new: "四".into(),
            pick: NO_PICK,
            cands: vec![],
        }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// 一张 16 × 8 的图，一处改动，一个选中的候选（(2, 2) 涂红）
    fn one(dir: &Path) -> (Job, TeImg) {
        let base = img(16, 8);
        let mut job = create(dir, &base).unwrap();
        job.edits.push(edit(1));
        let p = paste(&base, (1, 1, 4, 4), &[(2, 2)], RED);
        job.edits[0].pick = add_cand(dir, &mut job, 1, &p, By::Local, "").unwrap();
        save(dir, &job).unwrap();
        (job, base)
    }

    fn json(job: &Job) -> String {
        serde_json::to_string(job).unwrap()
    }

    /// 涂过的地方之外和 `base` 相同
    fn check(out: &TeImg, base: &TeImg, hits: &[((u32, u32), [u8; 4])]) {
        let mut want = base.px.clone();
        for &((x, y), c) in hits {
            let at = ((y * base.w + x) * 4) as usize;
            want[at..at + 4].copy_from_slice(&c);
        }
        assert_eq!((out.w, out.h), (base.w, base.h));
        assert!(out.px == want);
    }

    #[test]
    fn job_roundtrip() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, _) = one(&d);
        job.edits[0].cands[0].note = "字可能不对，看一下".into();
        save(&d, &job).unwrap();
        let back = load(&d, &job.id).unwrap();
        assert_eq!(json(&back), json(&job));
        let v: serde_json::Value = serde_json::from_str(&json(&back)).unwrap();
        assert_eq!(v["edits"][0]["cands"][0]["by"], "local");
        // 不留临时文件：原图、job.json、一个候选
        assert_eq!(fs::read_dir(d.join(&job.id)).unwrap().count(), 3);
        // 写不成读得回来的数值：不动上一份
        job.edits[0].range[1] = f32::INFINITY;
        assert!(save(&d, &job).is_err());
        assert_eq!(json(&load(&d, &job.id).unwrap()), json(&back));
    }

    #[test]
    fn bad_id_rejected() {
        let t = Tmp::new();
        let d = t.te();
        one(&d);
        for id in ["", ".", "..", "../textedit", "trash", "a/b", "/"] {
            assert!(load(&d, id).is_err());
            assert!(remove(&d, id).is_err());
            assert!(restore(&d, id).is_err());
        }
        assert_eq!(list(&d).len(), 1);
    }

    /// 存不成：不留半截文件夹
    #[test]
    fn create_fail_leaves_nothing() {
        let t = Tmp::new();
        let bad = TeImg {
            w: 0,
            h: 0,
            px: vec![],
        };
        assert!(create(&t.te(), &bad).is_err());
        assert_eq!(fs::read_dir(t.te()).unwrap().count(), 0);
    }

    #[test]
    fn no_edits_not_listed() {
        let t = Tmp::new();
        let d = t.te();
        assert!(list(&d).is_empty());
        let mut job = create(&d, &img(4, 4)).unwrap();
        assert!(list(&d).is_empty());
        // 有一处，但还没出过结果
        job.edits.push(edit(1));
        save(&d, &job).unwrap();
        assert!(list(&d).is_empty());
        assert_eq!(stats(&d), (0, 0));
        let p = paste(&img(4, 4), (0, 0, 1, 1), &[], RED);
        add_cand(&d, &mut job, 1, &p, By::Ai, "").unwrap();
        assert_eq!(list(&d).len(), 1);
    }

    #[test]
    fn list_new_first() {
        let t = Tmp::new();
        let d = t.te();
        let made: Vec<Job> = [20, 30, 10]
            .iter()
            .map(|&at| {
                let (mut j, _) = one(&d);
                j.created = at;
                save(&d, &j).unwrap();
                j
            })
            .collect();
        let got: Vec<String> = list(&d).into_iter().map(|j| j.id).collect();
        assert_eq!(got, [1, 0, 2].map(|i: usize| made[i].id.clone()));
        let (n, bytes) = stats(&d);
        assert_eq!(n, 3);
        let sum: u64 = made
            .iter()
            .flat_map(|j| fs::read_dir(d.join(&j.id)).unwrap())
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(bytes, sum);
    }

    #[test]
    fn cands_kept_after_repick() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, base) = one(&d);
        for c in [BLUE, [0, 255, 0, 255]] {
            let p = paste(&base, (1, 1, 4, 4), &[(3, 3)], c);
            add_cand(&d, &mut job, 1, &p, By::Ai, "").unwrap();
        }
        // 加候选不改选中的
        assert_eq!(load(&d, &job.id).unwrap().edits[0].pick, 0);
        for pick in [2, 1, 0, NO_PICK, 1] {
            job.edits[0].pick = pick;
            save(&d, &job).unwrap();
        }
        let back = load(&d, &job.id).unwrap();
        assert_eq!((back.edits[0].pick, back.edits[0].cands.len()), (1, 3));
        for c in &back.edits[0].cands {
            assert!(d.join(&job.id).join(&c.file).is_file());
        }
        check(&render(&d, &back).unwrap(), &base, &[((3, 3), BLUE)]);
    }

    /// 那一处不在了：不写文件
    #[test]
    fn add_cand_unknown_edit() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, base) = one(&d);
        let p = paste(&base, (0, 0, 2, 2), &[], RED);
        assert!(add_cand(&d, &mut job, 9, &p, By::Ai, "").is_err());
        assert_eq!(fs::read_dir(d.join(&job.id)).unwrap().count(), 3);
        // 文件夹不在了（删了）：不重新建出来，也不留在列表里
        remove(&d, &job.id).unwrap();
        assert!(add_cand(&d, &mut job, 1, &p, By::Ai, "").is_err());
        assert_eq!(job.edits[0].cands.len(), 1);
        assert!(save(&d, &job).is_err());
        assert!(list(&d).is_empty());
    }

    #[test]
    fn unplaced_not_rendered() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, base) = one(&d);
        // 没贴好的是接口出的那张，比原图大，位置也没有意义
        let p = TePaste {
            patch: img(40, 40),
            x: 0,
            y: 0,
            exact: false,
            soft: false,
        };
        let i = add_cand(&d, &mut job, 1, &p, By::Ai, "AI 出的图对不上").unwrap();
        job.edits[0].cands[i].placed = false;
        job.edits[0].pick = i;
        save(&d, &job).unwrap();
        let back = load(&d, &job.id).unwrap();
        check(&render(&d, &back).unwrap(), &base, &[]);
        assert!(!back.ai());
        // 什么都没选中
        job.edits[0].pick = NO_PICK;
        check(&render(&d, &job).unwrap(), &base, &[]);
    }

    /// 画上去的才算
    #[test]
    fn ai_counts_picked_only() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, base) = one(&d);
        let p = paste(&base, (1, 1, 2, 2), &[(1, 1)], RED);
        let i = add_cand(&d, &mut job, 1, &p, By::Ai, "").unwrap();
        assert!(!job.ai());
        job.edits[0].pick = i;
        assert!(job.ai());
    }

    #[test]
    fn render_outside_untouched() {
        let t = Tmp::new();
        let d = t.te();
        let (job, base) = one(&d);
        check(&render(&d, &job).unwrap(), &base, &[((2, 2), RED)]);
        // 伸出图外的裁掉
        let mut job = job;
        job.edits.push(edit(2));
        let mut p = paste(&base, (0, 0, 2, 2), &[(0, 0), (1, 0), (0, 1), (1, 1)], BLUE);
        (p.x, p.y) = (15, 7);
        job.edits[1].pick = add_cand(&d, &mut job, 2, &p, By::Local, "").unwrap();
        check(
            &render(&d, &job).unwrap(),
            &base,
            &[((2, 2), RED), ((15, 7), BLUE)],
        );
    }

    /// 后贴的矩形盖住先贴的：里面没变的像素不盖回去；两处都改的地方按先后
    #[test]
    fn render_overlapping_patches() {
        let t = Tmp::new();
        let d = t.te();
        let (mut job, base) = one(&d);
        job.edits.push(edit(2));
        let p = paste(&base, (0, 0, 10, 6), &[(7, 4)], BLUE);
        job.edits[1].pick = add_cand(&d, &mut job, 2, &p, By::Ai, "").unwrap();
        check(
            &render(&d, &job).unwrap(),
            &base,
            &[((2, 2), RED), ((7, 4), BLUE)],
        );
        let p = paste(&base, (0, 0, 10, 6), &[(2, 2), (7, 4)], BLUE);
        job.edits[1].pick = add_cand(&d, &mut job, 2, &p, By::Ai, "").unwrap();
        check(
            &render(&d, &job).unwrap(),
            &base,
            &[((2, 2), BLUE), ((7, 4), BLUE)],
        );
        job.edits.swap(0, 1);
        check(
            &render(&d, &job).unwrap(),
            &base,
            &[((2, 2), RED), ((7, 4), BLUE)],
        );
    }

    /// 候选文件没了：报错，不交出少了一处的图
    #[test]
    fn render_missing_cand_fails() {
        let t = Tmp::new();
        let d = t.te();
        let (job, _) = one(&d);
        fs::remove_file(d.join(&job.id).join(&job.edits[0].cands[0].file)).unwrap();
        assert!(render(&d, &job).is_err());
    }

    #[test]
    fn corrupt_job_skipped() {
        let t = Tmp::new();
        let d = t.te();
        let (good, _) = one(&d);
        let (bad, _) = one(&d);
        fs::write(d.join(&bad.id).join("job.json"), b"{").unwrap();
        let (moved, _) = one(&d);
        fs::rename(d.join(&moved.id), d.join(crate::meme_gen::uuid())).unwrap();
        let (no_orig, _) = one(&d);
        fs::remove_file(d.join(&no_orig.id).join("orig.png")).unwrap();
        fs::create_dir_all(d.join("x")).unwrap();
        fs::write(d.join(".DS_Store"), b"x").unwrap();
        let got: Vec<String> = list(&d).into_iter().map(|j| j.id).collect();
        assert_eq!(got, [good.id]);
        assert!(load(&d, &bad.id).is_err());
        // 坏了的不自动删：里面的候选可能花了钱
        clean(&d);
        assert!(d.join(&bad.id).is_dir());
        assert!(d.join(&no_orig.id).is_dir());
        // 用户清空时一起删
        clear(&d).unwrap();
        assert!(!d.join(&bad.id).exists());
    }

    #[test]
    fn remove_then_restore() {
        let t = Tmp::new();
        let d = t.te();
        let (job, base) = one(&d);
        assert!(restore(&d, &job.id).is_err());
        remove(&d, &job.id).unwrap();
        assert!(list(&d).is_empty());
        assert!(load(&d, &job.id).is_err());
        assert!(remove(&d, &job.id).is_err());
        assert_eq!(stats(&d), (0, 0));
        restore(&d, &job.id).unwrap();
        assert_eq!(json(&list(&d)[0]), json(&job));
        check(&render(&d, &job).unwrap(), &base, &[((2, 2), RED)]);

        // 撤销后又删：前一次的到点不删这一次的
        let first = trash(&d, &job.id).unwrap();
        restore(&d, &job.id).unwrap();
        let second = trash(&d, &job.id).unwrap();
        assert!(purge(&first).is_err());
        assert!(second.is_dir());
        // 到点：删干净，撤销不回来
        purge(&second).unwrap();
        assert!(restore(&d, &job.id).is_err());
        assert_eq!(fs::read_dir(d.join("trash")).unwrap().count(), 0);
        // 正在删的不撤销回来
        fs::create_dir(d.join("trash").join(format!("{}-x.gone", job.id))).unwrap();
        assert!(restore(&d, &job.id).is_err());
    }

    #[test]
    fn startup_clears_trash() {
        let t = Tmp::new();
        let d = t.te();
        clean(&d);
        assert!(!d.exists());
        let (kept, _) = one(&d);
        let (gone, _) = one(&d);
        // 打开过、一处都没改好的，不留
        let opened = create(&d, &img(4, 4)).unwrap();
        remove(&d, &gone.id).unwrap();
        clean(&d);
        assert!(!d.join("trash").exists());
        assert!(!d.join(&opened.id).exists());
        assert!(restore(&d, &gone.id).is_err());
        assert_eq!(list(&d)[0].id, kept.id);
    }

    #[test]
    fn clear_all() {
        let t = Tmp::new();
        let d = t.te();
        clear(&d).unwrap();
        let (a, _) = one(&d);
        one(&d);
        // 窗口里刚打开、还没改好的不算“最近改过”，不清
        let opened = create(&d, &img(4, 4)).unwrap();
        remove(&d, &a.id).unwrap();
        clear(&d).unwrap();
        assert!(list(&d).is_empty());
        assert!(!d.join("trash").exists());
        assert!(load(&d, &opened.id).is_ok());
    }

    /// 清空历史、保留天数、最多条数、启动清软删除都不动改字的数据
    #[test]
    fn clear_history_keeps_textedit() {
        let t = Tmp::new();
        let d = t.te();
        let (job, base) = one(&d);
        let mut store = xpaste_core::Store::open(&t.0).unwrap();
        store.clear_unpinned().unwrap();
        store.prune_retention(Some(1), i64::MAX).unwrap();
        store.prune_capacity(0).unwrap();
        store.purge_all_soft_deleted().unwrap();
        drop(store);
        xpaste_core::Store::open(&t.0).unwrap();
        assert_eq!(json(&list(&d)[0]), json(&job));
        check(&render(&d, &job).unwrap(), &base, &[((2, 2), RED)]);
    }

    #[test]
    fn out_path_table() {
        let t = Tmp::new();
        for (src, out) in [
            ("a.jpg", "a 改字.jpg"),
            ("a.JPG", "a 改字.JPG"),
            ("a.jpeg", "a 改字.jpeg"),
            ("a.png", "a 改字.png"),
            ("a.PNG", "a 改字.PNG"),
            ("a.bmp", "a 改字.png"),
            ("a.BMP", "a 改字.png"),
            ("a.webp", "a 改字.png"),
            ("a.tiff", "a 改字.png"),
            ("a.gif", "a 改字.png"),
            ("a.b.c.jpg", "a.b.c 改字.jpg"),
            ("a", "a 改字.png"),
            // 点后面不是图片的扩展名：是名字的一部分
            ("截图 10.30", "截图 10.30 改字.png"),
            (".png", ".png 改字.png"),
            ("a 改字.jpg", "a 改字 改字.jpg"),
        ] {
            assert_eq!(out_path(&t.0.join(src)), t.0.join(out));
        }
        fs::write(t.0.join("a 改字.jpg"), b"x").unwrap();
        assert_eq!(out_path(&t.0.join("a.jpg")), t.0.join("a 改字 2.jpg"));
        fs::write(t.0.join("a 改字 2.jpg"), b"x").unwrap();
        assert_eq!(out_path(&t.0.join("a.jpg")), t.0.join("a 改字 3.jpg"));
    }

    #[test]
    fn write_never_overwrites() {
        let t = Tmp::new();
        let src = t.0.join("a.png");
        fs::write(&src, b"src").unwrap();
        fs::write(t.0.join("a 改字.png"), b"mine").unwrap();
        // 失效的符号链接也算占用
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.0.join("nowhere"), t.0.join("a 改字 2.png")).unwrap();
        // 几处同时存：名字都不同
        let mut got: Vec<PathBuf> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..6)
                .map(|_| s.spawn(|| write_beside(&src, &img(8, 8), false).ok().unwrap()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        got.sort();
        got.dedup();
        assert_eq!(got.len(), 6);
        assert!(!got.contains(&t.0.join("a 改字.png")));
        assert_eq!(fs::read(&src).unwrap(), b"src");
        assert_eq!(fs::read(t.0.join("a 改字.png")).unwrap(), b"mine");
        #[cfg(unix)]
        assert!(!t.0.join("nowhere").exists());
        for p in got {
            assert!(image::open(p).unwrap().into_rgba8().into_raw() == img(8, 8).px);
        }
    }

    /// 内容按扩展名：PNG 逐字节相同，JPEG 质量 95、不降色度；都带元数据
    #[test]
    fn write_formats() {
        let t = Tmp::new();
        let p = write_beside(&t.0.join("a.webp"), &img(8, 8), true)
            .ok()
            .unwrap();
        let png = fs::read(&p).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        let has = |b: &[u8], s: &str| b.windows(s.len()).any(|w| w == s.as_bytes());
        assert!(has(&png, "xPaste 改字") && has(&png, "compositeWithTrained"));
        let back = image::load_from_memory(&png).unwrap().into_rgba8();
        assert!(back.into_raw() == img(8, 8).px);

        let p = write_beside(&t.0.join("a.JPG"), &img(16, 16), false)
            .ok()
            .unwrap();
        let jpg = fs::read(&p).unwrap();
        assert_eq!(jpg[..2], [0xFF, 0xD8]);
        assert!(has(&jpg, "xPaste 改字") && !has(&jpg, "compositeWithTrained"));
        // SOF0：三个分量的采样都是 1 × 1（4:4:4）
        let sof = jpg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        for c in 0..3 {
            assert_eq!(jpg[sof + 11 + c * 3], 0x11);
        }
        // DQT：亮度表第一项 16 按质量 95 是 2（(16 × 10 + 50) / 100）
        let dqt = jpg.windows(2).position(|w| w == [0xFF, 0xDB]).unwrap();
        assert_eq!(jpg[dqt + 5], 2);
        let back = image::load_from_memory(&jpg).unwrap();
        assert_eq!((back.width(), back.height()), (16, 16));
    }

    #[test]
    fn write_readonly_dir() {
        let t = Tmp::new();
        let d = t.te();
        let (job, base) = one(&d);
        let out = render(&d, &job).unwrap();
        // 文件夹没了：不新建
        let gone = t.0.join("gone");
        assert!(matches!(
            write_beside(&gone.join("a.png"), &out, false),
            Err(SaveFail::Dir)
        ));
        assert!(!gone.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let ro = t.0.join("ro");
            fs::create_dir(&ro).unwrap();
            fs::write(ro.join("a.png"), b"src").unwrap();
            let mode = |m| fs::set_permissions(&ro, fs::Permissions::from_mode(m)).unwrap();
            mode(0o555);
            let r = write_beside(&ro.join("a.png"), &out, false);
            mode(0o755);
            assert!(matches!(r, Err(SaveFail::Dir)));
            assert_eq!(fs::read_dir(&ro).unwrap().count(), 1);
        }
        // 文件名太长，加不了后缀：也改走另存为
        let long = format!("{}.png", "a".repeat(250));
        assert!(matches!(
            write_beside(&t.0.join(long), &out, false),
            Err(SaveFail::Dir)
        ));
        let back = load(&d, &job.id).unwrap();
        check(&render(&d, &back).unwrap(), &base, &[((2, 2), RED)]);
    }

    #[test]
    fn write_source_gone() {
        let t = Tmp::new();
        let p = write_beside(&t.0.join("a.jpg"), &img(8, 8), false)
            .ok()
            .unwrap();
        assert_eq!(p, t.0.join("a 改字.jpg"));
        assert!(p.is_file());
    }
}
