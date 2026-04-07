use crate::error::Result;
use crate::session;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn run() -> Result<()> {
    let mut removed = 0;
    for dir in session_dirs() {
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
            let Some(pid) = map.get("pid").and_then(|p| p.parse::<u32>().ok()) else {
                continue;
            };
            if !session::is_process_alive(pid) {
                match std::fs::remove_file(&path) {
                    Ok(_) => {
                        eprintln!("[clean] Removed {} (pid {} dead)", path.display(), pid);
                        removed += 1;
                    }
                    Err(e) => {
                        eprintln!("[clean] Failed to remove {}: {}", path.display(), e);
                    }
                }
            }
        }
    }

    eprintln!("[clean] Removed {} dead session file(s).", removed);
    Ok(())
}

fn session_dirs() -> Vec<PathBuf> {
    let local_app = std::env::var("LOCALAPPDATA").unwrap_or_default();
    if local_app.is_empty() {
        return vec![];
    }
    let base = PathBuf::from(&local_app);
    vec![
        base.join("ghostty/control-plane/winui3/sessions"),
        base.join("WindowsTerminal/control-plane/winui3/sessions"),
        base.join("Packages/WindowsTerminalDev_8wekyb3d8bbwe/LocalCache/Local/WindowsTerminal/control-plane/winui3/sessions"),
        base.join("Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalCache/Local/WindowsTerminal/control-plane/winui3/sessions"),
    ]
}
