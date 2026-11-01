//! 真实帧回归（M23 规则 13）：`tests/fixtures/stitch/<场景>/` 下是按序号命名的 PNG 和 `expect.txt`
//! （两行：`lost=<没接上帧数>`、`height=<长图高度>`），按 scale 2 重放后必须一致。
//! 新场景的帧用 `XPASTE_LONG_DUMP` 存下来，先用 example 看结果，再写 expect.txt。

mod common;
use common::replay::replay;

#[test]
fn replays_real_captures() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/stitch");
    let mut n = 0;
    for dir in std::fs::read_dir(&root).expect("缺少回归帧目录") {
        let dir = dir.unwrap().path();
        if !dir.is_dir() {
            continue;
        }
        n += 1;
        let expect = std::fs::read_to_string(dir.join("expect.txt")).expect("缺少 expect.txt");
        let get = |k: &str| -> usize {
            expect
                .lines()
                .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
                .unwrap_or_else(|| panic!("expect.txt 缺 {k}"))
                .trim()
                .parse()
                .unwrap()
        };
        let r = replay(&dir, 2.0, |_| {});
        let name = dir.file_name().unwrap().to_string_lossy();
        assert_eq!(r.lost, get("lost"), "{name}：没接上的帧数");
        assert_eq!(r.height, get("height"), "{name}：长图高度");
    }
    assert!(n > 0, "没有回归场景");
}
