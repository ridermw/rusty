//! CLI entry point for Symphony.

use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info};

#[derive(Parser, Debug)]
#[command(name = "symphony", about = "Symphony orchestration daemon")]
pub struct Args {
    /// Path to WORKFLOW.md file
    #[arg(default_value = "WORKFLOW.md")]
    pub workflow_path: PathBuf,

    /// HTTP server port (overrides server.port in WORKFLOW.md)
    #[arg(long)]
    pub port: Option<u16>,

    /// Log files directory
    #[arg(long)]
    pub logs_root: Option<PathBuf>,

    /// Required safety acknowledgement flag
    #[arg(long = "i-understand-that-this-will-be-running-without-the-usual-guardrails")]
    pub guardrails_acknowledged: bool,
}

pub async fn run() -> anyhow::Result<()> {
    let args = Args::parse();

    if !args.guardrails_acknowledged {
        anyhow::bail!(
            "Symphony requires the --i-understand-that-this-will-be-running-without-the-usual-guardrails flag.\n\
             This acknowledges that Symphony will run coding agents autonomously."
        );
    }

    let _log_guard = crate::logging::init_logging(args.logs_root.as_deref())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    info!(workflow = %args.workflow_path.display(), port = ?args.port, "Symphony starting");

    if !args.workflow_path.exists() {
        anyhow::bail!("Workflow file not found: {}", args.workflow_path.display());
    }

    let workflow = crate::workflow::load_workflow(&args.workflow_path)?;

    let config: crate::config::schema::SymphonyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&workflow.config)?)?;

    crate::config::validate_dispatch_config(&config)?;
    info!("Configuration validated");

    let (orch_tx, _orch_rx) =
        tokio::sync::mpsc::channel::<crate::orchestrator::OrchestratorMsg>(256);

    if let Some(port) = args.port.or(config.server.port) {
        let tx = orch_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::server::start_server(port, tx).await {
                error!(error = %e, "HTTP server failed");
            }
        });
        info!(port, "HTTP server started");
    }

    info!("Symphony running. Press Ctrl+C to stop.");
    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received");

    Ok(())
}
