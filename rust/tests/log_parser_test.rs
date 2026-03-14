use std::path::PathBuf;

use rusty::agent::log_parser::{self, LogTokenUsage};

// ── parse_token_line edge cases ──────────────────────────────────

#[test]
fn parse_all_three_fields_from_realistic_log_block() {
    let lines = [
        "2026-03-14T09:32:58Z INFO  assistant_usage prompt_tokens_count: 45760",
        "2026-03-14T09:32:58Z INFO  assistant_usage completion_tokens_count: 658",
        "2026-03-14T09:32:58Z INFO  assistant_usage total_tokens_count: 46418",
    ];

    let mut usage = LogTokenUsage::default();
    for line in &lines {
        if let Some((field, value)) = log_parser::parse_token_line(line) {
            match field {
                "prompt" => usage.prompt_tokens = usage.prompt_tokens.max(value),
                "completion" => usage.completion_tokens = usage.completion_tokens.max(value),
                "total" => usage.total_tokens = usage.total_tokens.max(value),
                _ => {}
            }
        }
    }

    assert_eq!(usage.prompt_tokens, 45760);
    assert_eq!(usage.completion_tokens, 658);
    assert_eq!(usage.total_tokens, 46418);
}

#[test]
fn parse_json_rpc_style_log_lines() {
    let line = r#"{"jsonrpc":"2.0","method":"assistant_usage","params":{"prompt_tokens_count":12000,"completion_tokens_count":500,"total_tokens_count":12500}}"#;

    // Should find the first matching field
    let result = log_parser::parse_token_line(line);
    assert!(result.is_some());
    let (field, value) = result.unwrap();
    assert_eq!(field, "prompt");
    assert_eq!(value, 12000);
}

// ── scan_log_file integration ─────────────────────────────────────

#[tokio::test]
async fn scan_realistic_copilot_log() {
    let dir = tempfile::tempdir().unwrap();
    let log_content = r#"
2026-03-14T09:30:00Z INFO  Starting copilot session
2026-03-14T09:30:01Z DEBUG Initialized ACP protocol
2026-03-14T09:30:05Z INFO  assistant_usage prompt_tokens_count: 10000
2026-03-14T09:30:05Z INFO  assistant_usage completion_tokens_count: 200
2026-03-14T09:30:05Z INFO  assistant_usage total_tokens_count: 10200
2026-03-14T09:31:00Z INFO  Turn 1 completed
2026-03-14T09:31:05Z INFO  assistant_usage prompt_tokens_count: 25000
2026-03-14T09:31:05Z INFO  assistant_usage completion_tokens_count: 450
2026-03-14T09:31:05Z INFO  assistant_usage total_tokens_count: 25450
2026-03-14T09:32:00Z INFO  Turn 2 completed
2026-03-14T09:32:05Z INFO  assistant_usage prompt_tokens_count: 45760
2026-03-14T09:32:05Z INFO  assistant_usage completion_tokens_count: 658
2026-03-14T09:32:05Z INFO  assistant_usage total_tokens_count: 46418
2026-03-14T09:32:30Z INFO  Session completed
"#;

    let log_path = dir.path().join("process-12345.log");
    tokio::fs::write(&log_path, log_content).await.unwrap();

    let usage = log_parser::scan_log_file(&log_path).await.unwrap();
    assert_eq!(usage.prompt_tokens, 45760);
    assert_eq!(usage.completion_tokens, 658);
    assert_eq!(usage.total_tokens, 46418);
}

// ── scan_log_dir integration ──────────────────────────────────────

#[tokio::test]
async fn scan_dir_with_multiple_session_logs() {
    let dir = tempfile::tempdir().unwrap();

    // Session 1 log
    tokio::fs::write(
        dir.path().join("process-100.log"),
        "prompt_tokens_count: 10000\ncompletion_tokens_count: 500\ntotal_tokens_count: 10500\n",
    )
    .await
    .unwrap();

    // Session 2 log with higher values
    tokio::fs::write(
        dir.path().join("process-200.log"),
        "prompt_tokens_count: 50000\ncompletion_tokens_count: 1000\ntotal_tokens_count: 51000\n",
    )
    .await
    .unwrap();

    let usage = log_parser::scan_log_dir(dir.path()).await;
    // scan_log_dir returns the max across files
    assert_eq!(usage.prompt_tokens, 50000);
    assert_eq!(usage.completion_tokens, 1000);
    assert_eq!(usage.total_tokens, 51000);
}

#[tokio::test]
async fn scan_dir_ignores_non_log_files() {
    let dir = tempfile::tempdir().unwrap();

    tokio::fs::write(
        dir.path().join("config.yaml"),
        "prompt_tokens_count: 99999\n",
    )
    .await
    .unwrap();

    tokio::fs::write(
        dir.path().join("process-1.log"),
        "prompt_tokens_count: 100\ncompletion_tokens_count: 10\ntotal_tokens_count: 110\n",
    )
    .await
    .unwrap();

    let usage = log_parser::scan_log_dir(dir.path()).await;
    assert_eq!(usage.prompt_tokens, 100);
    assert_eq!(usage.total_tokens, 110);
}

// ── config integration ────────────────────────────────────────────

#[test]
fn agent_config_log_dir_defaults_to_none() {
    let config: rusty::config::schema::AgentConfig = Default::default();
    assert!(config.log_dir.is_none());
}

#[test]
fn agent_config_log_dir_deserializes_from_yaml() {
    let yaml = r#"
command: "copilot --acp"
log_dir: "/tmp/copilot-logs"
"#;
    let config: rusty::config::schema::AgentConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.log_dir.as_deref(), Some("/tmp/copilot-logs"));
}

#[test]
fn agent_config_without_log_dir_deserializes() {
    let yaml = r#"
command: "copilot --acp"
"#;
    let config: rusty::config::schema::AgentConfig = serde_yaml::from_str(yaml).unwrap();
    assert!(config.log_dir.is_none());
}

// ── command injection verification ────────────────────────────────

#[test]
fn log_dir_flag_injection_logic() {
    // Simulate the flag injection logic from agent/mod.rs
    let base_args = vec!["--acp", "--yolo", "--no-ask-user"];
    let log_dir = Some(PathBuf::from("/tmp/logs/issue-42-20260314"));

    let mut owned_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    if let Some(ref dir) = log_dir {
        owned_args.push("--log-dir".to_string());
        owned_args.push(dir.to_string_lossy().to_string());
    }

    assert_eq!(owned_args.len(), 5);
    assert_eq!(owned_args[3], "--log-dir");
    assert_eq!(owned_args[4], "/tmp/logs/issue-42-20260314");
}

#[test]
fn no_log_dir_flag_when_none() {
    let base_args = vec!["--acp", "--yolo", "--no-ask-user"];
    let log_dir: Option<PathBuf> = None;

    let mut owned_args: Vec<String> = base_args.iter().map(|s| s.to_string()).collect();
    if let Some(ref dir) = log_dir {
        owned_args.push("--log-dir".to_string());
        owned_args.push(dir.to_string_lossy().to_string());
    }

    assert_eq!(owned_args.len(), 3);
    assert!(!owned_args.contains(&"--log-dir".to_string()));
}
