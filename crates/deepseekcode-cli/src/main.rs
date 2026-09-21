use clap::{Parser, Subcommand};
use deepseekcode_config::LoggingConfig;
use deepseekcode_schema::session::{CreateSessionRequest, PromptDelivery, PromptRequest};
use deepseekcode_sdk::{client::DeepSeekCodeClient, EmbeddedEngine};
use deepseekcode_telemetry::init;
use deepseekcode_types::{MessageId, SessionId};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Parser)]
#[command(name = "deepseekcode", about = "DeepSeekCode command-line interface")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a single prompt non-interactively
    Run {
        #[arg(short, long)]
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
        #[arg(long, default_value = "queue")]
        delivery: String,
        #[arg(long, default_value = "true")]
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
    let logging = load_logging_config();
    let run_ctx = init("cli", &logging);
    tracing::info!(
        target: deepseekcode_telemetry::APP,
        run_id = %run_ctx.run_id,
        "deepseekcode-cli starting"
    );

    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            directory,
            auto,
            prompt,
        } => {
            if auto {
                std::env::set_var("DEEPSEEKCODE_POLICY_MODE", "auto");
            }
            let (engine, client) = scoped_client(&directory).await?;
            let session = client
                .create_session(CreateSessionRequest {
                    id: None,
                    directory: directory.clone(),
                    agent: None,
                    model_id: None,
                    provider_id: None,
                })
                .await?;
            let _message_id = client
                .prompt(PromptRequest {
                    id: None,
                    session_id: session.id,
                    prompt,
                    delivery: PromptDelivery::Queue,
                    resume: true,
                })
                .await?;
            use deepseekcode_schema::event::EventKind;
            use deepseekcode_sdk::EventStreamItem;
            use futures::StreamExt;
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
                                        for event in events {
                                            if event.session_id != Some(session.id) {
                                                continue;
                                            }
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
                                        }
                                        // Advance only: never move backwards.
                                        // The server's `head` can lag behind what
                                        // this client already applied (e.g. after
                                        // a broadcast-lag gap).
                                        if head > since_seq {
                                            since_seq = head;
                                        }
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
                                    EventKind::SessionIdle => return Ok(()),
                                    EventKind::SessionInterrupted => {
                                        return Err(anyhow::anyhow!("session interrupted"));
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
                Err(anyhow::anyhow!("event stream ended before session idle"))
            })
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    eprintln!("{e}");
                    std::process::exit(1);
                }
                Err(_) => {
                    eprintln!("timed out waiting for session idle");
                    std::process::exit(1);
                }
            }
            engine.shutdown().await;
        }
        Commands::Serve {
            directory,
            port,
            auto,
        } => {
            if auto {
                std::env::set_var("DEEPSEEKCODE_POLICY_MODE", "auto");
            }
            let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
            let db_path = dirs::data_dir()
                .map(|p| p.join("deepseekcode").join("deepseekcode.db"))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "deepseekcode.db".into());
            let db_url = format!("sqlite://{db_path}");
            deepseekcode_server::serve(directory, db_url, addr, None).await?;
        }
        Commands::Upgrade { check_only } => {
            if check_only {
                println!(
                    "DeepSeekCode {} — update check not configured",
                    env!("CARGO_PKG_VERSION")
                );
            } else {
                println!("Auto-update requires a release channel; use package manager for now.");
            }
        }
        Commands::SessionList { directory } => {
            let (engine, client) = scoped_client(&directory).await?;
            for s in client.list_sessions().await? {
                println!("{} {:?}", s.id, s.title);
            }
            engine.shutdown().await;
        }
        Commands::Diagnostics { command } => match command {
            DiagnosticsCommands::Export { output, run_id } => {
                let path =
                    deepseekcode_telemetry::export_diagnostic_bundle(run_id.as_deref(), &output)?;
                println!("Exported diagnostics to {}", path.display());
                println!("run_id: {}", run_ctx.run_id);
            }
        },
        Commands::Agent { command } => match command {
            AgentCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                println!("build, plan, general (session agent is set per prompt)");
                let _ = client;
                engine.shutdown().await;
            }
            AgentCommands::Set {
                directory,
                agent,
                session,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = resolve_session(&client, session.as_deref()).await?;
                let session = client.get_session(session_id).await?;
                let model = session.model_id.unwrap_or_else(|| "gpt-4o-mini".into());
                let provider = session.provider_id.unwrap_or_else(|| "openai".into());
                client
                    .update_model(session_id, Some(&agent), &model, &provider)
                    .await?;
                println!("agent set to {agent}");
                engine.shutdown().await;
            }
        },
        Commands::Model { command } => match command {
            ModelCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for provider in client.list_providers().await? {
                    for model in provider.models {
                        println!("{} {} ({})", provider.id, model.name, model.id);
                    }
                }
                engine.shutdown().await;
            }
            ModelCommands::Set {
                directory,
                model,
                provider,
                session,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = resolve_session(&client, session.as_deref()).await?;
                let provider_id = provider.unwrap_or_else(|| "openai".into());
                client
                    .update_model(session_id, None, &model, &provider_id)
                    .await?;
                println!("model set to {model}");
                engine.shutdown().await;
            }
        },
        Commands::Provider { command } => match command {
            ProviderCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for p in client.list_providers().await? {
                    println!("{} {} connected={}", p.id, p.name, p.connected);
                }
                engine.shutdown().await;
            }
            ProviderCommands::Connect {
                directory,
                provider,
                api_key,
                base_url,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                client
                    .connect_provider(&provider, api_key, base_url)
                    .await?;
                println!("connected {provider}");
                engine.shutdown().await;
            }
            ProviderCommands::Disconnect {
                directory,
                provider,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                client.disconnect_provider(&provider).await?;
                println!("disconnected {provider}");
                engine.shutdown().await;
            }
        },
        Commands::Prompt {
            directory,
            session,
            delivery,
            resume,
            text,
        } => {
            let (engine, client) = scoped_client(&directory).await?;
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
            println!("prompt submitted");
            engine.shutdown().await;
        }
        Commands::Session { command } => match command {
            SessionCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for s in client.list_sessions().await? {
                    println!("{} {:?} {}", s.id, s.title, s.updated_at);
                }
                engine.shutdown().await;
            }
            SessionCommands::Fork {
                directory,
                session,
                until,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let until_id = until.as_deref().map(MessageId::from_str).transpose()?;
                let forked = client.fork_session(session_id, until_id).await?;
                println!("forked session {}", forked.id);
                engine.shutdown().await;
            }
            SessionCommands::Export {
                directory,
                session,
                no_sanitize,
                output,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let data = client
                    .export_session_with_sanitize(session_id, !no_sanitize)
                    .await?;
                std::fs::write(&output, serde_json::to_string_pretty(&data)?)?;
                println!("exported to {}", output.display());
                engine.shutdown().await;
            }
            SessionCommands::Revert {
                directory,
                session,
                message,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = SessionId::from_str(&session)?;
                let message_id = MessageId::from_str(&message)?;
                client.revert_stage(session_id, message_id).await?;
                println!("revert staged at {message}");
                engine.shutdown().await;
            }
            SessionCommands::Compact { directory, session } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.compact_session(session_id).await?;
                println!("compaction requested");
                engine.shutdown().await;
            }
            SessionCommands::Move {
                directory,
                session,
                target,
            } => {
                let (engine, client) = scoped_client(&directory).await?;
                let session_id = SessionId::from_str(&session)?;
                client.move_session(session_id, &target).await?;
                println!("session moved to {target}");
                engine.shutdown().await;
            }
        },
        Commands::Mcp { command } => match command {
            McpCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for entry in client.list_mcp().await? {
                    println!("{}", serde_json::to_string(&entry)?);
                }
                engine.shutdown().await;
            }
            McpCommands::Connect { directory, name } => {
                let (engine, client) = scoped_client(&directory).await?;
                client.connect_mcp(&name).await?;
                println!("mcp {name} connected");
                engine.shutdown().await;
            }
            McpCommands::Disconnect { directory, name } => {
                let (engine, client) = scoped_client(&directory).await?;
                client.disconnect_mcp(&name).await?;
                println!("mcp {name} disconnected");
                engine.shutdown().await;
            }
        },
        Commands::Worktree { command } => match command {
            WorktreeCommands::List { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for entry in client.list_worktrees().await? {
                    println!("{}", serde_json::to_string(&entry)?);
                }
                engine.shutdown().await;
            }
            WorktreeCommands::Create { directory, branch } => {
                let (engine, client) = scoped_client(&directory).await?;
                let created = client.create_worktree(branch.as_deref()).await?;
                println!("{}", serde_json::to_string_pretty(&created)?);
                engine.shutdown().await;
            }
            WorktreeCommands::Remove { directory, path } => {
                let (engine, client) = scoped_client(&directory).await?;
                client.remove_worktree(&path).await?;
                println!("removed worktree {path}");
                engine.shutdown().await;
            }
            WorktreeCommands::Prune { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                client.prune_worktrees().await?;
                println!("pruned stale worktrees");
                engine.shutdown().await;
            }
        },
        Commands::Project { command } => match command {
            ProjectCommands::Directories { command } => match command {
                ProjectDirectoryCommands::List { directory } => {
                    let (engine, client) = scoped_client(&directory).await?;
                    for entry in client.list_project_directories().await? {
                        println!("{}", serde_json::to_string(&entry)?);
                    }
                    engine.shutdown().await;
                }
                ProjectDirectoryCommands::Add { directory, path } => {
                    let (engine, client) = scoped_client(&directory).await?;
                    let entry = client.add_project_directory(&path, None).await?;
                    println!("{}", serde_json::to_string_pretty(&entry)?);
                    engine.shutdown().await;
                }
            },
        },
        Commands::Config { directory } => {
            let cfg =
                deepseekcode_config::load_effective_config(camino::Utf8Path::new(&directory))?;
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        Commands::Health { directory, url } => {
            if let Some(url) = url {
                let client = DeepSeekCodeClient::new(url, directory);
                client.health().await?;
                println!("ok");
            } else {
                let (engine, client) = scoped_client(&directory).await?;
                client.health().await?;
                println!("ok");
                engine.shutdown().await;
            }
        }
        Commands::Policy { command } => match command {
            PolicyCommands::Pending { directory } => {
                let (engine, client) = scoped_client(&directory).await?;
                for req in client.list_pending_policies().await? {
                    println!(
                        "{} {} {} {:?}",
                        req.id, req.scope, req.tool_name, req.patterns
                    );
                }
                engine.shutdown().await;
            }
            PolicyCommands::Reply {
                directory,
                id,
                reply,
                message,
            } => {
                use deepseekcode_schema::policy::{PolicyReply, PolicyReplyKind};
                let kind = match reply.to_ascii_lowercase().as_str() {
                    "once" | "allow" | "permit" => PolicyReplyKind::Once,
                    "project" | "always" => PolicyReplyKind::Project,
                    "reject" | "deny" => PolicyReplyKind::Reject,
                    other => anyhow::bail!("unknown reply kind: {other}"),
                };
                let (engine, client) = scoped_client(&directory).await?;
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
                engine.shutdown().await;
            }
        },
    }
    Ok(())
}

async fn scoped_client(directory: &str) -> anyhow::Result<(EmbeddedEngine, DeepSeekCodeClient)> {
    let engine = EmbeddedEngine::start(directory).await?;
    let mut client =
        DeepSeekCodeClient::new(format!("http://{}", engine.addr), directory.to_string());
    if let Ok(password) = std::env::var("DEEPSEEKCODE_SERVER_PASSWORD") {
        client = client.with_auth("deepseekcode", password);
    }
    Ok((engine, client))
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
        .ok_or_else(|| anyhow::anyhow!("no sessions — create one first"))?;
    Ok(latest.id)
}

fn load_logging_config() -> LoggingConfig {
    deepseekcode_config::load_effective_config(camino::Utf8Path::new("."))
        .map(|c| c.logging)
        .unwrap_or_default()
}
