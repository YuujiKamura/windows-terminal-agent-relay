use crate::backend::AgentBackend;
use crate::error::Result;

pub fn run(backend: &dyn AgentBackend, session_hint: &str, text: &str, tab: Option<&str>) -> Result<()> {
    backend.paste(session_hint, text, tab)?;
    println!("PASTE|OK");
    Ok(())
}
