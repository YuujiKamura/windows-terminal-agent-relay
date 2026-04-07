//! sample-all: Automatically cycle through agent lifecycles and capture real buffers.
//!
//! For each agent (claude, gemini, codex):
//!   1. Capture SHELL_IDLE
//!   2. Launch agent → capture AGENT_STARTING
//!   3. Wait for ready → capture AGENT_READY
//!   4. Send a trivial task → capture AGENT_WORKING
//!   5. Wait for done → capture AGENT_DONE
//!   6. Stop agent → back to shell
//!
//! Also captures SHELL_BUSY via a simple shell command.

use crate::backend::AgentBackend;
use crate::error::{AgentCtlError, Result};
use crate::librarian;
use crate::pipe;
use crate::protocol::{self, TabTarget};
use crate::session;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const AGENTS: &[(&str, &str)] = &[
    ("claude", "echo hello"),
    ("gemini", "echo hello"),
    ("codex", "echo hello"),
];

struct CapturedSample {
    name: String,
    expected: String,
    buffer: String,
    librarian_says: String,
}

pub fn run(
    backend: &dyn AgentBackend,
    session_hint: &str,
    agents: &[String],
    output: Option<&str>,
) -> Result<()> {
    let agent_list: Vec<(&str, &str)> = if agents.is_empty() {
        AGENTS.to_vec()
    } else {
        agents
            .iter()
            .map(|a| {
                let task = AGENTS
                    .iter()
                    .find(|(name, _)| *name == a.as_str())
                    .map(|(_, t)| *t)
                    .unwrap_or("echo hello");
                (a.as_str(), task)
            })
            .collect()
    };

    let fixture_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("buffers_real.toml")
        });

    let mut samples: Vec<CapturedSample> = Vec::new();

    // Verify session is alive
    let s = session::find_session(session_hint)?;
    eprintln!("[sample-all] Using session: {}", s.session_name);

    // ── SHELL_IDLE: should already be at shell ──
    eprintln!("\n[sample-all] ── Capturing SHELL_IDLE ──");
    if let Some(sample) = capture(backend, session_hint, "shell_idle", "SHELL_IDLE")? {
        samples.push(sample);
    }

    // ── SHELL_BUSY: run a slow-ish command ──
    eprintln!("\n[sample-all] ── Capturing SHELL_BUSY ──");
    let busy_msg = protocol::raw_input("agent-ctl", "ping -n 3 127.0.0.1\r", TabTarget::None);
    let _ = pipe::send_pipe_message(&s.pipe_path, &busy_msg);
    std::thread::sleep(Duration::from_secs(1));
    if let Some(sample) = capture(backend, session_hint, "shell_busy", "SHELL_BUSY")? {
        samples.push(sample);
    }
    // Wait for ping to finish
    wait_for_state(backend, session_hint, &["SHELL_IDLE"], 15)?;

    // ── Per-agent lifecycle ──
    for (agent, task) in &agent_list {
        eprintln!("\n[sample-all] ══════ Agent: {} ══════", agent);

        // Launch agent
        eprintln!("[sample-all] Launching {}...", agent);
        let launch_msg = protocol::raw_input("agent-ctl", &format!("bash\r"), TabTarget::None);
        let _ = pipe::send_pipe_message(&s.pipe_path, &launch_msg);
        std::thread::sleep(Duration::from_secs(1));

        let agent_cmd = protocol::raw_input("agent-ctl", &format!("{}\r", agent), TabTarget::None);
        let _ = pipe::send_pipe_message(&s.pipe_path, &agent_cmd);

        // ── AGENT_STARTING: capture quickly before it finishes loading ──
        std::thread::sleep(Duration::from_secs(2));
        eprintln!("[sample-all] ── Capturing AGENT_STARTING ──");
        if let Some(sample) = capture(
            backend,
            session_hint,
            &format!("{}_starting", agent),
            "AGENT_STARTING",
        )? {
            samples.push(sample);
        }

        // ── AGENT_READY: wait for agent to be ready ──
        eprintln!("[sample-all] Waiting for AGENT_READY...");
        wait_for_state(
            backend,
            session_hint,
            &["AGENT_READY", "AGENT_DONE"],
            60,
        )?;
        eprintln!("[sample-all] ── Capturing AGENT_READY ──");
        if let Some(sample) = capture(
            backend,
            session_hint,
            &format!("{}_ready", agent),
            "AGENT_READY",
        )? {
            samples.push(sample);
        }

        // ── AGENT_WORKING: send task and capture quickly ──
        eprintln!("[sample-all] Sending task: {}", task);
        backend.send(session_hint, task)?;
        std::thread::sleep(Duration::from_secs(2));
        eprintln!("[sample-all] ── Capturing AGENT_WORKING ──");
        if let Some(sample) = capture(
            backend,
            session_hint,
            &format!("{}_working", agent),
            "AGENT_WORKING",
        )? {
            samples.push(sample);
        }

        // ── AGENT_DONE: wait for completion ──
        eprintln!("[sample-all] Waiting for AGENT_DONE...");
        wait_for_state(
            backend,
            session_hint,
            &["AGENT_DONE", "AGENT_READY", "SHELL_IDLE"],
            120,
        )?;
        eprintln!("[sample-all] ── Capturing AGENT_DONE ──");
        if let Some(sample) = capture(
            backend,
            session_hint,
            &format!("{}_done", agent),
            "AGENT_DONE",
        )? {
            samples.push(sample);
        }

        // ── Stop agent ──
        eprintln!("[sample-all] Stopping {}...", agent);
        backend.stop(session_hint, agent)?;
        std::thread::sleep(Duration::from_secs(2));

        // Wait for shell to return
        wait_for_state(backend, session_hint, &["SHELL_IDLE"], 30)?;
        eprintln!("[sample-all] {} cycle complete.", agent);
    }

    // ── Write results ──
    eprintln!("\n[sample-all] ══════ Writing {} samples ══════", samples.len());
    write_fixtures(&fixture_path, &samples)?;

    // ── Summary ──
    eprintln!("\n══════════════════════════════════════════");
    eprintln!("  CAPTURED: {} samples → {}", samples.len(), fixture_path.display());
    eprintln!("══════════════════════════════════════════");
    let mut match_count = 0;
    let mut mismatch_count = 0;
    for s in &samples {
        let icon = if s.expected == s.librarian_says {
            match_count += 1;
            "✓"
        } else {
            mismatch_count += 1;
            "✗"
        };
        eprintln!(
            "  {} {:30} expected={:20} librarian={}",
            icon, s.name, s.expected, s.librarian_says
        );
    }
    eprintln!("──────────────────────────────────────────");
    eprintln!(
        "  Match: {}/{}  Mismatch: {}",
        match_count,
        samples.len(),
        mismatch_count
    );
    eprintln!("══════════════════════════════════════════");

    println!(
        "SAMPLE_ALL|{}|match={}|mismatch={}|file={}",
        samples.len(),
        match_count,
        mismatch_count,
        fixture_path.display()
    );

    Ok(())
}

/// Capture current buffer, run Librarian, return a sample.
fn capture(
    backend: &dyn AgentBackend,
    session_hint: &str,
    name: &str,
    expected: &str,
) -> Result<Option<CapturedSample>> {
    let raw_tail = backend.read(session_hint, 20, None)?;
    let buffer = if let Some(idx) = raw_tail.find('\n') {
        raw_tail[idx + 1..].to_string()
    } else {
        raw_tail
    };

    match librarian::judge(&buffer) {
        Ok(judgment) => {
            let librarian_says = judgment.state.as_str().to_string();
            eprintln!(
                "  captured: {} (librarian={}, expected={})",
                name, librarian_says, expected
            );
            Ok(Some(CapturedSample {
                name: name.to_string(),
                expected: expected.to_string(),
                buffer,
                librarian_says,
            }))
        }
        Err(e) => {
            eprintln!("  WARNING: Librarian failed for {}: {}", name, e);
            Ok(Some(CapturedSample {
                name: name.to_string(),
                expected: expected.to_string(),
                buffer,
                librarian_says: "ERROR".to_string(),
            }))
        }
    }
}

/// Poll Librarian until one of the target states is reached.
fn wait_for_state(
    backend: &dyn AgentBackend,
    session_hint: &str,
    target_states: &[&str],
    timeout_secs: u64,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let poll = Duration::from_secs(3);

    loop {
        if Instant::now() > deadline {
            eprintln!(
                "  TIMEOUT waiting for {:?} after {}s",
                target_states, timeout_secs
            );
            return Err(AgentCtlError::WaitTimeout(timeout_secs));
        }

        let raw_tail = backend.read(session_hint, 20, None)?;
        let buffer = if let Some(idx) = raw_tail.find('\n') {
            &raw_tail[idx + 1..]
        } else {
            &raw_tail
        };

        if let Ok(j) = librarian::judge(buffer) {
            let state = j.state.as_str();
            if target_states.contains(&state) {
                return Ok(());
            }
            eprint!("  waiting... (current={})\r", state);
        }

        std::thread::sleep(poll);
    }
}

fn write_fixtures(path: &PathBuf, samples: &[CapturedSample]) -> Result<()> {
    let mut f = fs::File::create(path)
        .map_err(|e| AgentCtlError::Other(format!("Failed to create {}: {}", path.display(), e)))?;

    writeln!(f, "# Auto-captured real terminal buffers")
        .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
    writeln!(
        f,
        "# Generated by: agent-ctl sample-all"
    )
    .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
    writeln!(f, "# Date: {}", chrono_now())
        .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
    writeln!(f)
        .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;

    for s in samples {
        writeln!(f, "[[case]]")
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        writeln!(f, "name = \"{}\"", s.name)
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        writeln!(f, "expected = \"{}\"", s.expected)
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        // If Librarian disagreed, note it
        if s.expected != s.librarian_says {
            writeln!(f, "# NOTE: librarian judged {} at capture time", s.librarian_says)
                .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        }
        writeln!(f, "buffer = \"\"\"")
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        // Write buffer lines
        for line in s.buffer.lines() {
            writeln!(f, "{}", line)
                .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        }
        writeln!(f, "\"\"\"")
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
        writeln!(f)
            .map_err(|e| AgentCtlError::Other(format!("write error: {}", e)))?;
    }

    Ok(())
}

fn chrono_now() -> String {
    // Simple timestamp without chrono dependency
    use std::time::SystemTime;
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}", secs)
}
