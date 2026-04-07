use crate::error::{AgentCtlError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Terminal type associated with the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalType {
    Ghostty,
    WindowsTerminal,
    Unknown,
}

/// Parsed session file info.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_name: String,
    pub safe_session_name: String,
    pub pid: u32,
    pub hwnd: String,
    pub pipe_name: String,
    pub pipe_path: String,
    pub log_file: String,
    pub terminal_type: TerminalType,
}

impl SessionInfo {
    fn from_map(map: &HashMap<String, String>, terminal_type: TerminalType) -> Option<Self> {
        let pipe_path = map.get("pipe_path")?.clone();
        let pipe_name = map
            .get("pipe_name")
            .cloned()
            .unwrap_or_else(|| {
                // Derive pipe_name from pipe_path: \\.\pipe\NAME -> NAME
                pipe_path
                    .strip_prefix("\\\\.\\pipe\\")
                    .unwrap_or(&pipe_path)
                    .to_string()
            });
        Some(Self {
            session_name: map.get("session_name")?.clone(),
            safe_session_name: map.get("safe_session_name").cloned().unwrap_or_default(),
            pid: map.get("pid")?.parse().ok()?,
            hwnd: map.get("hwnd").cloned().unwrap_or_default(),
            pipe_name,
            pipe_path,
            log_file: map.get("log_file").cloned().unwrap_or_default(),
            terminal_type,
        })
    }
}

/// Get all session search directories.
fn session_dirs() -> Vec<(PathBuf, TerminalType)> {
    let local_app = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if local_app.is_empty() {
        return vec![];
    }
    let base = PathBuf::from(&local_app);
    vec![
        (base.join("ghostty/control-plane/winui3/sessions"), TerminalType::Ghostty),
        (base.join("WindowsTerminal/control-plane/winui3/sessions"), TerminalType::WindowsTerminal),
        (base.join("Packages/WindowsTerminalDev_8wekyb3d8bbwe/LocalCache/Local/WindowsTerminal/control-plane/winui3/sessions"), TerminalType::WindowsTerminal),
        (base.join("Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalCache/Local/WindowsTerminal/control-plane/winui3/sessions"), TerminalType::WindowsTerminal),
    ]
}

/// Read all .session files and parse them.
pub fn discover_sessions() -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for (dir, term_type) in session_dirs() {
        if !dir.exists() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("session") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let mut map = HashMap::new();
            for line in content.lines() {
                if let Some((key, value)) = line.split_once('=') {
                    map.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
            if let Some(info) = SessionInfo::from_map(&map, term_type) {
                sessions.push(info);
            }
        }
    }
    sessions
}

/// Check if a process with given PID is alive.
pub fn is_process_alive(pid: u32) -> bool {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);
        match handle {
            Ok(h) => {
                let _ = windows::Win32::Foundation::CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

/// Find a session by hint (substring match on session_name), returning first alive match.
/// If hint is empty, return first alive session.
pub fn find_session(hint: &str) -> Result<SessionInfo> {
    let sessions = discover_sessions();
    if sessions.is_empty() {
        return Err(AgentCtlError::NoSessions);
    }

    let candidates: Vec<_> = if hint.is_empty() {
        sessions
    } else {
        sessions
            .into_iter()
            .filter(|s| s.session_name.contains(hint) || s.safe_session_name.contains(hint))
            .collect()
    };

    if candidates.is_empty() {
        return Err(AgentCtlError::SessionNotFound(hint.to_string()));
    }

    for s in &candidates {
        if is_process_alive(s.pid) {
            return Ok(s.clone());
        }
    }

    Err(AgentCtlError::SessionDead(candidates[0].pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_from_map() {
        let mut map = HashMap::new();
        map.insert("session_name".into(), "test-session".into());
        map.insert("safe_session_name".into(), "test-session".into());
        map.insert("pid".into(), "12345".into());
        map.insert("hwnd".into(), "0x1234".into());
        map.insert(
            "pipe_name".into(),
            "windows-terminal-winui3-test-session-12345".into(),
        );
        map.insert(
            "pipe_path".into(),
            "\\\\.\\pipe\\windows-terminal-winui3-test-session-12345".into(),
        );
        map.insert("log_file".into(), "C:\\logs\\test.log".into());

        let info = SessionInfo::from_map(&map, TerminalType::WindowsTerminal).unwrap();
        assert_eq!(info.session_name, "test-session");
        assert_eq!(info.pid, 12345);
        assert_eq!(info.terminal_type, TerminalType::WindowsTerminal);
    }
}
