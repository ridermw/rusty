use clap::Parser;
use symphony::cli::Args;

#[test]
fn cli_default_workflow_path() {
    let args = Args::parse_from([
        "symphony",
        "--i-understand-that-this-will-be-running-without-the-usual-guardrails",
    ]);
    assert_eq!(args.workflow_path.to_str().unwrap(), "WORKFLOW.md");
    assert!(args.guardrails_acknowledged);
    assert_eq!(args.port, None);
    assert_eq!(args.logs_root, None);
}

#[test]
fn cli_custom_workflow_path() {
    let args = Args::parse_from([
        "symphony",
        "custom/WORKFLOW.md",
        "--i-understand-that-this-will-be-running-without-the-usual-guardrails",
    ]);
    assert_eq!(args.workflow_path.to_str().unwrap(), "custom/WORKFLOW.md");
}

#[test]
fn cli_with_port_and_logs() {
    let args = Args::parse_from([
        "symphony",
        "--port",
        "4000",
        "--logs-root",
        "./logs",
        "--i-understand-that-this-will-be-running-without-the-usual-guardrails",
    ]);
    assert_eq!(args.port, Some(4000));
    assert_eq!(args.logs_root.unwrap().to_str().unwrap(), "./logs");
}

#[test]
fn cli_guardrails_flag_defaults_false() {
    let args = Args::parse_from(["symphony"]);
    assert!(!args.guardrails_acknowledged);
}
