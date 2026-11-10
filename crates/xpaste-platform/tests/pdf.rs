use std::path::Path;

use xpaste_platform::pdf_text;

/// 一页、一行 Helvetica 文字的最小 PDF；交叉引用表的偏移按实际字节算。
fn tiny_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 18 Tf 20 100 Td ({text}) Tj ET");
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 144] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![];
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{o}\nendobj\n", i + 1).bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objs.len() + 1
        )
        .bytes(),
    );
    out
}

#[test]
fn pdf_text_reads_page_text() {
    let p = std::env::temp_dir().join(format!("xpaste-pdf-{}.pdf", std::process::id()));
    std::fs::write(&p, tiny_pdf("Invoice 482913")).unwrap();
    let got = pdf_text(&p, 64 * 1024);
    let short = pdf_text(&p, 7);
    std::fs::remove_file(&p).unwrap();
    // Windows 没有系统自带的读法，恒为 None（M05 规则 2）
    if !cfg!(target_os = "macos") {
        assert!(got.is_none());
        return;
    }
    assert!(got.unwrap().contains("Invoice 482913"));
    // 不超过上限
    assert_eq!(short.as_deref(), Some("Invoice"));
    assert!(pdf_text(Path::new("/nonexistent/x.pdf"), 1024).is_none());
    // 不是 PDF 的文件
    let txt = std::env::temp_dir().join(format!("xpaste-notpdf-{}.pdf", std::process::id()));
    std::fs::write(&txt, b"hello").unwrap();
    let r = pdf_text(&txt, 1024);
    std::fs::remove_file(&txt).unwrap();
    assert!(r.is_none());
}
