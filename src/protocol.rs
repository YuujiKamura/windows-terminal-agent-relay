use base64::{engine::general_purpose::STANDARD, Engine};

/// Encode text payload as base64 for INPUT/RAW_INPUT commands.
pub fn encode_payload(text: &str) -> String {
    STANDARD.encode(text.as_bytes())
}

/// Build a PING request.
pub fn ping() -> String {
    "PING".to_string()
}

/// Build a STATE request, optionally for a specific tab.
pub fn state(tab_index: Option<usize>) -> String {
    match tab_index {
        Some(idx) => format!("STATE|{}", idx),
        None => "STATE".to_string(),
    }
}

/// Build a TAIL request.
pub fn tail(lines: usize) -> String {
    format!("TAIL|{}", lines)
}

/// Build a TAIL request for a specific tab.
pub fn tail_tab(lines: usize, tab_index: usize) -> String {
    format!("TAIL|{}|{}", lines, tab_index)
}

/// Build a LIST_TABS request.
pub fn list_tabs() -> String {
    "LIST_TABS".to_string()
}

/// Build an INPUT request (bracketed paste).
pub fn input(from: &str, text: &str) -> String {
    format!("INPUT|{}|{}", from, encode_payload(text))
}

/// Build a RAW_INPUT request (direct terminal write).
pub fn raw_input(from: &str, text: &str) -> String {
    format!("RAW_INPUT|{}|{}", from, encode_payload(text))
}

/// Build a NEW_TAB request.
pub fn new_tab() -> String {
    "NEW_TAB".to_string()
}

/// Build a CLOSE_TAB request.
pub fn close_tab(index: Option<usize>) -> String {
    match index {
        Some(idx) => format!("CLOSE_TAB|{}", idx),
        None => "CLOSE_TAB".to_string(),
    }
}

/// Build a SWITCH_TAB request.
pub fn switch_tab(index: usize) -> String {
    format!("SWITCH_TAB|{}", index)
}

/// Build a FOCUS request.
pub fn focus() -> String {
    "FOCUS".to_string()
}

/// Check if a response is an error.
pub fn is_error(response: &str) -> Option<String> {
    let line = response.lines().next()?.trim();
    if line.starts_with("ERR|") {
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() >= 3 {
            return Some(parts[2].to_string());
        }
        return Some(line.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_payload() {
        let encoded = encode_payload("hello");
        assert_eq!(encoded, "aGVsbG8=");
    }

    #[test]
    fn test_input_message() {
        let msg = input("claude", "echo hello");
        assert!(msg.starts_with("INPUT|claude|"));
    }

    #[test]
    fn test_raw_input_message() {
        let msg = raw_input("claude", "\r");
        assert!(msg.starts_with("RAW_INPUT|claude|"));
    }

    #[test]
    fn test_raw_input_ctrl_c() {
        let msg = raw_input("agent-ctl", "\x03");
        assert!(msg.starts_with("RAW_INPUT|agent-ctl|"));
        // \x03 base64-encoded is "Aw=="
        assert!(msg.ends_with("Aw=="), "Expected base64 of \\x03, got: {}", msg);
    }

    #[test]
    fn test_raw_input_ctrl_d() {
        let msg = raw_input("agent-ctl", "\x04");
        assert!(msg.starts_with("RAW_INPUT|agent-ctl|"));
        // \x04 base64-encoded is "BA=="
        assert!(msg.ends_with("BA=="), "Expected base64 of \\x04, got: {}", msg);
    }

    #[test]
    fn test_raw_input_ctrl_z() {
        let msg = raw_input("agent-ctl", "\x1a");
        assert!(msg.starts_with("RAW_INPUT|agent-ctl|"));
        // \x1a base64-encoded is "Gg=="
        assert!(msg.ends_with("Gg=="), "Expected base64 of \\x1a, got: {}", msg);
    }

    #[test]
    fn test_input_cjk_message() {
        // CJK text should be properly base64-encoded
        let cjk = "あいうえおかきくけこ";
        let msg = input("agent-ctl", cjk);
        assert!(msg.starts_with("INPUT|agent-ctl|"));
        // Verify round-trip: extract base64 payload and decode
        let payload_b64 = msg.strip_prefix("INPUT|agent-ctl|").unwrap();
        let decoded = STANDARD.decode(payload_b64).unwrap();
        let decoded_str = String::from_utf8(decoded).unwrap();
        assert_eq!(decoded_str, cjk);
    }

    #[test]
    fn test_input_long_cjk_message() {
        // Long CJK text (90+ chars) should encode/decode correctly
        let long_cjk = "これは非常に長い日本語テキストです。表示テストのため送信しています。全角文字の幅計算が正しく行われているかを確認します。";
        let msg = input("agent-ctl", long_cjk);
        let payload_b64 = msg.strip_prefix("INPUT|agent-ctl|").unwrap();
        let decoded = STANDARD.decode(payload_b64).unwrap();
        let decoded_str = String::from_utf8(decoded).unwrap();
        assert_eq!(decoded_str, long_cjk);
    }

    #[test]
    fn test_raw_input_cjk() {
        let cjk = "漢字テスト";
        let msg = raw_input("agent-ctl", cjk);
        let payload_b64 = msg.strip_prefix("RAW_INPUT|agent-ctl|").unwrap();
        let decoded = STANDARD.decode(payload_b64).unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), cjk);
    }

    #[test]
    fn test_is_error() {
        assert_eq!(
            is_error("ERR|session|unknown\n"),
            Some("unknown".to_string())
        );
        assert_eq!(is_error("PONG|session|123\n"), None);
    }
}
