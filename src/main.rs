mod backend;
#[cfg(feature = "bridge")]
mod bridge;
mod commands;
mod error;
#[cfg(feature = "librarian")]
mod librarian;
mod pipe;
mod protocol;
mod session;

use backend::{AgentBackend, control_plane::ControlPlaneBackend};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent-ctl", about = "Agent Control Plane CLI")]
struct Cli {
    /// Backend to use: wt, ghostty
    #[arg(long, default_value = "wt")]
    backend: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all control plane sessions
    List {
        /// Only show alive sessions
        #[arg(long)]
        alive_only: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Get agent status of a session (via Librarian)
    #[cfg(feature = "librarian")]
    Status {
        /// Session name or hint (substring match)
        session: String,
    },
    /// Send text to a session (INPUT + Enter)
    Send {
        /// Session name or hint
        session: String,
        /// Text to send
        text: String,
        /// Also send raw CR after the text
        #[arg(long)]
        enter: bool,
    },
    /// Read terminal output (TAIL)
    Read {
        /// Session name or hint
        session: String,
        /// Number of lines to read
        #[arg(long, default_value = "30")]
        lines: usize,
        /// Tab index to read from (default: active tab)
        #[arg(long)]
        tab: Option<usize>,
    },
    /// Wait for session to become idle
    Wait {
        /// Session name or hint
        session: String,
        /// Timeout in seconds
        #[arg(long, default_value = "120")]
        timeout: u64,
        /// Auto-approve when approval is detected
        #[arg(long)]
        auto_approve: bool,
    },
    /// Launch terminal (if needed), start agent, send task, wait for completion
    Run {
        /// Agent type: claude, gemini, codex
        #[arg(long)]
        agent: String,
        /// Task to send to the agent
        #[arg(long)]
        task: String,
        /// Path to terminal exe (launches if no alive session found)
        #[arg(long)]
        exe: Option<String>,
        /// Session name or hint (empty = any alive session)
        #[arg(long, default_value = "")]
        session: String,
        /// Stop at stage: launch, ready, sent (default), done
        #[arg(long, default_value = "sent")]
        stop_at: String,
    },
    /// Send approval (y + Enter) to a session
    Approve {
        /// Session name or hint
        session: String,
    },
    /// Tab management (new, switch, close, list)
    Tab {
        /// Session name or hint
        session: String,
        /// Action: new, switch, close, list
        action: String,
        /// Tab index (required for switch, optional for close)
        index: Option<usize>,
    },
    /// Launch an agent in a session
    Launch {
        /// Session name or hint
        session: String,
        /// Agent type: claude, gemini, codex
        agent_type: String,
        /// Optional initial prompt
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Stop an agent in a session
    Stop {
        /// Session name or hint
        session: String,
        /// Agent type: claude, gemini, codex
        agent_type: String,
    },
    /// Send PING, expect OK response
    Ping {
        /// Session name or hint
        session: String,
    },
    /// Send RAW_INPUT (no Enter appended)
    RawSend {
        /// Session name or hint
        session: String,
        /// Text to send raw (ignored if --ctrl-c/--ctrl-d/--ctrl-z is set)
        #[clap(default_value = "")]
        text: String,
        /// Send Ctrl+C (0x03 ETX) to interrupt the running process
        #[clap(long)]
        ctrl_c: bool,
        /// Send Ctrl+D (0x04 EOT) to signal end-of-input
        #[clap(long)]
        ctrl_d: bool,
        /// Send Ctrl+Z (0x1a SUB) to suspend the running process
        #[clap(long)]
        ctrl_z: bool,
    },
    /// Send STATE request, print response
    State {
        /// Session name or hint
        session: String,
    },
    /// Send PASTE request (bracketed paste)
    Paste {
        /// Session name or hint
        session: String,
        /// Text to paste
        text: String,
        /// Tab index or id
        #[arg(long)]
        tab: Option<String>,
    },
    /// Send WAIT_FOR request
    WaitFor {
        /// Session name or hint
        session: String,
        /// Pattern to wait for
        pattern: String,
        /// Timeout in milliseconds
        #[arg(long, default_value = "5000")]
        timeout: u32,
        /// Tab index or id
        #[arg(long)]
        tab: Option<String>,
    },
    /// Send LIST_TABS request, print response
    Tabs {
        /// Session name or hint
        session: String,
    },
    /// JSON bridge server for agent-deck WtcpDriver
    #[cfg(feature = "bridge")]
    Bridge {
        /// Named pipe path (default: \\.\pipe\WT_CP_bridge)
        #[arg(long)]
        pipe_name: Option<String>,
    },
    /// Remove dead session files from LocalAppData
    Clean,
    /// Automated smoke test (PING + LIST_TABS + STATE)
    Smoke {
        /// Session name or hint
        session: String,
    },
    /// Auto-cycle all agents and capture real buffers for test fixtures
    #[cfg(feature = "librarian")]
    SampleAll {
        /// Session name or hint
        session: String,
        /// Agents to test (default: claude, gemini, codex)
        #[arg(long, value_delimiter = ',')]
        agents: Vec<String>,
        /// Output file (default: tests/fixtures/buffers_real.toml)
        #[arg(long)]
        output: Option<String>,
    },
    /// Capture real terminal buffer as Librarian test fixture
    #[cfg(feature = "librarian")]
    Sample {
        /// Session name or hint
        session: String,
        /// Test case name (e.g. codex_ready). If omitted, just display + judge.
        #[arg(long)]
        name: Option<String>,
        /// Expected state (e.g. AGENT_READY). If omitted, uses Librarian judgment.
        #[arg(long)]
        state: Option<String>,
        /// Number of lines to TAIL
        #[arg(long, default_value = "20")]
        lines: usize,
    },
}

fn main() {
    let cli = Cli::parse();

    let backend: Box<dyn AgentBackend> = match cli.backend.as_str() {
        "wt" | "ghostty" => Box::new(ControlPlaneBackend),
        other => {
            eprintln!("Error: Unknown backend '{}'", other);
            std::process::exit(1);
        }
    };

    let result = match cli.command {
        Commands::List { alive_only, json } => commands::list::run(backend.as_ref(), alive_only, json),
        #[cfg(feature = "librarian")]
        Commands::Status { session } => commands::status::run(backend.as_ref(), &session),
        Commands::Send { session, text, enter } => commands::send::run(backend.as_ref(), &session, &text, enter),
        Commands::Read { session, lines, tab } => commands::read::run(backend.as_ref(), &session, lines, tab),
        Commands::Wait {
            session,
            timeout,
            auto_approve,
        } => commands::wait::run(backend.as_ref(), &session, timeout, auto_approve),
        Commands::Run {
            session,
            agent,
            task,
            exe,
            stop_at,
        } => commands::run::run(backend.as_ref(), &session, &agent, &task, exe.as_deref(), &stop_at),
        Commands::Approve { session } => commands::approve::run(backend.as_ref(), &session),
        Commands::Tab {
            session,
            action,
            index,
        } => commands::tab::run(backend.as_ref(), &session, &action, index),
        Commands::Launch {
            session,
            agent_type,
            prompt,
        } => commands::launch::run(backend.as_ref(), &session, &agent_type, prompt.as_deref()),
        Commands::Stop {
            session,
            agent_type,
        } => commands::stop::run(backend.as_ref(), &session, &agent_type),
        Commands::Ping { session } => commands::ping::run(backend.as_ref(), &session),
        Commands::RawSend { session, text, ctrl_c, ctrl_d, ctrl_z } => {
            let payload = if ctrl_c {
                "\x03".to_string()
            } else if ctrl_d {
                "\x04".to_string()
            } else if ctrl_z {
                "\x1a".to_string()
            } else {
                text
            };
            commands::raw_send::run(backend.as_ref(), &session, &payload)
        },
        Commands::State { session } => commands::state::run(backend.as_ref(), &session),
        Commands::Paste { session, text, tab } => commands::paste::run(backend.as_ref(), &session, &text, tab.as_deref()),
        Commands::WaitFor { session, pattern, timeout, tab } => {
            commands::wait_for::run(backend.as_ref(), &session, &pattern, timeout, tab.as_deref())
        },
        Commands::Tabs { session } => commands::tabs::run(backend.as_ref(), &session),
        #[cfg(feature = "bridge")]
        Commands::Bridge { pipe_name } => bridge::run(pipe_name.as_deref()),
        Commands::Clean => commands::clean::run(),
        Commands::Smoke { session } => commands::smoke::run(backend.as_ref(), &session),
        #[cfg(feature = "librarian")]
        Commands::SampleAll { session, agents, output } => {
            commands::sample_all::run(backend.as_ref(), &session, &agents, output.as_deref())
        }
        #[cfg(feature = "librarian")]
        Commands::Sample { session, name, state, lines } => {
            commands::sample::run(backend.as_ref(), &session, name.as_deref(), state.as_deref(), lines)
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
