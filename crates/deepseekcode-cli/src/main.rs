use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use deepseekcode_config::LoggingConfig;
use deepseekcode_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest};
use deepseekcode_sdk::{client::DeepSeekCodeClient, EmbeddedEngine};
use deepseekcode_telemetry::init;
use deepseekcode_types::{MessageId, SessionId};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

mod cli_locale;

/// Per-turn receipt with cost: `in … (H cached / M new) · out … · ¥0.0123`.
/// Cost comes from the server-stamped `estimated_cost`/`currency` when present
/// (per-model official row at occurrence time); otherwise it is priced locally
/// via the official catalog so old events still show money. Display-only:
/// `show=false` stays silent, accounting always runs server-side.
fn turn_receipt_line(payload: &serde_json::Value, show: bool) -> Option<String> {
    // Transcript-only events carry no usage: stay silent instead of printing
    // a zero line (the shared helper rebuilds from zeros when asked).
    let has_usage = payload
        .get("input_tokens")
        .and_then(|v| v.as_u64())
        .is_some()
        || payload
            .get("cache_receipt")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
    if !has_usage {
        return None;
    }
    // Prefer server-stamped cost (exact per-model band); fall back to local
    // catalog pricing so pre-upgrade events still show a cost line.
    let cost_display: Option<String> = payload
        .get("estimated_cost")
        .and_then(|v| v.as_f64())
        .map(|cost| {
            let cur = payload
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CNY");
            deepseekcode_core::pricing::format_cost(cur, cost)
        })
        .or_else(|| {
            let model = payload.get("model").and_then(|v| v.as_str()).unwrap_or("");
            let currency = payload
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CNY");
            let (card, _) = deepseekcode_core::pricing::rate_for(model, currency)
                .or_else(|| {
                    // Unknown model or missing model field: price with the
                    // configured global card rather than going blind.
                    None
                })?;
            let usage = deepseekcode_llm::TokenUsage {
                input_tokens: payload.get("input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
                output_tokens: payload.get("output_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
                cache_hit: payload.get("cache_hit").and_then(|v| v.as_u64()).map(|v| v as u32),
                cache_miss: payload.get("cache_miss").and_then(|v| v.as_u64()).map(|v| v as u32),
                ..Default::default()
            };
            let hit = usage.cache_hit.unwrap_or(0) as u64;
            let miss = usage.cache_miss.map(|v| v as u64).unwrap_or_else(|| {
                (usage.input_tokens.unwrap_or(0) as u64).saturating_sub(hit)
            });
            let cost = deepseekcode_core::pricing::cost_for_tokens(
                hit,
                miss,
                usage.output_tokens.unwrap_or(0) as u64,
                &card,
            );
            Some(deepseekcode_core::pricing::format_cost(&card.currency, cost))
        });
    let usage = deepseekcode_llm::TokenUsage {
        input_tokens: payload.get("input_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
        output_tokens: payload.get("output_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
        reasoning_tokens: payload.get("reasoning_tokens").and_then(|v| v.as_u64()).map(|v| v as u32),
        cache_tokens: payload.get("cache_hit").and_then(|v| v.as_u64()).map(|v| v as u32),
        cache_hit: payload.get("cache_hit").and_then(|v| v.as_u64()).map(|v| v as u32),
        cache_miss: payload.get("cache_miss").and_then(|v| v.as_u64()).map(|v| v as u32),
        cache_write: payload.get("cache_write").and_then(|v| v.as_u64()).map(|v| v as u32),
    };
    let line = deepseekcode_core::cache_status::format_turn_receipt_with_cost(
        &usage,
        show,
        cost_display.as_deref(),
    );
    if line.is_empty() {
        return None;
    }
    // Model tag keeps mixed-model sessions auditable: `[deepseek-flash] …`.
    let model_tag = payload
        .get("model")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|m| format!("[{m}] "))
        .unwrap_or_default();
    Some(format!("\n{model_tag}{line}"))
}

#[derive(Parser)]
#[command(name = "deepseekcode", about = "DeepSeekCode command-line interface")]
struct Cli {
    /// Remote server URL (e.g. http://127.0.0.1:4096). When set, commands use
    /// the remote server instead of starting an embedded engine (P1-11).
    #[arg(long, global = true, env = "DEEPSEEKCODE_SERVER_URL")]
    url: Option<String>,
    /// Server password for remote `--url` (or DEEPSEEKCODE_SERVER_PASSWORD).
    #[arg(long, global = true, env = "DEEPSEEKCODE_SERVER_PASSWORD")]
    password: Option<String>,
    /// Interface language tag (`auto`, `en`, `zh-Hans`, …). The catalog
    /// rewrites clap help after this is read, so help itself is translated.
    #[arg(long, global = true, value_name = "LOCALE")]
    lang: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single prompt non-interactively
    Run {
        #[arg(short, long, default_value = ".")]
        directory: String,
        /// Auto-permit non-block policy rules (YOLO mode)
        #[arg(long)]
        auto: bool,
        prompt: String,
    },
    /// Start the HTTP server
    Serve {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long, default_value = "4096")]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long)]
        password: Option<String>,
        /// Auto-permit non-block policy rules (YOLO mode)
        #[arg(long)]
        auto: bool,
    },
    /// Check for application updates (stub for Phase 6 packaging).
    Upgrade {
        #[arg(long)]
        check_only: bool,
    },
    /// List sessions (legacy alias)
    SessionList {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// Export diagnostic logs
    Diagnostics {
        #[command(subcommand)]
        command: DiagnosticsCommands,
    },
    /// Agent commands
    Agent {
        #[command(subcommand)]
        command: AgentCommands,
    },
    /// Model commands
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    /// Provider commands
    Provider {
        #[command(subcommand)]
        command: ProviderCommands,
    },
    /// Send a prompt to a session
    Prompt {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: Option<String>,
        // Default steer to match Desktop Settings "Steer (default)".
        #[arg(long, default_value = "steer")]
        delivery: String,
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        resume: bool,
        text: String,
    },
    /// Session lifecycle commands
    Session {
        #[command(subcommand)]
        command: SessionCommands,
    },
    /// MCP server commands
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },
    /// Project filesystem commands
    Fs {
        #[command(subcommand)]
        command: FsCommands,
    },
    /// List discovered skills
    Skills {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// List loaded plugins
    Plugins {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// Git worktree commands
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommands,
    },
    /// Project commands
    Project {
        #[command(subcommand)]
        command: ProjectCommands,
    },
    /// Show effective configuration
    Config {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// Check server health
    Health {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(long)]
        url: Option<String>,
    },
    /// Policy prompt management
    Policy {
        #[command(subcommand)]
        command: PolicyCommands,
    },
}

#[derive(Subcommand)]
enum PolicyCommands {
    /// List pending policy prompts
    Pending {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// Reply to a pending policy prompt
    Reply {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        id: String,
        #[arg(short, long)]
        reply: String,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// List remembered project grants (previously missing).
    Grants {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    /// Delete a remembered grant by id.
    GrantDelete {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand)]
enum DiagnosticsCommands {
    Export {
        #[arg(short, long)]
        output: PathBuf,
        #[arg(long)]
        run_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum AgentCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Set {
        #[arg(short, long, default_value = ".")]
        directory: String,
        agent: String,
        #[arg(short, long)]
        session: Option<String>,
    },
}

#[derive(Subcommand)]
enum ModelCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Set {
        #[arg(short, long, default_value = ".")]
        directory: String,
        model: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(short, long)]
        session: Option<String>,
    },
}

#[derive(Subcommand)]
enum ProviderCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Connect {
        #[arg(short, long, default_value = ".")]
        directory: String,
        provider: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
    },
    Disconnect {
        #[arg(short, long, default_value = ".")]
        directory: String,
        provider: String,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Fork {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
        #[arg(long)]
        until: Option<String>,
    },
    Export {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
        #[arg(
            long,
            help = "Disable redaction of sensitive fields (redaction is on by default)"
        )]
        no_sanitize: bool,
        #[arg(short, long)]
        output: PathBuf,
    },
    Revert {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
        #[arg(long)]
        message: String,
    },
    Compact {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    Move {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
        target: String,
    },
    /// Interrupt a running session (previously missing; desktop-only).
    Interrupt {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    /// Delete a session.
    Delete {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    /// Show session todos.
    Todo {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    /// List completed plans for a session.
    Plans {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    /// Answer a pending session question.
    Answer {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
        text: String,
    },
    /// Commit a staged revert.
    RevertCommit {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
    /// Clear a staged revert without applying.
    RevertClear {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        session: String,
    },
}

#[derive(Subcommand)]
enum FsCommands {
    /// List a directory within the project.
    Tree {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long, default_value = "")]
        path: String,
        #[arg(long, default_value_t = 0)]
        offset: usize,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Print a file within the project.
    File {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(short, long)]
        path: String,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Connect {
        #[arg(short, long, default_value = ".")]
        directory: String,
        name: String,
    },
    Disconnect {
        #[arg(short, long, default_value = ".")]
        directory: String,
        name: String,
    },
}

#[derive(Subcommand)]
enum WorktreeCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Create {
        #[arg(short, long, default_value = ".")]
        directory: String,
        #[arg(long)]
        branch: Option<String>,
    },
    Remove {
        #[arg(short, long, default_value = ".")]
        directory: String,
        path: String,
    },
    Prune {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
}

#[derive(Subcommand)]
enum ProjectCommands {
    Directories {
        #[command(subcommand)]
        command: ProjectDirectoryCommands,
    },
}

#[derive(Subcommand)]
enum ProjectDirectoryCommands {
    List {
        #[arg(short, long, default_value = ".")]
        directory: String,
    },
    Add {
        #[arg(short, long, default_value = ".")]
        directory: String,
        path: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Language first: translated `--help` and translated command output must
    // observe the same locale. The directory hint is recovered from the raw
    // arguments because `clap` has not parsed yet.
    let raw_args = cli_locale::raw_args_after_program();
    cli_locale::init_cli_locale(&raw_args);

    // P1-11: honor `--directory` for logging config (previously hardcoded `.`).
    // Parse args first for the directory hint, then init logging.
    let mut command = Cli::command();
    cli_locale::localize_command(&mut command);
    let matches = command
        .try_get_matches_from(
            std::iter::once(
                std::env::args()
                    .next()
                    .unwrap_or_else(|| "deepseekcode".into()),
            )
            .chain(raw_args.into_iter()),
        )
        .unwrap_or_else(|e| e.exit());
    let pre = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    let log_dir = directory_hint(&pre);
    let (logging, config_error) = load_logging_config_for(&log_dir);
    // Top-level boundary: every command failure is logged with run_id +
    // command + error so CLI invocations are never silent in app.log.
    let command_label = cli_command_label(&pre.command);
    let cli = pre;
    match run(cli, logging, config_error).await {
        Ok(()) => Ok(()),
        Err(e) => {
            tracing::error!(
                target: deepseekcode_telemetry::CLI,
                run_id = %deepseekcode_telemetry::current_run_id(),
                command = %command_label,
                error = %e,
                "cli command failed"
            );
            Err(e)
        }
    }
}

fn cli_command_label(command: &Commands) -> &'static str {
    match command {
        Commands::Run { .. } => "run",
        Commands::Serve { .. } => "serve",
        Commands::Upgrade { .. } => "upgrade",
        Commands::SessionList { .. } => "session-list",
        Commands::Diagnostics { .. } => "diagnostics",
        Commands::Agent { .. } => "agent",
        Commands::Model { .. } => "model",
        Commands::Provider { .. } => "provider",
        Commands::Prompt { .. } => "prompt",
        Commands::Session { .. } => "session",
        Commands::Mcp { .. } => "mcp",
        Commands::Fs { .. } => "fs",
        Commands::Skills { .. } => "skills",
        Commands::Plugins { .. } => "plugins",
        Commands::Worktree { .. } => "worktree",
        Commands::Project { .. } => "project",
        Commands::Config { .. } => "config",
        Commands::Health { .. } => "health",
        Commands::Policy { .. } => "policy",
    }
}

fn directory_hint(cli: &Cli) -> String {
    match &cli.command {
        Commands::Run { directory, .. }
        | Commands::Prompt { directory, .. }
        | Commands::SessionList { directory }
        | Commands::Config { directory }
        | Commands::Health { directory, .. } => directory.clone(),
        Commands::Serve { directory, .. } => directory.clone(),
        Commands::Session { command } => match command {
            SessionCommands::List { directory }
            | SessionCommands::Fork { directory, .. }
            | SessionCommands::Export { directory, .. }
            | SessionCommands::Revert { directory, .. }
            | SessionCommands::Compact { directory, .. }
            | SessionCommands::Move { directory, .. }
            | SessionCommands::Interrupt { directory, .. }
            | SessionCommands::Delete { directory, .. }
            | SessionCommands::Todo { directory, .. }
            | SessionCommands::Plans { directory, .. }
            | SessionCommands::Answer { directory, .. }
            | SessionCommands::RevertCommit { directory, .. }
            | SessionCommands::RevertClear { directory, .. } => directory.clone(),
        },
        Commands::Fs { command } => match command {
            FsCommands::Tree { directory, .. } | FsCommands::File { directory, .. } => {
                directory.clone()
            }
        },
        Commands::Skills { directory } | Commands::Plugins { directory } => directory.clone(),
        _ => ".".to_string(),
    }
}

async fn run(cli: Cli, logging: LoggingConfig, config_error: Option<String>) -> anyhow::Result<()> {
    let run_ctx = init("cli", &logging);
    tracing::info!(
        target: deepseekcode_telemetry::CLI,
        run_id = %run_ctx.run_id,
        "deepseekcode-cli starting"
    );
    // Stage two of config-error visibility: stage one eprintln'd before the
    // subscriber existed; now also persist to app.log for bundle debugging.
    if let Some(err) = config_error.as_deref() {
        tracing::warn!(
            target: deepseekcode_telemetry::CLI,
            run_id = %run_ctx.run_id,
            error = %err,
            "config load failed; using default logging"
        );
    }

    // P1-11: global `--url`/`--password` (or env) route every command to a
    // remote server instead of starting an embedded engine.
    let Cli {
        url: global_url,
        password: global_password,
        lang: _,
        command,
    } = cli;
    let (url, password) = (global_url, global_password);

    match command {
        Commands::Run {
            directory,
            auto,
            prompt,
        } => {
            if auto {
                std::env::set_var("DEEPSEEKCODE_POLICY_MODE", "auto");
            }
            let (engine, client) =
                scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
            let session = client
                .create_session(CreateSessionRequest {
                    id: None,
                    directory: directory.clone(),
                    agent: None,
                    model_id: None,
                    provider_id: None,
                })
                .await?;
            // P1-11: `run` is a fresh single-shot prompt — `steer`
            // matches `prompt` default and Desktop behavior (previously
            // `Queue`, splitting the two paths).
            let _message_id = client
                .prompt(PromptRequest {
                    id: None,
                    session_id: session.id,
                    prompt,
                    delivery: PromptDelivery::Steer,
                    resume: true,
                })
                .await?;
            use deepseekcode_schema::event::EventKind;
            use deepseekcode_sdk::EventStreamItem;
            use futures::StreamExt;
            // P1-4: receipt display gate. Accounting always runs server-side;
            // this only controls the per-turn line. Default true on error.
            let show_turn_usage =
                deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory))
                    .map(|cfg| cfg.ui.show_turn_usage)
                    .unwrap_or(true);
            let result = tokio::time::timeout(std::time::Duration::from_secs(600), async {
                let mut since_seq = 0i64;
                // Exponential backoff for reconnects after a failed gap
                // backfill; otherwise the server keeps reporting the same gap
                // and we would hammer it in a tight reconnect loop.
                let mut reconnect_backoff = std::time::Duration::from_millis(500);
                loop {
                    let mut stream = client.event_stream(since_seq, Some(session.id)).await?;
                    let mut stream_ended = true;
                    while let Some(item) = stream.next().await {
                        stream_ended = false;
                        match item {
                            EventStreamItem::Gap { head, .. } => {
                                match client.list_session_events(session.id, since_seq).await {
                                    Ok(events) => {
                                        reconnect_backoff = std::time::Duration::from_millis(500);
                                        let mut max_seq = since_seq;
                                        for event in events {
                                            if event.session_id != Some(session.id) {
                                                max_seq = max_seq.max(event.seq);
                                                continue;
                                            }
                                            // Advance per-event so reconnects
                                            // never reprint deltas (previously
                                            // only `head` at end, causing dupes
                                            // when head lagged).
                                            max_seq = max_seq.max(event.seq);
                                            if event.kind == EventKind::SessionIdle {
                                                return Ok(());
                                            }
                                            if event.kind == EventKind::MessageDelta {
                                                if let Some(delta) = event
                                                    .payload
                                                    .get("delta")
                                                    .and_then(|v| v.as_str())
                                                {
                                                    print!("{delta}");
                                                    let _ = std::io::Write::flush(
                                                        &mut std::io::stdout(),
                                                    );
                                                }
                                            }
                                            // P1-4: per-turn cache receipt on
                                            // completion (gap-backfill path).
                                            if event.kind == EventKind::MessageCompleted {
                                                if let Some(line) = turn_receipt_line(
                                                    &event.payload,
                                                    show_turn_usage,
                                                ) {
                                                    print!("{line}");
                                                    let _ = std::io::Write::flush(
                                                        &mut std::io::stdout(),
                                                    );
                                                }
                                            }
                                        }
                                        // Advance only: never move backwards.
                                        // The server's `head` can lag behind what
                                        // this client already applied (e.g. after
                                        // a broadcast-lag gap).
                                        since_seq = since_seq.max(max_seq).max(head);
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            target: deepseekcode_telemetry::CLI,
                                            error = %e,
                                            since_seq,
                                            head,
                                            "sse gap backfill failed; keeping since_seq"
                                        );
                                        tokio::time::sleep(reconnect_backoff).await;
                                        reconnect_backoff = (reconnect_backoff * 2)
                                            .min(std::time::Duration::from_secs(30));
                                    }
                                }
                                break;
                            }
                            EventStreamItem::Event(event) => {
                                since_seq = event.seq;
                                if event.session_id != Some(session.id) {
                                    continue;
                                }
                                match event.kind {
                                    EventKind::MessageDelta => {
                                        if let Some(delta) =
                                            event.payload.get("delta").and_then(|v| v.as_str())
                                        {
                                            print!("{delta}");
                                            let _ = std::io::Write::flush(&mut std::io::stdout());
                                        }
                                    }
                                    // P1-4: per-turn cache receipt on
                                    // completion (live-stream path).
                                    EventKind::MessageCompleted => {
                                        if let Some(line) =
                                            turn_receipt_line(&event.payload, show_turn_usage)
                                        {
                                            print!("{line}");
                                            let _ = std::io::Write::flush(&mut std::io::stdout());
                                        }
                                    }
                                    EventKind::SessionIdle => return Ok(()),
                                    EventKind::SessionInterrupted => {
                                        return Err(anyhow::anyhow!(deepseekcode_i18n::tr!(
                                            "cli.err.session_interrupted"
                                        )));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    if stream_ended {
                        break;
                    }
                }
                Err(anyhow::anyhow!(deepseekcode_i18n::tr!(
                    "cli.err.stream_ended"
                )))
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Session total: always fetch so the user sees money spent
                    // even when `show_turn_usage` hid the per-turn lines.
                    // Failure is non-fatal (transcript already printed).
                    if let Ok(status) = client.session_cache_status(session.id).await {
                        let turns = status.get("turns").and_then(|v| v.as_u64()).unwrap_or(0);
                        if turns > 0 {
                            let input = status.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                            let output = status.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
                            let cost = status.get("estimated_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
                            let currency = status.get("currency").and_then(|v| v.as_str()).unwrap_or("CNY");
                            let cost_str = deepseekcode_core::pricing::format_cost(currency, cost);
                            let summary = deepseekcode_i18n::tr!(
                                "cli.out.session_cost",
                                "input" => deepseekcode_i18n::count(input),
                                "output" => deepseekcode_i18n::count(output),
                                "turns" => turns,
                                "cost" => cost_str.as_str()
                            );
                            let models = status.get("models").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                            let mut per_model = String::new();
                            for m in models.iter().take(5) {
                                let name = m.get("model").and_then(|v| v.as_str()).unwrap_or("unknown");
                                let mc = m.get("estimated_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let mcur = m.get("currency").and_then(|v| v.as_str()).unwrap_or(currency);
                                per_model.push_str(&format!(
                                    "\n{}",
                                    deepseekcode_i18n::tr!(
                                        "cli.out.session_model_cost",
                                        "model" => name,
                                        "cost" => deepseekcode_core::pricing::format_cost(mcur, mc).as_str()
                                    )
                                ));
                            }
                            println!("\n[session] {summary}{per_model}");
                        }
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                Err(_) => {
                    eprintln!("{}", deepseekcode_i18n::tr!("cli.err.timeout"));
                    std::process::exit(1);
                }
            }
            shutdown_opt(engine).await;
        }
        Commands::Serve {
            directory,
            port,
            host,
            password,
            auto,
        } => {
            if auto {
                std::env::set_var("DEEPSEEKCODE_POLICY_MODE", "auto");
            }
            let addr: SocketAddr = format!("{host}:{port}").parse()?;
            let db_path = deepseekcode_types::data_dir()
                .map(|p| p.join("deepseekcode").join("deepseekcode.db"))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "deepseekcode.db".into());
            if let Some(parent) = std::path::Path::new(&db_path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let password = password
                .filter(|p| !p.trim().is_empty())
                .or_else(|| std::env::var("DEEPSEEKCODE_SERVER_PASSWORD").ok());
            let db_url = format!("sqlite://{db_path}");
            // Never log `password`/secret: log only addr + run_id.
            if let Err(e) = deepseekcode_server::serve(directory, db_url, addr, password).await {
                tracing::error!(
                    target: deepseekcode_telemetry::CLI,
                    run_id = %deepseekcode_telemetry::current_run_id(),
                    command = "serve",
                    addr = %addr,
                    error = %e,
                    "serve startup failed"
                );
                return Err(e.into());
            }
        }
        Commands::Upgrade { check_only } => {
            // P1-11: real check against the release channel manifest
            // (`packaging/desktop/windows/dist/latest.json` schema) instead of
            // a println stub. Falls back to a clear message when offline.
            let current = env!("CARGO_PKG_VERSION");
            let manifest_url = std::env::var("DEEPSEEKCODE_UPDATE_URL").ok();
            if let Some(url) = manifest_url {
                match reqwest::Client::builder()
                    .timeout(std::time::Duration::from_secs(10))
                    .build()
                {
                    Ok(http) => match http.get(&url).send().await {
                        Ok(resp) => match resp.json::<serde_json::Value>().await {
                            Ok(manifest) => {
                                let latest = manifest
                                    .get("version")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown");
                                let download =
                                    manifest.get("url").and_then(|v| v.as_str()).unwrap_or("");
                                if latest == current {
                                    println!(
                                        "{}",
                                        deepseekcode_i18n::tr!("cli.out.up_to_date", "current" => current)
                                    );
                                } else {
                                    println!(
                                        "{}",
                                        deepseekcode_i18n::tr!(
                                            "cli.out.update_available",
                                            "current" => current,
                                            "latest" => latest,
                                            "download" => download
                                        )
                                    );
                                    if !check_only {
                                        println!(
                                            "{}",
                                            deepseekcode_i18n::tr!("cli.out.download_hint")
                                        );
                                    }
                                }
                            }
                            Err(e) => println!(
                                "{}",
                                deepseekcode_i18n::tr!(
                                    "cli.out.manifest_parse_failed",
                                    "current" => current,
                                    "error" => e.to_string()
                                )
                            ),
                        },
                        Err(e) => println!(
                            "{}",
                            deepseekcode_i18n::tr!(
                                "cli.out.check_failed_offline",
                                "current" => current,
                                "error" => e.to_string()
                            )
                        ),
                    },
                    Err(e) => println!(
                        "{}",
                        deepseekcode_i18n::tr!(
                            "cli.out.check_failed",
                            "current" => current,
                            "error" => e.to_string()
                        )
                    ),
                }
            } else if check_only {
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.no_channel", "current" => current)
                );
            } else {
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.auto_update_unavailable")
                );
            }
        }
        Commands::SessionList { directory } => {
            eprintln!(
                "{}",
                deepseekcode_i18n::tr!("cli.out.deprecated_session_list")
            );
            let (engine, client) =
                scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
            for s in client.list_sessions().await? {
                println!("{} {:?}", s.id, s.title);
            }
            shutdown_opt(engine).await;
        }
        Commands::Diagnostics { command } => match command {
            DiagnosticsCommands::Export { output, run_id } => {
                match deepseekcode_telemetry::export_diagnostic_bundle(run_id.as_deref(), &output) {
                    Ok(path) => {
                        tracing::info!(
                            target: deepseekcode_telemetry::CLI,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            command = "diagnostics export",
                            path = %path.display(),
                            "diagnostics export succeeded"
                        );
                        println!(
                            "{}",
                            deepseekcode_i18n::tr!("cli.out.exported_diagnostics", "path" => path.display().to_string())
                        );
                        println!(
                            "{}",
                            deepseekcode_i18n::tr!("cli.out.run_id", "run_id" => run_ctx.run_id.as_str())
                        );
                    }
                    Err(e) => {
                        tracing::error!(
                            target: deepseekcode_telemetry::CLI,
                            run_id = %deepseekcode_telemetry::current_run_id(),
                            command = "diagnostics export",
                            error = %e,
                            "diagnostics export failed"
                        );
                        return Err(e.into());
                    }
                }
            }
        },
        Commands::Agent { command } => match command {
            AgentCommands::List { directory } => {
                // P1-11: list from effective config (not hardcoded) so custom
                // agents appear.
                let cfg =
                    deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory));
                match cfg {
                    Ok(c) => {
                        let mut names: Vec<_> = c.agents.keys().cloned().collect();
                        if names.is_empty() {
                            names = vec!["build".into(), "plan".into(), "general".into()];
                        }
                        names.sort();
                        for n in names {
                            println!("{n}");
                        }
                    }
                    Err(e) => {
                        println!("build\nplan\ngeneral");
                        eprintln!(
                            "{}",
                            deepseekcode_i18n::tr!("cli.out.config_defaults", "error" => e.to_string())
                        );
                    }
                }
            }
            AgentCommands::Set {
                directory,
                agent,
                session,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = resolve_session(&client, session.as_deref()).await?;
                let session = client.get_session(session_id).await?;
                // P1-11: defaults from effective config, not hardcoded
                // `gpt-4o-mini`/`openai` (previously diverged from Server).
                let cfg =
                    deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory))
                        .ok();
                let model = session
                    .model_id
                    .or_else(|| {
                        cfg.as_ref()
                            .and_then(|c| c.model.as_ref().and_then(|m| m.id.clone()))
                    })
                    .unwrap_or_else(|| "gpt-4o-mini".into());
                let provider = session
                    .provider_id
                    .or_else(|| {
                        cfg.as_ref()
                            .and_then(|c| c.model.as_ref().and_then(|m| m.provider.clone()))
                    })
                    .unwrap_or_else(|| "openai".into());
                client
                    .update_model(session_id, Some(&agent), &model, &provider)
                    .await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.agent_set", "agent" => agent.as_str())
                );
                shutdown_opt(engine).await;
            }
        },
        Commands::Model { command } => match command {
            ModelCommands::List { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for provider in client.list_providers().await? {
                    for model in provider.models {
                        println!("{} {} ({})", provider.id, model.name, model.id);
                    }
                }
                shutdown_opt(engine).await;
            }
            ModelCommands::Set {
                directory,
                model,
                provider,
                session,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = resolve_session(&client, session.as_deref()).await?;
                let provider_id = provider.unwrap_or_else(|| "openai".into());
                client
                    .update_model(session_id, None, &model, &provider_id)
                    .await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.model_set", "model" => model.as_str())
                );
                shutdown_opt(engine).await;
            }
        },
        Commands::Provider { command } => match command {
            ProviderCommands::List { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for p in client.list_providers().await? {
                    println!("{} {} connected={}", p.id, p.name, p.connected);
                }
                shutdown_opt(engine).await;
            }
            ProviderCommands::Connect {
                directory,
                provider,
                api_key,
                base_url,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client
                    .connect_provider(&provider, api_key, base_url)
                    .await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.provider_connected", "provider" => provider.as_str())
                );
                shutdown_opt(engine).await;
            }
            ProviderCommands::Disconnect {
                directory,
                provider,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.disconnect_provider(&provider).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.provider_disconnected", "provider" => provider.as_str())
                );
                shutdown_opt(engine).await;
            }
        },
        Commands::Prompt {
            directory,
            session,
            delivery,
            resume,
            text,
        } => {
            let (engine, client) =
                scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
            let session_id = resolve_session(&client, session.as_deref()).await?;
            let delivery = match delivery.as_str() {
                "steer" => PromptDelivery::Steer,
                _ => PromptDelivery::Queue,
            };
            client
                .prompt(PromptRequest {
                    id: None,
                    session_id,
                    prompt: text,
                    delivery,
                    resume,
                })
                .await?;
            println!("{}", deepseekcode_i18n::tr!("cli.out.prompt_submitted"));
            shutdown_opt(engine).await;
        }
        Commands::Session { command } => match command {
            SessionCommands::List { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for s in client.list_sessions().await? {
                    println!("{} {:?} {}", s.id, s.title, s.updated_at);
                }
                shutdown_opt(engine).await;
            }
            SessionCommands::Fork {
                directory,
                session,
                until,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let until_id = until.as_deref().map(MessageId::from_str).transpose()?;
                let forked = client.fork_session(session_id, until_id).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.session_forked", "session" => forked.id.to_string())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Export {
                directory,
                session,
                no_sanitize,
                output,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let data = client
                    .export_session_with_sanitize(session_id, !no_sanitize)
                    .await?;
                std::fs::write(&output, serde_json::to_string_pretty(&data)?)?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.exported_to", "path" => output.display().to_string())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Revert {
                directory,
                session,
                message,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let message_id = MessageId::from_str(&message)?;
                client.revert_stage(session_id, message_id).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.revert_staged", "message" => message.as_str())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Compact { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let installed = client.compact_session(session_id).await?;
                println!(
                    "{}",
                    if installed {
                        deepseekcode_i18n::tr!("cli.out.compaction_installed")
                    } else {
                        deepseekcode_i18n::tr!("cli.out.nothing_to_compact")
                    }
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Move {
                directory,
                session,
                target,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.move_session(session_id, &target).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.session_moved", "target" => target.as_str())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Interrupt { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.interrupt(session_id).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.session_interrupted", "session" => session_id.to_string())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Delete { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.delete_session(session_id).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.session_deleted", "session" => session_id.to_string())
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Todo { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&client.session_todo(session_id).await?)?
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Plans { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&client.list_session_plans(session_id).await?)?
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::Answer {
                directory,
                session,
                text,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.submit_question_answer(session_id, &text).await?;
                println!("{}", deepseekcode_i18n::tr!("cli.out.answer_submitted"));
                shutdown_opt(engine).await;
            }
            SessionCommands::RevertCommit { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&client.revert_commit(session_id).await?)?
                );
                shutdown_opt(engine).await;
            }
            SessionCommands::RevertClear { directory, session } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.revert_clear(session_id).await?;
                println!("{}", deepseekcode_i18n::tr!("cli.out.revert_cleared"));
                shutdown_opt(engine).await;
            }
        },
        Commands::Mcp { command } => match command {
            McpCommands::List { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for entry in client.list_mcp().await? {
                    println!("{}", serde_json::to_string(&entry)?);
                }
                shutdown_opt(engine).await;
            }
            McpCommands::Connect { directory, name } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.connect_mcp(&name).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.mcp_connected", "name" => name.as_str())
                );
                shutdown_opt(engine).await;
            }
            McpCommands::Disconnect { directory, name } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.disconnect_mcp(&name).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.mcp_disconnected", "name" => name.as_str())
                );
                shutdown_opt(engine).await;
            }
        },
        Commands::Worktree { command } => match command {
            WorktreeCommands::List { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for entry in client.list_worktrees().await? {
                    println!("{}", serde_json::to_string(&entry)?);
                }
                shutdown_opt(engine).await;
            }
            WorktreeCommands::Create { directory, branch } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let created = client.create_worktree(branch.as_deref()).await?;
                println!("{}", serde_json::to_string_pretty(&created)?);
                shutdown_opt(engine).await;
            }
            WorktreeCommands::Remove { directory, path } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.remove_worktree(&path).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.worktree_removed", "path" => path.as_str())
                );
                shutdown_opt(engine).await;
            }
            WorktreeCommands::Prune { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.prune_worktrees().await?;
                println!("{}", deepseekcode_i18n::tr!("cli.out.worktrees_pruned"));
                shutdown_opt(engine).await;
            }
        },
        Commands::Project { command } => match command {
            ProjectCommands::Directories { command } => match command {
                ProjectDirectoryCommands::List { directory } => {
                    let (engine, client) =
                        scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                    for entry in client.list_project_directories().await? {
                        println!("{}", serde_json::to_string(&entry)?);
                    }
                    shutdown_opt(engine).await;
                }
                ProjectDirectoryCommands::Add { directory, path } => {
                    let (engine, client) =
                        scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                    let entry = client.add_project_directory(&path, None).await?;
                    println!("{}", serde_json::to_string_pretty(&entry)?);
                    shutdown_opt(engine).await;
                }
            },
        },
        Commands::Fs { command } => match command {
            FsCommands::Tree {
                directory,
                path,
                offset,
                limit,
            } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                let page = client
                    .fs_tree_page(
                        if path.is_empty() { None } else { Some(&path) },
                        Some(offset),
                        Some(limit),
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&page)?);
                shutdown_opt(engine).await;
            }
            FsCommands::File { directory, path } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                print!("{}", client.fs_file(&path).await?);
                shutdown_opt(engine).await;
            }
        },
        Commands::Skills { directory } => {
            let (engine, client) =
                scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client.list_skills().await?)?
            );
            shutdown_opt(engine).await;
        }
        Commands::Plugins { directory } => {
            let (engine, client) =
                scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client.list_plugins().await?)?
            );
            shutdown_opt(engine).await;
        }
        Commands::Config { directory } => {
            let cfg =
                deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory))?;
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        Commands::Health {
            directory,
            url: flag_url,
        } => {
            // Per-command `--url` wins over global `--url`.
            let endpoint = flag_url.as_deref().or(url.as_deref());
            let (engine, client) = scoped_client(endpoint, password.as_deref(), &directory).await?;
            client.health().await?;
            println!("ok");
            shutdown_opt(engine).await;
        }
        Commands::Policy { command } => match command {
            PolicyCommands::Pending { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for req in client.list_pending_policies().await? {
                    // Show full decision context (previously only id/scope).
                    println!(
                        "{} scope={} tool={} patterns={:?} remember={:?} status={:?} desc={}",
                        req.id,
                        req.scope,
                        req.tool_name,
                        req.patterns,
                        req.remember_patterns,
                        req.status,
                        req.description,
                    );
                }
                shutdown_opt(engine).await;
            }
            PolicyCommands::Grants { directory } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                for grant in client.list_policy_grants().await? {
                    println!(
                        "{} scope={} pattern={} created={}",
                        grant.id, grant.scope, grant.pattern, grant.created_at
                    );
                }
                shutdown_opt(engine).await;
            }
            PolicyCommands::GrantDelete { directory, id } => {
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client.delete_policy_grant(&id).await?;
                println!(
                    "{}",
                    deepseekcode_i18n::tr!("cli.out.grant_deleted", "id" => id.as_str())
                );
                shutdown_opt(engine).await;
            }
            PolicyCommands::Reply {
                directory,
                id,
                reply,
                message,
            } => {
                use deepseekcode_schema::policy::{PolicyReply, PolicyReplyKind};
                // Validate ULID id early (previously raw String passed through).
                if deepseekcode_types::PermissionId::from_str(&id).is_err() {
                    anyhow::bail!(
                        "{}",
                        deepseekcode_i18n::tr!("cli.err.invalid_policy_id", "id" => id.as_str())
                    );
                }
                let kind = match reply.to_ascii_lowercase().as_str() {
                    "once" => PolicyReplyKind::Once,
                    "allow" => PolicyReplyKind::Once,
                    "permit" => PolicyReplyKind::Once,
                    "project" | "always" => PolicyReplyKind::Project,
                    "reject" | "deny" => PolicyReplyKind::Reject,
                    other => anyhow::bail!(
                        "{}",
                        deepseekcode_i18n::tr!("cli.err.unknown_reply_kind", "kind" => other)
                    ),
                };
                let (engine, client) =
                    scoped_client(url.as_deref(), password.as_deref(), &directory).await?;
                client
                    .reply_policy(
                        &id,
                        PolicyReply {
                            reply: kind,
                            message,
                        },
                    )
                    .await?;
                println!("ok");
                shutdown_opt(engine).await;
            }
        },
    }
    Ok(())
}

/// Remote-or-embedded client scope (P1-11): `url` (`--url` /
/// `DEEPSEEKCODE_SERVER_URL`) routes to a remote server instead of starting
/// an embedded engine, so a `serve --password` remote is fully operable.
/// Returns the client plus the owned engine when embedded (`None` for
/// remote); keep it alive for the command duration via `shutdown_opt`.
async fn scoped_client(
    url: Option<&str>,
    password: Option<&str>,
    directory: &str,
) -> anyhow::Result<(Option<EmbeddedEngine>, DeepSeekCodeClient)> {
    let pw = password
        .map(str::to_string)
        .or_else(|| std::env::var("DEEPSEEKCODE_SERVER_PASSWORD").ok());
    if let Some(url) = url {
        let mut client = DeepSeekCodeClient::new(url, directory.to_string());
        if let Some(password) = pw {
            client = client.with_auth("deepseekcode", password);
        }
        // Fail fast on unreachable remote.
        client.health().await?;
        return Ok((None, client));
    }
    let engine = EmbeddedEngine::start(directory).await?;
    let mut client =
        DeepSeekCodeClient::new(format!("http://{}", engine.addr), directory.to_string());
    if let Some(password) = pw {
        client = client.with_auth("deepseekcode", password);
    }
    Ok((Some(engine), client))
}

async fn shutdown_opt(engine: Option<EmbeddedEngine>) {
    if let Some(engine) = engine {
        engine.shutdown().await;
    }
}

async fn resolve_session(
    client: &DeepSeekCodeClient,
    session: Option<&str>,
) -> anyhow::Result<SessionId> {
    if let Some(id) = session {
        return Ok(SessionId::from_str(id)?);
    }
    let sessions = client.list_sessions().await?;
    let latest = sessions
        .into_iter()
        .max_by_key(|s| s.updated_at)
        .ok_or_else(|| anyhow::anyhow!(deepseekcode_i18n::tr!("cli.err.no_sessions")))?;
    Ok(latest.id)
}

fn load_logging_config_for(directory: &str) -> (LoggingConfig, Option<String>) {
    // P1-11: respect `--directory` (previously hardcoded `.`, ignoring the flag).
    match deepseekcode_config::load_effective_config(camino::Utf8Path::new(directory)) {
        Ok(c) => (c.logging, None),
        Err(e) => {
            // Stage one: telemetry not yet initialized, stderr only.
            // Stage two (in `run` after `init`) persists the same to app.log.
            let msg = deepseekcode_i18n::tr!("cli.err.config_load_failed", "directory" => directory, "error" => e.to_string());
            eprintln!("deepseekcode: {msg}");
            (Default::default(), Some(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn receipt_ignores_stored_english_and_gates_display() {
        // A stored English receipt must not leak into a non-English CLI: the
        // numeric fields are the source of truth and are rendered for the
        // active locale.
        let payload = json!({
            "input_tokens": 12000u64,
            "output_tokens": 100u64,
            "cache_receipt": " (10800 cached / 1200 new)",
            "cache_hit": 10800u64,
            "cache_miss": 1200u64,
        });
        let line = turn_receipt_line(&payload, true).expect("receipt");
        assert!(
            line.contains("10,800") && line.contains("out 100"),
            "{line}"
        );
        assert_eq!(turn_receipt_line(&payload, false), None);
    }

    #[test]
    fn receipt_rebuilds_and_reports_write() {
        let payload = json!({
            "input_tokens": 12000u64,
            "output_tokens": 100u64,
            "cache_hit": 10800u64,
            "cache_miss": 1200u64,
            "cache_write": 300u64,
        });
        let line = turn_receipt_line(&payload, true).expect("receipt");
        assert!(
            line.contains("10,800") && line.contains("300 write"),
            "{line}"
        );
    }

    #[test]
    fn receipt_absent_without_usage() {
        assert_eq!(turn_receipt_line(&json!({"content": "hi"}), true), None);
    }
}
