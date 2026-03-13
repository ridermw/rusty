//! CLI entry point for Rusty (Rusty orchestration daemon).

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing::{error, info};

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(
    name = "rusty",
    version = VERSION,
    about = "Rusty — Rusty orchestration daemon for GitHub Issues + Copilot CLI",
    long_about = "Rusty is a long-running daemon that polls GitHub Issues, creates isolated\n\
                  per-issue workspaces, and orchestrates Copilot CLI coding agent sessions.\n\n\
                  Quick start:\n  \
                    rusty setup              # Interactive first-time setup\n  \
                    rusty run --yolo         # Start the daemon\n  \
                    rusty run --yolo --port 4000  # Start with web dashboard\n\n\
                  Docs: https://github.com/ridermw/rusty/blob/main/rust/README.md",
    after_help = "Environment variables:\n  \
                  GITHUB_TOKEN/GH_TOKEN  GitHub API token (or use gh auth login)\n  \
                  RUST_LOG               Log level filter (default: info)\n\n\
                  Examples:\n  \
                  rusty run --yolo\n  \
                  rusty run --yolo --port 4000 --logs-root ./logs\n  \
                  rusty run --yolo path/to/WORKFLOW.md\n  \
                  rusty setup"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the orchestration daemon
    Run(RunArgs),
    /// Interactive first-time setup
    Setup,
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Path to WORKFLOW.md file
    #[arg(default_value = "WORKFLOW.md")]
    pub workflow_path: PathBuf,

    /// HTTP server port (overrides server.port in WORKFLOW.md)
    #[arg(long)]
    pub port: Option<u16>,

    /// Log files directory (default: ./logs next to the executable)
    #[arg(long)]
    pub logs_root: Option<PathBuf>,

    /// Acknowledge autonomous agent execution (required to start)
    #[arg(long)]
    pub yolo: bool,
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup => run_setup().await,
        Commands::Run(args) => run_daemon(args).await,
    }
}

async fn run_setup() -> anyhow::Result<()> {
    println!("🦀 Rusty Setup v{VERSION}");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    print!("1. Checking GitHub auth... ");
    let env_token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|t| !t.is_empty());

    if let Some(t) = env_token {
        println!("✅ set via env ({} chars)", t.len());
    } else {
        match tokio::process::Command::new("gh")
            .args(["auth", "token"])
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let t = String::from_utf8_lossy(&output.stdout);
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    println!("✅ set via gh auth ({} chars)", trimmed.len());
                } else {
                    print_token_missing();
                }
            }
            _ => print_token_missing(),
        }
    }

    // Check for WORKFLOW.md
    print!("2. Checking WORKFLOW.md... ");
    if std::path::Path::new("WORKFLOW.md").exists() {
        println!("✅ found in current directory");
    } else if std::path::Path::new("rust/WORKFLOW.md").exists() {
        println!("⚠️  found at rust/WORKFLOW.md but not in current directory");
        println!("   Copy it: copy rust\\WORKFLOW.md .\\WORKFLOW.md");
    } else {
        println!("❌ not found");
        println!("   Create one or copy the template from rust/WORKFLOW.md");
    }

    // Check for Copilot CLI
    print!("3. Checking Copilot CLI... ");
    match tokio::process::Command::new("copilot")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let ver = String::from_utf8_lossy(&output.stdout);
            println!("✅ {}", ver.trim());
        }
        _ => {
            println!("❌ not found");
            println!("   Install: https://docs.github.com/en/copilot/github-copilot-in-the-cli");
        }
    }

    // Check for gh CLI
    print!("4. Checking GitHub CLI... ");
    match tokio::process::Command::new("gh")
        .arg("--version")
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let ver = String::from_utf8_lossy(&output.stdout);
            let first_line = ver.lines().next().unwrap_or("unknown");
            println!("✅ {first_line}");
        }
        _ => {
            println!("❌ not found");
            println!("   Install: https://cli.github.com/");
        }
    }

    // Check logs directory
    let logs_dir = PathBuf::from("logs");
    print!("5. Checking logs directory... ");
    if logs_dir.exists() {
        println!("✅ ./logs/ exists");
    } else {
        println!("📁 creating ./logs/");
        std::fs::create_dir_all(&logs_dir)?;
    }

    println!();
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Ready to run:");
    println!("  rusty run --yolo");
    println!("  rusty run --yolo --port 4000   # with web dashboard");
    println!();

    Ok(())
}

fn print_token_missing() {
    println!("❌ not found");
    println!("   Option 1: gh auth login");
    println!("   Option 2: $env:GITHUB_TOKEN = \"ghp_your_token_here\"");
    println!("   Required scopes: repo, read:discussion, project");
}

async fn run_daemon(args: RunArgs) -> anyhow::Result<()> {
    if !args.yolo {
        anyhow::bail!(
            "Rusty requires the --yolo flag to start.\n\
             This acknowledges that Rusty will run coding agents autonomously.\n\n\
             Usage: rusty run --yolo"
        );
    }

    // Default logs_root to ./logs next to the executable, create if needed
    let logs_root = args.logs_root.unwrap_or_else(|| PathBuf::from("logs"));
    if let Err(e) = std::fs::create_dir_all(&logs_root) {
        eprintln!(
            "Warning: could not create logs directory {}: {e}",
            logs_root.display()
        );
        eprintln!("Continuing with stderr-only logging.");
    }

    let _log_guard = crate::logging::init_logging(Some(&logs_root)).unwrap_or_else(|e| {
        eprintln!("Warning: file logging unavailable: {e}");
        None
    });

    println!("🦀 Rusty v{VERSION} starting...");
    info!(workflow = %args.workflow_path.display(), port = ?args.port, "Rusty starting");

    if !args.workflow_path.exists() {
        anyhow::bail!(
            "Workflow file not found: {}\n\n\
             Run 'rusty setup' to check your configuration, or provide a path:\n  \
             rusty run --yolo path/to/WORKFLOW.md",
            args.workflow_path.display()
        );
    }

    let workflow = crate::workflow::load_workflow(&args.workflow_path)?;

    let config: crate::config::schema::RustyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&workflow.config)?)?;

    crate::config::validate_dispatch_config(&config).await?;
    info!("Configuration validated");

    let workspace_root = match config.workspace.root.as_deref() {
        Some(raw) => std::path::PathBuf::from(crate::config::expand_home(raw)),
        None => std::env::temp_dir().join("rusty_workspaces"),
    };
    std::fs::create_dir_all(&workspace_root)
        .map_err(|e| anyhow::anyhow!("failed to create workspace root: {e}"))?;

    let shell_executor: std::sync::Arc<dyn crate::workspace::hooks::ShellExecutor> =
        std::sync::Arc::from(crate::workspace::hooks::default_shell_executor());

    let tracker: std::sync::Arc<dyn crate::tracker::Tracker> = std::sync::Arc::new(
        crate::tracker::github::adapter::GitHubAdapter::new(config.tracker.clone()),
    );

    let orch_state = crate::orchestrator::state::OrchestratorState::new(
        config.polling.interval_ms,
        config.agent.max_concurrent_agents,
    );

    let (orch_tx, orch_rx) =
        tokio::sync::mpsc::channel::<crate::orchestrator::OrchestratorMsg>(256);

    let shutdown_tx = orch_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx
                .send(crate::orchestrator::OrchestratorMsg::Shutdown)
                .await;
        }
    });

    if let Some(port) = args.port.or(config.server.port) {
        let tx = orch_tx.clone();
        let (server_ready_tx, server_ready_rx) =
            tokio::sync::oneshot::channel::<Result<(), String>>();
        tokio::spawn(async move {
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    let _ = server_ready_tx.send(Ok(()));
                    let app = crate::server::api::build_router(tx);
                    info!(%addr, "HTTP server listening");
                    if let Err(e) = axum::serve(listener, app).await {
                        error!(error = %e, "HTTP server failed");
                    }
                }
                Err(e) => {
                    let _ = server_ready_tx.send(Err(e.to_string()));
                }
            }
        });
        match server_ready_rx.await {
            Ok(Ok(())) => {
                println!("🌐 Dashboard: http://127.0.0.1:{port}/");
                info!(port, "HTTP server started");
            }
            Ok(Err(e)) => anyhow::bail!("HTTP server failed to bind port {port}: {e}"),
            Err(_) => anyhow::bail!("HTTP server task died before binding"),
        }
    }

    println!("✅ Rusty is running. Press Ctrl+C to stop.");

    crate::orchestrator::run_orchestrator(
        orch_state,
        config,
        tracker,
        workflow.prompt_template,
        workspace_root,
        shell_executor,
        orch_rx,
        orch_tx,
    )
    .await;

    println!("\n🛑 Shutdown complete");
    Ok(())
}
