use base64::{engine::general_purpose::STANDARD, Engine};
use std::fmt;

/// Target for tab-specific commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabTarget {
    None,
    Index(usize),
    Id(String),
}

impl fmt::Display for TabTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TabTarget::None => Ok(()),
            TabTarget::Index(idx) => write!(f, "{}", idx),
            TabTarget::Id(id) => write!(f, "id={}", id),
        }
    }
}

impl TabTarget {
    fn to_suffix(&self) -> String {
        match self {
            TabTarget::None => String::new(),
            _ => format!("|{}", self),
        }
    }
}

/// Encode text payload as base64 for INPUT/RAW_INPUT/PASTE commands.
pub fn encode_payload(text: &str) -> String {
    STANDARD.encode(text.as_bytes())
}

/// Build a PING request.
pub fn ping() -> String {
    "PING".to_string()
}

/// Build a CAPABILITIES request.
pub fn capabilities() -> String {
    "CAPABILITIES".to_string()
}

/// Build a STATE request.
pub fn state(target: TabTarget) -> String {
    format!("STATE{}", target.to_suffix())
}

/// Build a CAPTURE_PANE request.
pub fn capture_pane(target: TabTarget) -> String {
    format!("CAPTURE_PANE{}", target.to_suffix())
}

/// Build a TAIL request.
pub fn tail(lines: usize, target: TabTarget) -> String {
    format!("TAIL|{}{}", lines, target.to_suffix())
}

/// Build a HISTORY request.
pub fn history(lines: Option<usize>, target: TabTarget) -> String {
    match lines {
        Some(l) => format!("HISTORY|{}{}", l, target.to_suffix()),
        None => format!("HISTORY{}", target.to_suffix()),
    }
}

/// Build a WAIT_FOR request.
pub fn wait_for(timeout_ms: u32, pattern: &str, target: TabTarget) -> String {
    format!("WAIT_FOR|{}|{}{}", timeout_ms, pattern, target.to_suffix())
}

/// Build a LIST_TABS request.
pub fn list_tabs() -> String {
    "LIST_TABS".to_string()
}

/// Build an INPUT request (bracketed paste).
pub fn input(from: &str, text: &str, target: TabTarget) -> String {
    format!("INPUT|{}|{}{}", from, encode_payload(text), target.to_suffix())
}

/// Build a RAW_INPUT request (direct terminal write).
pub fn raw_input(from: &str, text: &str, target: TabTarget) -> String {
    format!("RAW_INPUT|{}|{}{}", from, encode_payload(text), target.to_suffix())
}

/// Build a PASTE request.
pub fn paste(from: &str, text: &str, target: TabTarget) -> String {
    format!("PASTE|{}|{}{}", from, encode_payload(text), target.to_suffix())
}

/// Build a SEND_KEYS request.
pub fn send_keys(from: &str, keys: &str, target: TabTarget) -> String {
    format!("SEND_KEYS|{}|{}{}", from, keys, target.to_suffix())
}

/// Build a NEW_TAB request.
pub fn new_tab() -> String {
    "NEW_TAB".to_string()
}

/// Build a CLOSE_TAB request.
pub fn close_tab(target: TabTarget) -> String {
    format!("CLOSE_TAB{}", target.to_suffix())
}

/// Build a SWITCH_TAB request.
pub fn switch_tab(target: TabTarget) -> String {
    format!("SWITCH_TAB{}", target.to_suffix())
}

/// Build a FOCUS request.
pub fn focus() -> String {
    "FOCUS".to_string()
}

/// Build an AGENT_STATUS request.
pub fn agent_status() -> String {
    "AGENT_STATUS".to_string()
}

/// Build a SET_AGENT request.
pub fn set_agent(target: TabTarget, agent_type: &str) -> String {
    format!("SET_AGENT|{}|{}", target, agent_type)
}

/// Check if a response is an error.
/// Returns Some(error_code) if it starts with "ERR|".
pub fn is_error(response: &str) -> Option<String> {
    let line = response.lines().next()?.trim();
    if line.starts_with("ERR|") {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 3 {
            // ERR|session_name|code
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
    fn test_tab_target_display() {
        assert_eq!(format!("{}", TabTarget::None), "");
        assert_eq!(format!("{}", TabTarget::Index(2)), "2");
        assert_eq!(format!("{}", TabTarget::Id("t1".into())), "id=t1");
    }

    #[test]
    fn test_commands_with_target() {
        assert_eq!(state(TabTarget::None), "STATE");
        assert_eq!(state(TabTarget::Index(1)), "STATE|1");
        assert_eq!(state(TabTarget::Id("abc".into())), "STATE|id=abc");

        assert_eq!(tail(50, TabTarget::Index(0)), "TAIL|50|0");
        assert_eq!(input("cli", "hi", TabTarget::None), "INPUT|cli|aGk=");
    }

    #[test]
    fn test_is_error() {
        assert_eq!(
            is_error("ERR|sess|NOT_FOUND\n"),
            Some("NOT_FOUND".to_string())
        );
        assert_eq!(is_error("OK|sess|PONG\n"), None);
    }
}
