//! CLI entry point for Rusty (Rusty orchestration daemon).

use clap::{Args, Parser, Subcommand};
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
                    rusty run --yolo --port 4000  # Start with web dashboard\n  \
                    rusty dashboard --url http://127.0.0.1:4000  # Open terminal dashboard\n\n\
                  Docs: https://github.com/ridermw/rusty/blob/main/rust/README.md",
    after_help = "Environment variables:\n  \
                  GITHUB_TOKEN/GH_TOKEN  GitHub API token (or use gh auth login)\n  \
                  RUST_LOG               Log level filter (default: info)\n\n\
                  Examples:\n  \
                  rusty run --yolo\n  \
                  rusty run --yolo --port 4000 --logs-root ./logs\n  \
                  rusty run --yolo path/to/WORKFLOW.md\n  \
                  rusty dashboard --url http://127.0.0.1:4000\n  \
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
    /// Watch orchestrator status in a terminal dashboard
    Dashboard(DashboardArgs),
    /// Interactive first-time setup
    Setup,
}

#[derive(Args, Debug, Clone)]
pub struct DashboardArgs {
    /// Dashboard API URL (default: http://127.0.0.1:8080)
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub url: String,

    /// Refresh interval in seconds
    #[arg(long, default_value = "2")]
    pub refresh: u64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubAuthStatus {
    Environment { length: usize },
    GhCli { length: usize },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowFileStatus {
    CurrentDir,
    RustDir,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrerequisiteReport {
    pub github_auth: GitHubAuthStatus,
    pub workflow_file: WorkflowFileStatus,
    pub copilot_cli_version: Option<String>,
    pub github_cli_version: Option<String>,
}

pub fn resolve_workspace_root(config: &crate::config::schema::RustyConfig) -> PathBuf {
    config
        .workspace
        .root
        .as_deref()
        .map(|root| {
            // Resolve $VAR env references first, then expand ~
            let resolved =
                crate::config::resolve_env_value(root).unwrap_or_else(|_| root.to_string());
            crate::config::expand_home(&resolved)
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("rusty_workspaces"))
}

pub fn check_prerequisites() -> PrerequisiteReport {
    let env_token = std::env::var("GITHUB_TOKEN")
        .or_else(|_| std::env::var("GH_TOKEN"))
        .ok()
        .filter(|token| !token.is_empty());

    let github_auth = if let Some(token) = env_token {
        GitHubAuthStatus::Environment {
            length: token.len(),
        }
    } else if let Some(token) = command_stdout("gh", &["auth", "token"]) {
        GitHubAuthStatus::GhCli {
            length: token.len(),
        }
    } else {
        GitHubAuthStatus::Missing
    };

    let workflow_file = if std::path::Path::new("WORKFLOW.md").exists() {
        WorkflowFileStatus::CurrentDir
    } else if std::path::Path::new("rust/WORKFLOW.md").exists() {
        WorkflowFileStatus::RustDir
    } else {
        WorkflowFileStatus::Missing
    };

    PrerequisiteReport {
        github_auth,
        workflow_file,
        copilot_cli_version: command_stdout("copilot", &["--version"]),
        github_cli_version: command_stdout("gh", &["--version"])
            .map(|version| version.lines().next().unwrap_or("unknown").to_string()),
    }
}

fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        None
    } else {
        Some(stdout)
    }
}

pub async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Setup => run_setup().await,
        Commands::Run(args) => run_daemon(args).await,
        Commands::Dashboard(args) => crate::tui::run_dashboard(args).await,
    }
}

async fn run_setup() -> anyhow::Result<()> {
    println!("🦀 Rusty Setup v{VERSION}");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let report = check_prerequisites();

    print!("1. Checking GitHub auth... ");
    match report.github_auth {
        GitHubAuthStatus::Environment { length } => {
            println!("✅ set via env ({} chars)", length);
        }
        GitHubAuthStatus::GhCli { length } => {
            println!("✅ set via gh auth ({} chars)", length);
        }
        GitHubAuthStatus::Missing => print_token_missing(),
    }

    // Check for WORKFLOW.md
    print!("2. Checking WORKFLOW.md... ");
    match report.workflow_file {
        WorkflowFileStatus::CurrentDir => println!("✅ found in current directory"),
        WorkflowFileStatus::RustDir => {
            println!("⚠️  found at rust/WORKFLOW.md but not in current directory");
            println!("   Copy it: copy rust\\WORKFLOW.md .\\WORKFLOW.md");
        }
        WorkflowFileStatus::Missing => {
            println!("❌ not found");
            println!("   Create one or copy the template from rust/WORKFLOW.md");
        }
    }

    // Check for Copilot CLI
    print!("3. Checking Copilot CLI... ");
    if let Some(version) = report.copilot_cli_version {
        println!("✅ {version}");
    } else {
        println!("❌ not found");
        println!("   Install: https://docs.github.com/en/copilot/github-copilot-in-the-cli");
    }

    // Check for gh CLI
    print!("4. Checking GitHub CLI... ");
    if let Some(version) = report.github_cli_version {
        println!("✅ {version}");
    } else {
        println!("❌ not found");
        println!("   Install: https://cli.github.com/");
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

    let workspace_root = resolve_workspace_root(&config);
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

    let (sse_tx, _) = tokio::sync::broadcast::channel::<crate::orchestrator::OrchestratorSnapshot>(64);

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
        let sse = sse_tx.clone();
        let (server_ready_tx, server_ready_rx) =
            tokio::sync::oneshot::channel::<Result<(), String>>();
        tokio::spawn(async move {
            let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => {
                    let _ = server_ready_tx.send(Ok(()));
                    let app = crate::server::api::build_router_with_sse(tx, Some(sse));
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

    crate::orchestrator::run_orchestrator_with_sse(
        orch_state,
        config,
        tracker,
        workflow.prompt_template,
        workspace_root,
        shell_executor,
        orch_rx,
        orch_tx,
        Some(sse_tx),
    )
    .await;

    println!("\n🛑 Shutdown complete");
    Ok(())
}
