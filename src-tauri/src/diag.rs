//! 导出日志（M15“日志”、M17 规则 2）：本应用日志、系统留下的崩溃报告和版本信息打成一个 zip，
//! 放进“下载”并在访达 / 资源管理器里选中，用户直接拖进聊天窗口发给开发者。

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;
use zip::write::SimpleFileOptions;

use crate::error::AppError;
use crate::state::AppState;

/// 崩溃报告只收最近 30 天的
const CRASH_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);

/// 导出并在文件管理器里选中，返回文件名。
pub fn export(app: &AppHandle) -> Result<String, AppError> {
    let ver = app.package_info().version.to_string();
    let name = format!(
        "xPaste-日志-{ver}-{}.zip",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let path = app.path().download_dir()?.join(&name);
    let mut files = list(
        &app.state::<AppState>().data_dir.join("logs"),
        "logs",
        |_| true,
    );
    files.extend(crash_reports(app));
    let crashes = files.iter().filter(|f| f.0.starts_with("crash/")).count();
    let info = format!(
        "xPaste {ver}\n平台 {} {}\n系统版本 {}\n导出时间 {}\n",
        xpaste_platform::OS,
        std::env::consts::ARCH,
        os_version(),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S %:z")
    );
    let bytes = write_zip(&path, &files, &info).inspect_err(|_| drop(fs::remove_file(&path)))?;
    log::info!("导出日志 {bytes} 字节，崩溃报告 {crashes} 份");
    app.opener().reveal_item_in_dir(&path)?;
    Ok(name)
}

/// `dir` 下名字满足 `keep` 的文件，包内路径为 `<prefix>/<文件名>`；目录读不了时为空。
fn list(dir: &Path, prefix: &str, keep: impl Fn(&str) -> bool) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return vec![];
    };
    rd.flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            keep(&name).then(|| (format!("{prefix}/{name}"), e.path()))
        })
        .collect()
}

fn recent(p: &Path) -> bool {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < CRASH_MAX_AGE)
}

/// macOS：`~/Library/Logs/DiagnosticReports/xpaste-*.ips`（含 ObjC 异常这类 panic 钩子抓不到的崩溃）。
#[cfg(target_os = "macos")]
fn crash_reports(app: &AppHandle) -> Vec<(String, PathBuf)> {
    let Ok(home) = app.path().home_dir() else {
        return vec![];
    };
    let mut v = list(&home.join("Library/Logs/DiagnosticReports"), "crash", |n| {
        n.to_ascii_lowercase().starts_with("xpaste")
    });
    v.retain(|f| recent(&f.1));
    v
}

/// Windows：错误报告目录里 `AppCrash_xpaste.exe_*` 各自的 `Report.wer`，包内改名为 `<目录名>.wer`。
#[cfg(windows)]
fn crash_reports(app: &AppHandle) -> Vec<(String, PathBuf)> {
    let Ok(local) = app.path().local_data_dir() else {
        return vec![];
    };
    let wer = local.join("Microsoft/Windows/WER");
    ["ReportArchive", "ReportQueue"]
        .iter()
        .filter_map(|d| fs::read_dir(wer.join(d)).ok())
        .flat_map(|rd| rd.flatten())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let report = e.path().join("Report.wer");
            (name.to_ascii_lowercase().starts_with("appcrash_xpaste") && recent(&report))
                .then(|| (format!("crash/{name}.wer"), report))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn os_version() -> String {
    xpaste_platform::macos_version().map_or("未知".into(), |v| format!("macOS {v}"))
}

#[cfg(windows)]
fn os_version() -> String {
    format!("Windows build {}", xpaste_platform::windows_build())
}

/// 写 zip，返回文件大小。读不了的单个文件（被占用、刚被删）跳过，不让整次导出失败。
fn write_zip(path: &Path, files: &[(String, PathBuf)], info: &str) -> io::Result<u64> {
    let mut z = zip::ZipWriter::new(File::create(path)?);
    let opt = SimpleFileOptions::default();
    z.start_file("info.txt", opt)?;
    z.write_all(info.as_bytes())?;
    for (name, src) in files {
        let Ok(mut f) = File::open(src) else {
            continue;
        };
        z.start_file(name.as_str(), opt)?;
        io::copy(&mut f, &mut z)?;
    }
    Ok(z.finish()?.metadata()?.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_has_info_and_files_and_skips_unreadable() {
        let dir = std::env::temp_dir().join(format!("xpaste-diag-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("xPaste.log");
        fs::write(&log, "hello").unwrap();
        let out = dir.join("out.zip");
        let files = [
            ("logs/xPaste.log".to_string(), log),
            ("crash/gone.ips".to_string(), dir.join("missing")),
        ];
        assert!(write_zip(&out, &files, "info").unwrap() > 0);
        let mut z = zip::ZipArchive::new(File::open(&out).unwrap()).unwrap();
        let mut names: Vec<_> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["info.txt", "logs/xPaste.log"]);
        let mut s = String::new();
        io::Read::read_to_string(&mut z.by_name("logs/xPaste.log").unwrap(), &mut s).unwrap();
        assert_eq!(s, "hello");
        fs::remove_dir_all(&dir).unwrap();
    }
}
