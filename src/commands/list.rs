use crate::backend::AgentBackend;
use crate::error::Result;
use crate::session::is_process_alive;

pub fn run(backend: &dyn AgentBackend, alive_only: bool, json: bool) -> Result<()> {
    let sessions = backend.list()?;

    if json {
        let entries: Vec<serde_json::Value> = sessions
            .iter()
            .filter(|s| !alive_only || is_process_alive(s.pid))
            .map(|s| {
                serde_json::json!({
                    "session_name": s.session_name,
                    "pid": s.pid,
                    "pipe_name": s.pipe_name,
                    "alive": is_process_alive(s.pid),
                    "terminal": s.terminal_type,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries).unwrap());
    } else {
        if sessions.is_empty() {
            println!("No control plane sessions found.");
            return Ok(());
        }
        for s in &sessions {
            let alive = is_process_alive(s.pid);
            if alive_only && !alive {
                continue;
            }
            let status = if alive { "ALIVE" } else { "DEAD" };
            println!(
                "{} | {:<15} | session={:<20} | pid={:<6} | pipe={}",
                status, format!("{:?}", s.terminal_type), s.session_name, s.pid, s.pipe_name
            );
        }
    }
    Ok(())
}
