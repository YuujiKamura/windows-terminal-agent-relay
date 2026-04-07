use crate::backend::AgentBackend;
use crate::error::Result;

pub fn run(
    backend: &dyn AgentBackend,
    session_hint: &str,
    pattern: &str,
    timeout_ms: u32,
    tab: Option<&str>,
) -> Result<()> {
    let response = backend.wait_for(session_hint, pattern, timeout_ms, tab)?;
    println!("{}", response.trim());
    Ok(())
}
