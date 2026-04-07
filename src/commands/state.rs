use crate::backend::AgentBackend;
use crate::error::Result;

pub fn run(backend: &dyn AgentBackend, session_hint: &str) -> Result<()> {
    let response = backend.state(session_hint)?;
    let line = response.trim();
    
    if line.starts_with("STATE|") {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 11 {
            println!("Terminal State:");
            println!("  Session:   {}", parts[1]);
            println!("  PID:       {}", parts[2]);
            println!("  HWND:      {}", parts[3]);
            println!("  Title:     {}", parts[4]);
            println!("  Status:    {}, {}", parts[5], parts[6]);
            println!("  PWD:       {}", parts[7]);
            println!("  Tabs:      {} (Active: {})", parts[8], parts[9]);
            println!("  Active ID: {}", parts[10]);
            return Ok(());
        }
    }
    
    println!("{}", line);
    Ok(())
}
