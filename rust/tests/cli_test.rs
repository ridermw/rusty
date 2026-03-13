use clap::Parser;
use rusty::cli::{
    check_prerequisites, resolve_workspace_root, Cli, Commands, GitHubAuthStatus,
    WorkflowFileStatus,
};
use rusty::config::expand_home;
use rusty::config::schema::RustyConfig;
use std::sync::{Mutex, MutexGuard, OnceLock};

#[test]
fn cli_run_default_workflow_path() {
    let cli = Cli::parse_from(["rusty", "run", "--yolo"]);
    match cli.command {
        Commands::Run(args) => {
            assert_eq!(args.workflow_path.to_str().unwrap(), "WORKFLOW.md");
            assert!(args.yolo);
            assert_eq!(args.port, None);
            assert_eq!(args.logs_root, None);
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn cli_run_custom_workflow_path() {
    let cli = Cli::parse_from(["rusty", "run", "--yolo", "custom/WORKFLOW.md"]);
    match cli.command {
        Commands::Run(args) => {
            assert_eq!(args.workflow_path.to_str().unwrap(), "custom/WORKFLOW.md");
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn cli_run_with_port_and_logs() {
    let cli = Cli::parse_from([
        "rusty",
        "run",
        "--yolo",
        "--port",
        "4000",
        "--logs-root",
        "./logs",
    ]);
    match cli.command {
        Commands::Run(args) => {
            assert_eq!(args.port, Some(4000));
            assert_eq!(args.logs_root.unwrap().to_str().unwrap(), "./logs");
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn cli_run_yolo_defaults_false() {
    let cli = Cli::parse_from(["rusty", "run"]);
    match cli.command {
        Commands::Run(args) => assert!(!args.yolo),
        _ => panic!("expected Run command"),
    }
}

/// Guardrails check: the daemon must not start without explicit acknowledgement.
/// The --yolo flag defaults to false, meaning the run() function will bail
/// before any autonomous agent execution begins.
#[test]
fn cli_guardrails_require_explicit_acknowledgement() {
    let cli = Cli::parse_from(["rusty", "run"]);
    match cli.command {
        Commands::Run(args) => {
            assert!(
                !args.yolo,
                "yolo flag must default to false — autonomous execution requires explicit opt-in"
            );
        }
        _ => panic!("expected Run command"),
    }
}

#[test]
fn cli_setup_subcommand() {
    let cli = Cli::parse_from(["rusty", "setup"]);
    assert!(matches!(cli.command, Commands::Setup));
}

#[test]
fn cli_version_flag() {
    let result = Cli::try_parse_from(["rusty", "--version"]);
    assert!(result.is_err()); // --version causes clap to exit
}

#[test]
fn resolve_workspace_root_expands_home() {
    let mut config = RustyConfig::default();
    config.workspace.root = Some("~/test_workspaces".to_string());

    let expanded = resolve_workspace_root(&config);

    assert_eq!(
        expanded,
        std::path::PathBuf::from(expand_home("~/test_workspaces"))
    );
    assert!(!expanded.to_string_lossy().contains('~'));
}

#[test]
fn resolve_workspace_root_uses_temp_when_not_set() {
    let mut config = RustyConfig::default();
    config.workspace.root = None;

    assert_eq!(
        resolve_workspace_root(&config),
        std::env::temp_dir().join("rusty_workspaces")
    );
}

#[test]
fn setup_checks_environment() {
    let _env_guard = EnvVarGuard::set(Some("test-token"), None);

    let report = check_prerequisites();

    assert_eq!(
        report.github_auth,
        GitHubAuthStatus::Environment {
            length: "test-token".len(),
        }
    );
    assert_eq!(report.workflow_file, WorkflowFileStatus::CurrentDir);
}

#[test]
fn config_loads_from_workflow_yaml() {
    let workflow = rusty::workflow::parse_workflow(
        "---\ntracker:\n  kind: github\n  repo: owner/repo\n---\nPrompt",
    )
    .unwrap();
    let config: RustyConfig =
        serde_yaml::from_value(serde_yaml::to_value(&workflow.config).unwrap()).unwrap();

    assert_eq!(config.tracker.kind, Some("github".to_string()));
}

fn environment_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct EnvVarGuard {
    _lock: MutexGuard<'static, ()>,
    previous_github_token: Option<String>,
    previous_gh_token: Option<String>,
}

impl EnvVarGuard {
    fn set(github_token: Option<&str>, gh_token: Option<&str>) -> Self {
        let lock = environment_lock().lock().unwrap();
        let previous_github_token = std::env::var("GITHUB_TOKEN").ok();
        let previous_gh_token = std::env::var("GH_TOKEN").ok();

        match github_token {
            Some(value) => std::env::set_var("GITHUB_TOKEN", value),
            None => std::env::remove_var("GITHUB_TOKEN"),
        }

        match gh_token {
            Some(value) => std::env::set_var("GH_TOKEN", value),
            None => std::env::remove_var("GH_TOKEN"),
        }

        Self {
            _lock: lock,
            previous_github_token,
            previous_gh_token,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous_github_token {
            Some(value) => std::env::set_var("GITHUB_TOKEN", value),
            None => std::env::remove_var("GITHUB_TOKEN"),
        }

        match &self.previous_gh_token {
            Some(value) => std::env::set_var("GH_TOKEN", value),
            None => std::env::remove_var("GH_TOKEN"),
        }
    }
}
