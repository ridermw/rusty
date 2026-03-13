use clap::Parser;
use rusty::cli::{Cli, Commands, RunArgs};

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
