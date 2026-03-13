use std::fs;
use std::path::Path;

use rusty::config::ConfigError;
use rusty::workflow::store::WorkflowStore;
use rusty::workflow::{load_workflow, parse_workflow};
use serde_yaml::{Mapping, Value};
use tempfile::tempdir;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout, Duration};

fn empty_mapping() -> Value {
    Value::Mapping(Mapping::new())
}

fn write_workflow(path: &Path, content: &str) {
    fs::write(path, content).expect("workflow file should be written");
}

#[test]
fn parse_workflow_splits_front_matter_and_prompt() {
    let definition = parse_workflow(
        "---\ntracker:\n  kind: github\npolling:\n  interval_ms: 1000\n---\n# Prompt\n\nDo the thing.\n",
    )
    .expect("workflow should parse");

    let expected_config: Value =
        serde_yaml::from_str("tracker:\n  kind: github\npolling:\n  interval_ms: 1000\n")
            .expect("yaml should parse");

    assert_eq!(definition.config, expected_config);
    assert_eq!(definition.prompt_template, "# Prompt\n\nDo the thing.");
}

#[test]
fn parse_workflow_without_front_matter_uses_entire_prompt() {
    let definition = parse_workflow("\n  # Prompt\n\nHello from the body.\n")
        .expect("workflow without front matter should parse");

    assert_eq!(definition.config, empty_mapping());
    assert_eq!(
        definition.prompt_template,
        "# Prompt\n\nHello from the body."
    );
}

#[test]
fn parse_workflow_supports_empty_prompt_body() {
    let definition =
        parse_workflow("---\ntracker:\n  kind: github\n---\n").expect("workflow should parse");

    let expected_config: Value =
        serde_yaml::from_str("tracker:\n  kind: github\n").expect("yaml should parse");

    assert_eq!(definition.config, expected_config);
    assert!(definition.prompt_template.is_empty());
}

#[test]
fn parse_workflow_rejects_non_map_front_matter() {
    let error = parse_workflow("---\n- github\n- linear\n---\nprompt")
        .expect_err("non-map front matter should fail");

    assert!(matches!(error, ConfigError::WorkflowFrontMatterNotAMap));
}

#[test]
fn load_workflow_returns_missing_file_error_for_absent_path() {
    let temp_dir = tempdir().expect("temp dir should be created");
    let missing_path = temp_dir.path().join("WORKFLOW.md");

    let error = load_workflow(&missing_path).expect_err("missing file should fail");

    match error {
        ConfigError::MissingWorkflowFile(path) => assert_eq!(path, missing_path),
        other => panic!("expected MissingWorkflowFile, got {other:?}"),
    }
}

#[test]
fn parse_workflow_returns_parse_error_for_invalid_yaml() {
    let error =
        parse_workflow("---\ntracker: [github\n---\nprompt").expect_err("invalid yaml should fail");

    assert!(matches!(error, ConfigError::WorkflowParseError(_)));
}

#[test]
fn parse_workflow_preserves_unknown_top_level_keys() {
    let definition = parse_workflow(
        "---\ntracker:\n  kind: github\nfuture_flag: true\nnew_section:\n  answer: 42\n---\nprompt\n",
    )
    .expect("workflow should parse");

    let expected_config: Value = serde_yaml::from_str(
        "tracker:\n  kind: github\nfuture_flag: true\nnew_section:\n  answer: 42\n",
    )
    .expect("yaml should parse");

    assert_eq!(definition.config, expected_config);
}

#[tokio::test]
async fn workflow_store_reloads_valid_changes() {
    let temp_dir = tempdir().expect("temp dir should be created");
    let workflow_path = temp_dir.path().join("WORKFLOW.md");
    write_workflow(
        &workflow_path,
        "---\ntracker:\n  kind: github\n---\ninitial prompt\n",
    );

    let (reload_tx, mut reload_rx) = mpsc::channel(4);
    let store = WorkflowStore::new(&workflow_path, reload_tx).expect("store should initialize");

    sleep(Duration::from_millis(250)).await;
    write_workflow(
        &workflow_path,
        "---\ntracker:\n  kind: github\ncustom: true\n---\nupdated prompt\n",
    );

    let reloaded = timeout(Duration::from_secs(5), reload_rx.recv())
        .await
        .expect("reload notification should arrive in time")
        .expect("reload channel should remain open");

    let expected_config: Value = serde_yaml::from_str("tracker:\n  kind: github\ncustom: true\n")
        .expect("yaml should parse");

    assert_eq!(reloaded.config, expected_config);
    assert_eq!(reloaded.prompt_template, "updated prompt");
    assert_eq!(store.current().prompt_template, "updated prompt");
}
