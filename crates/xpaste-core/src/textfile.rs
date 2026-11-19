//! 文本文件的识别与解码：UTF-8、带 BOM 的 UTF-16、GBK。

/// 文件大于它就不读。
pub const TEXT_FILE_MAX_BYTES: u64 = 1_048_576;

/// 前 8 KB 里有 NUL 字节就算二进制；带 BOM 的 UTF-16 不算（ASCII 字符里本来就有 0x00）。
pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    bytes[..bytes.len().min(8192)].contains(&0)
}

/// 依次试：UTF-8（去掉 BOM）、带 BOM 的 UTF-16 LE / BE、GBK；都不行返回 None。
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE
            .decode_without_bom_handling_and_without_replacement(rest)
            .map(|s| s.into_owned());
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE
            .decode_without_bom_handling_and_without_replacement(rest)
            .map(|s| s.into_owned());
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Some(s.to_owned());
    }
    encoding_rs::GBK
        .decode_without_bom_handling_and_without_replacement(bytes)
        .map(|s| s.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_plain_and_bom() {
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend("日志 ok".as_bytes());
        assert_eq!(
            decode_text("日志 ok".as_bytes()).as_deref(),
            Some("日志 ok")
        );
        assert_eq!(decode_text(&with_bom).as_deref(), Some("日志 ok"));
    }

    #[test]
    fn utf16_with_bom() {
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("中文".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("中文".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert_eq!(decode_text(&le).as_deref(), Some("中文"));
        assert_eq!(decode_text(&be).as_deref(), Some("中文"));
    }

    #[test]
    fn gbk_log() {
        let (b, _, _) = encoding_rs::GBK.encode("订单列表白屏");
        assert_eq!(decode_text(&b).as_deref(), Some("订单列表白屏"));
    }

    #[test]
    fn binary_detected() {
        assert!(looks_binary(&[0x50, 0x4B, 0x03, 0x04, 0x00, 0x01]));
        assert!(!looks_binary(b"plain text"));
        let mut late = vec![b'a'; 10_000];
        late[9000] = 0;
        assert!(!looks_binary(&late));
        // 带 BOM 的 UTF-16 里的 ASCII 字符有 0x00，不算二进制
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("log ok".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("log ok".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        assert!(!looks_binary(&le));
        assert!(!looks_binary(&be));
    }

    #[test]
    fn undecodable_is_none() {
        assert_eq!(decode_text(&[0xFF, 0xFF, 0xFF, 0x81]), None);
    }
}
