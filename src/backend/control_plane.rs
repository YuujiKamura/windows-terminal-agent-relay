use crate::backend::AgentBackend;
use crate::error::{AgentCtlError, Result};
#[cfg(feature = "librarian")]
use crate::librarian::AgentState;
use crate::pipe;
use crate::protocol::{self, TabTarget};
use crate::session::{self, SessionInfo};
use std::time::{Duration, Instant};

pub struct ControlPlaneBackend;

impl AgentBackend for ControlPlaneBackend {
    fn list(&self) -> Result<Vec<SessionInfo>> {
        Ok(session::discover_sessions())
    }

    fn send(&self, session_hint: &str, text: &str) -> Result<()> {
        let s = session::find_session(session_hint)?;
        // Step 1: Send text via INPUT (bracketed paste)
        let msg = protocol::input("agent-ctl", text, TabTarget::None);
        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        // Step 2: Wait for TUI to process INPUT before sending Enter
        std::thread::sleep(Duration::from_secs(1));
        // Step 3: Send Enter via RAW_INPUT (separate from INPUT so TUI treats it as submit)
        let enter_msg = protocol::raw_input("agent-ctl", "\r", TabTarget::None);
        let response2 = pipe::send_pipe_message(&s.pipe_path, &enter_msg)?;
        if let Some(err) = protocol::is_error(&response2) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(())
    }

    fn read(&self, session_hint: &str, lines: usize, tab_index: Option<usize>) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let target = match tab_index {
            Some(idx) => TabTarget::Index(idx),
            None => TabTarget::None,
        };
        let msg = protocol::tail(lines, target);
        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(response)
    }

    #[cfg(feature = "librarian")]
    fn wait(&self, session_hint: &str, timeout_secs: u64, auto_approve: bool) -> Result<()> {
        let s = session::find_session(session_hint)?;
        let deadline = Instant::now() + Duration::from_secs(timeout_secs);
        let poll_interval = Duration::from_secs(3);
        let mut approval_sent = false;
        let mut saw_working = false;
        let mut consecutive_done = 0u32;

        loop {
            if Instant::now() > deadline {
                return Err(AgentCtlError::WaitTimeout(timeout_secs));
            }

            let tail_resp = match pipe::send_pipe_message(&s.pipe_path, &protocol::tail(20, TabTarget::None)) {
                Ok(r) => r,
                Err(_) => { std::thread::sleep(poll_interval); continue; }
            };
            let buffer = tail_resp.splitn(2, '\n').nth(1).unwrap_or("");

            let judgment = match crate::librarian::judge(buffer) {
                Ok(j) => j,
                Err(e) => { eprintln!("[wait] librarian error: {e}"); std::thread::sleep(poll_interval); continue; }
            };

            eprintln!("[wait] state={}", judgment.state.as_str());
            for l in judgment.context(3) {
                eprintln!("[wait]   {}", l);
            }

            match judgment.state {
                AgentState::AgentApproval => {
                    saw_working = true;
                    if !auto_approve {
                        return Err(AgentCtlError::Other("AGENT_APPROVAL but auto_approve disabled".into()));
                    }
                    if !approval_sent {
                        eprintln!("[wait] AGENT_APPROVAL detected, sending approval...");
                        let msg = protocol::raw_input("agent-ctl", "1", TabTarget::None);
                        let _ = pipe::send_pipe_message(&s.pipe_path, &msg);
                        approval_sent = true;
                    }
                }
                AgentState::AgentDone => {
                    consecutive_done += 1;
                    if saw_working || consecutive_done >= 2 {
                        eprintln!("[wait] AGENT_DONE");
                        return Ok(());
                    }
                    eprintln!("[wait] AGENT_DONE (waiting for WORKING first, consecutive={}...)", consecutive_done);
                }
                AgentState::ShellIdle => {
                    eprintln!("[wait] SHELL_IDLE — agent not running");
                    return Ok(());
                }
                AgentState::AgentReady => {
                    consecutive_done += 1;
                    if saw_working || consecutive_done >= 2 {
                        eprintln!("[wait] AGENT_READY — task cycle complete");
                        return Ok(());
                    }
                    eprintln!("[wait] AGENT_READY (waiting for WORKING first, consecutive={}...)", consecutive_done);
                }
                AgentState::AgentInterrupted => {
                    return Err(AgentCtlError::Other("Agent was interrupted".into()));
                }
                AgentState::AgentError => {
                    return Err(AgentCtlError::Other("Agent error/crash".into()));
                }
                AgentState::AgentWorking | AgentState::AgentStarting | AgentState::ShellBusy => {
                    saw_working = true;
                    consecutive_done = 0;
                    approval_sent = false;
                }
                AgentState::Unknown => {
                    eprintln!("[wait] UNKNOWN state, continuing...");
                }
            }

            std::thread::sleep(poll_interval);
        }
    }

    #[cfg(not(feature = "librarian"))]
    fn wait(&self, _session_hint: &str, _timeout_secs: u64, _auto_approve: bool) -> Result<()> {
        Err(AgentCtlError::Other("librarian feature not enabled".into()))
    }

    fn approve(&self, session_hint: &str) -> Result<()> {
        let s = session::find_session(session_hint)?;
        let msg = protocol::raw_input("agent-ctl", "y\r", TabTarget::None);
        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(())
    }

    fn tab(&self, session_hint: &str, action: &str, index: Option<usize>) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let target = match index {
            Some(idx) => TabTarget::Index(idx),
            None => TabTarget::None,
        };
        let msg = match action {
            "new" => protocol::new_tab(),
            "switch" => {
                if matches!(target, TabTarget::None) {
                    return Err(AgentCtlError::Other("tab switch requires an index".into()));
                }
                protocol::switch_tab(target)
            }
            "close" => protocol::close_tab(target),
            "list" => protocol::list_tabs(),
            other => return Err(AgentCtlError::Other(format!("Unknown tab action: {}", other))),
        };

        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(response)
    }

    #[cfg(feature = "librarian")]
    fn launch(&self, session_hint: &str, agent_type: &str, prompt: Option<&str>) -> Result<()> {
        let s = session::find_session(session_hint)?;
        let agent_cmd = match agent_type {
            "claude" => "claude",
            "gemini" => "gemini",
            "codex" => "codex",
            other => other,
        };
        let launch_msg = protocol::raw_input("agent-ctl", &format!("{}\r", agent_cmd), TabTarget::None);
        let _ = pipe::send_pipe_message(&s.pipe_path, &launch_msg);

        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if Instant::now() > deadline {
                eprintln!("[launch] Timeout waiting for agent ready");
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
            if let Ok(tail) = pipe::send_pipe_message(&s.pipe_path, &protocol::tail(10, TabTarget::None)) {
                let buf = tail.splitn(2, '\n').nth(1).unwrap_or("");
                let last = buf.lines().last().unwrap_or("");
                eprintln!("[launch] ... {}", last);
                if let Some((state, _)) = crate::librarian::judge_by_score(buf) {
                    if matches!(state, AgentState::AgentReady | AgentState::AgentDone) {
                        break;
                    }
                }
            }
        }

        if let Some(prompt_text) = prompt {
            let input_msg = protocol::input("agent-ctl", prompt_text, TabTarget::None);
            let _ = pipe::send_pipe_message(&s.pipe_path, &input_msg);
            let enter_msg = protocol::raw_input("agent-ctl", "\r", TabTarget::None);
            let _ = pipe::send_pipe_message(&s.pipe_path, &enter_msg);
        }
        Ok(())
    }

    #[cfg(not(feature = "librarian"))]
    fn launch(&self, _session_hint: &str, _agent_type: &str, _prompt: Option<&str>) -> Result<()> {
        Err(AgentCtlError::Other("librarian feature not enabled".into()))
    }

    fn ping(&self, session_hint: &str) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let resp = pipe::send_pipe_message(&s.pipe_path, &protocol::ping())?;
        Ok(resp)
    }

    fn raw_send(&self, session_hint: &str, text: &str) -> Result<()> {
        let s = session::find_session(session_hint)?;
        let msg = protocol::raw_input("agent-ctl", text, TabTarget::None);
        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(())
    }

    fn state(&self, session_hint: &str) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let resp = pipe::send_pipe_message(&s.pipe_path, &protocol::state(TabTarget::None))?;
        Ok(resp)
    }

    fn tabs(&self, session_hint: &str) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let resp = pipe::send_pipe_message(&s.pipe_path, &protocol::list_tabs())?;
        Ok(resp)
    }

    fn paste(&self, session_hint: &str, text: &str, tab: Option<&str>) -> Result<()> {
        let s = session::find_session(session_hint)?;
        let target = parse_tab_str(tab);
        let msg = protocol::paste("agent-ctl", text, target);
        let response = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&response) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(())
    }

    fn wait_for(&self, session_hint: &str, pattern: &str, timeout_ms: u32, tab: Option<&str>) -> Result<String> {
        let s = session::find_session(session_hint)?;
        let target = parse_tab_str(tab);
        let msg = protocol::wait_for(timeout_ms, pattern, target);
        let resp = pipe::send_pipe_message(&s.pipe_path, &msg)?;
        if let Some(err) = protocol::is_error(&resp) {
            return Err(AgentCtlError::ServerError(err));
        }
        Ok(resp)
    }

    fn stop(&self, session_hint: &str, agent_type: &str) -> Result<()> {
        let s = session::find_session(session_hint)?;

        match agent_type {
            "claude" => {
                let ctrl_c = protocol::raw_input("agent-ctl", "\x03", TabTarget::None);
                let _ = pipe::send_pipe_message(&s.pipe_path, &ctrl_c);
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = pipe::send_pipe_message(&s.pipe_path, &ctrl_c);
            }
            "gemini" => {
                let ctrl_c = protocol::raw_input("agent-ctl", "\x03", TabTarget::None);
                let _ = pipe::send_pipe_message(&s.pipe_path, &ctrl_c);
            }
            "codex" => {
                let exit_msg = protocol::raw_input("agent-ctl", "/exit\r", TabTarget::None);
                let _ = pipe::send_pipe_message(&s.pipe_path, &exit_msg);
            }
            _ => {
                let ctrl_c = protocol::raw_input("agent-ctl", "\x03", TabTarget::None);
                let _ = pipe::send_pipe_message(&s.pipe_path, &ctrl_c);
            }
        }
        Ok(())
    }
}

fn parse_tab_str(tab: Option<&str>) -> TabTarget {
    match tab {
        None => TabTarget::None,
        Some(t) if t.is_empty() => TabTarget::None,
        Some(t) if t.starts_with("id=") => TabTarget::Id(t[3..].to_string()),
        Some(t) => {
            if let Ok(idx) = t.parse::<usize>() {
                TabTarget::Index(idx)
            } else {
                TabTarget::Id(t.to_string())
            }
        }
    }
}
