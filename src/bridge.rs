use crate::error::{AgentCtlError, Result};
use crate::pipe;
use crate::protocol::{self, TabTarget};
use crate::session;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEFAULT_BRIDGE_PIPE: &str = r"\\.\pipe\WT_CP_bridge";

#[derive(Debug, Deserialize)]
struct JsonRequest {
    action: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    lines: Option<usize>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    tab: Option<String>, // Changed to String to support id=...
    #[serde(default)]
    timeout_ms: Option<u32>,
    #[serde(default)]
    pattern: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonResponse {
    status: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    data: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
}

impl JsonResponse {
    fn ok(data: String) -> Self {
        Self {
            status: "ok".into(),
            data,
            error: String::new(),
        }
    }

    fn err(error: String) -> Self {
        Self {
            status: "error".into(),
            data: String::new(),
            error,
        }
    }
}

/// Resolve a session hint to a pipe path. If session is empty, finds any alive session.
fn resolve_pipe_path(session_hint: &str) -> Result<String> {
    let info = session::find_session(session_hint)?;
    Ok(info.pipe_path)
}

fn parse_tab_target(tab: Option<&str>) -> TabTarget {
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

/// Handle a JSON request string and return a JSON response string.
pub fn handle_json_request(json_str: &str) -> Result<String> {
    let req: JsonRequest = serde_json::from_str(json_str)
        .map_err(|e| AgentCtlError::Protocol(format!("invalid JSON: {}", e)))?;

    let pipe_path = resolve_pipe_path(&req.session)?;
    let target = parse_tab_target(req.tab.as_deref());

    let response = match req.action.to_uppercase().as_str() {
        "TAIL" => {
            let lines = req.lines.unwrap_or(30);
            let msg = protocol::tail(lines, target);
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "INPUT" => {
            let text = req.text.as_deref().unwrap_or("");
            // Step 1: Send text via INPUT (bracketed paste)
            let msg = protocol::input("agent-deck", text, target.clone());
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            // Step 2: Brief pause for TUI to process
            std::thread::sleep(Duration::from_millis(100));
            // Step 3: Send Enter via RAW_INPUT
            let enter_msg = protocol::raw_input("agent-deck", "\r", target);
            let resp2 = pipe::send_pipe_message(&pipe_path, &enter_msg)?;
            if let Some(err) = protocol::is_error(&resp2) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp2)
        }
        "RAW_INPUT" => {
            let text = req.text.as_deref().unwrap_or("");
            let msg = protocol::raw_input("agent-deck", text, target);
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "PASTE" => {
            let text = req.text.as_deref().unwrap_or("");
            let msg = protocol::paste("agent-deck", text, target);
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "WAIT_FOR" => {
            let timeout = req.timeout_ms.unwrap_or(5000);
            let pattern = req.pattern.as_deref().unwrap_or("");
            let msg = protocol::wait_for(timeout, pattern, target);
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "PING" => {
            let msg = protocol::ping();
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "STATE" => {
            let msg = protocol::state(target);
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "LIST_TABS" => {
            let msg = protocol::list_tabs();
            let resp = pipe::send_pipe_message(&pipe_path, &msg)?;
            if let Some(err) = protocol::is_error(&resp) {
                return Ok(serde_json::to_string(&JsonResponse::err(err)).unwrap());
            }
            JsonResponse::ok(resp)
        }
        "LIST_SESSIONS" => {
            let sessions = session::discover_sessions();
            let data: Vec<serde_json::Value> = sessions
                .iter()
                .filter(|s| session::is_process_alive(s.pid))
                .map(|s| {
                    serde_json::json!({
                        "session_name": s.session_name,
                        "pid": s.pid,
                        "pipe_path": s.pipe_path,
                    })
                })
                .collect();
            let json_data = serde_json::to_string(&data).unwrap_or_default();
            JsonResponse::ok(json_data)
        }
        other => {
            return Ok(
                serde_json::to_string(&JsonResponse::err(format!("unknown action: {}", other)))
                    .unwrap(),
            );
        }
    };

    Ok(serde_json::to_string(&response).unwrap())
}

/// Run the bridge named pipe server. Loops forever accepting connections.
pub fn run_server(pipe_name: &str) -> Result<()> {
    use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{
        FlushFileBuffers, ReadFile, WriteFile, PIPE_ACCESS_DUPLEX,
    };
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeA, DisconnectNamedPipe,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::core::PCSTR;

    let pipe_path = if pipe_name.starts_with(r"\\") {
        pipe_name.to_string()
    } else {
        format!(r"\\.\pipe\{}", pipe_name)
    };
    let pipe_path_cstr = format!("{}\0", pipe_path);

    eprintln!("[bridge] listening on {}", pipe_path);

    loop {
        // Create the named pipe instance
        let handle = unsafe {
            CreateNamedPipeA(
                PCSTR(pipe_path_cstr.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                65536, // out buffer
                65536, // in buffer
                0,     // default timeout
                None,  // default security
            )
        }
        .map_err(|e| AgentCtlError::PipeConnect(format!("CreateNamedPipe failed: {}", e)))?;

        if handle == INVALID_HANDLE_VALUE {
            return Err(AgentCtlError::PipeConnect(
                "CreateNamedPipe returned INVALID_HANDLE_VALUE".into(),
            ));
        }

        // Wait for a client to connect
        let connect_result = unsafe { ConnectNamedPipe(handle, None) };
        if let Err(e) = connect_result {
            // ERROR_PIPE_CONNECTED (535) means client connected before we called ConnectNamedPipe — that's OK
            let os_err = std::io::Error::last_os_error();
            if os_err.raw_os_error() != Some(535) {
                eprintln!("[bridge] ConnectNamedPipe failed: {} (os={:?})", e, os_err.raw_os_error());
                unsafe { let _ = CloseHandle(handle); }
                continue;
            }
        }

        // Read request
        let mut buffer = vec![0u8; 65536];
        let mut bytes_read = 0u32;
        let read_ok = unsafe { ReadFile(handle, Some(&mut buffer), Some(&mut bytes_read), None) };

        let response_json = if read_ok.is_err() || bytes_read == 0 {
            serde_json::to_string(&JsonResponse::err("failed to read request".into()))
                .unwrap_or_default()
        } else {
            let request_str =
                String::from_utf8_lossy(&buffer[..bytes_read as usize]).to_string();
            eprintln!("[bridge] << {}", request_str.trim());

            match handle_json_request(&request_str) {
                Ok(resp) => resp,
                Err(e) => {
                    serde_json::to_string(&JsonResponse::err(format!("{}", e)))
                        .unwrap_or_default()
                }
            }
        };

        eprintln!("[bridge] >> {}", response_json.trim());

        // Write response
        let resp_bytes = response_json.as_bytes();
        let mut written = 0u32;
        unsafe {
            let _ = WriteFile(handle, Some(resp_bytes), Some(&mut written), None);
            let _ = FlushFileBuffers(handle);
        }

        // Disconnect and close
        unsafe {
            let _ = DisconnectNamedPipe(handle);
            let _ = CloseHandle(handle);
        }
    }
}

/// Entry point for the `bridge` subcommand.
pub fn run(pipe_name: Option<&str>) -> Result<()> {
    let name = pipe_name.unwrap_or(DEFAULT_BRIDGE_PIPE);
    run_server(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tab_target() {
        assert_eq!(parse_tab_target(None), TabTarget::None);
        assert_eq!(parse_tab_target(Some("")), TabTarget::None);
        assert_eq!(parse_tab_target(Some("2")), TabTarget::Index(2));
        assert_eq!(parse_tab_target(Some("id=t1")), TabTarget::Id("t1".into()));
        assert_eq!(parse_tab_target(Some("t1")), TabTarget::Id("t1".into()));
    }
}
