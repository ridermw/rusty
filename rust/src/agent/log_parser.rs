//! Parse Copilot CLI log files for token usage metrics.
//!
//! Copilot CLI 1.0.5 does not expose token usage over the ACP protocol.
//! When `agent.log_dir` is configured, Rusty passes `--log-dir <path>` to
//! the Copilot CLI process and parses the resulting log files for
//! `prompt_tokens_count`, `completion_tokens_count`, and `total_tokens_count`
//! entries.

use std::path::Path;

use tokio::io::AsyncBufReadExt;
use tracing::{debug, warn};

/// Token counts extracted from a single log event.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogTokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

/// Parse a single line of Copilot log output for a token-count field.
///
/// Recognized patterns (colon-separated key-value):
/// - `prompt_tokens_count: 45760`
/// - `completion_tokens_count: 658`
/// - `total_tokens_count: 46418`
///
/// Also handles JSON-embedded variants where the key-value appears inside
/// a larger JSON object string, e.g. `"prompt_tokens_count":45760`.
///
/// Returns `Some((field, value))` if a recognized token field is found.
pub fn parse_token_line(line: &str) -> Option<(&'static str, u64)> {
    const FIELDS: &[(&str, &str)] = &[
        ("prompt_tokens_count", "prompt"),
        ("completion_tokens_count", "completion"),
        ("total_tokens_count", "total"),
    ];

    for &(key, label) in FIELDS {
        if let Some(value) = extract_value(line, key) {
            return Some((label, value));
        }
    }
    None
}

/// Extract a u64 value for a given key from a log line.
/// Handles both `key: value` (plain) and `"key":value` / `"key": value` (JSON) formats.
fn extract_value(line: &str, key: &str) -> Option<u64> {
    // Try plain format: `key: 12345` or `key:12345`
    if let Some(pos) = line.find(key) {
        let after_key = &line[pos + key.len()..];
        // Skip optional `"`, then expect `:`
        let rest = after_key.trim_start_matches('"');
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            // Parse digits, stopping at non-digit
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !num_str.is_empty() {
                return num_str.parse::<u64>().ok();
            }
        }
    }
    None
}

/// Scan all lines of a log file and accumulate the highest token counts seen.
///
/// Copilot logs may contain multiple usage snapshots; we take the maximum
/// of each field since token counts are cumulative within a session.
pub async fn scan_log_file(path: &Path) -> std::io::Result<LogTokenUsage> {
    let file = tokio::fs::File::open(path).await?;
    let reader = tokio::io::BufReader::new(file);
    let mut lines = reader.lines();
    let mut usage = LogTokenUsage::default();

    while let Some(line) = lines.next_line().await? {
        if let Some((field, value)) = parse_token_line(&line) {
            match field {
                "prompt" => {
                    if value > usage.prompt_tokens {
                        usage.prompt_tokens = value;
                    }
                }
                "completion" => {
                    if value > usage.completion_tokens {
                        usage.completion_tokens = value;
                    }
                }
                "total" => {
                    if value > usage.total_tokens {
                        usage.total_tokens = value;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(usage)
}

/// Scan all `process-*.log` files in a directory and return the aggregate
/// maximum token usage across all files.
pub async fn scan_log_dir(dir: &Path) -> LogTokenUsage {
    let mut aggregate = LogTokenUsage::default();

    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "failed to read log directory");
            return aggregate;
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();

        // Copilot writes logs to files like `process-*.log` or other .log files
        if !name.ends_with(".log") {
            continue;
        }

        match scan_log_file(&path).await {
            Ok(usage) => {
                debug!(
                    file = %name,
                    prompt = usage.prompt_tokens,
                    completion = usage.completion_tokens,
                    total = usage.total_tokens,
                    "parsed log file"
                );
                aggregate.prompt_tokens = aggregate.prompt_tokens.max(usage.prompt_tokens);
                aggregate.completion_tokens =
                    aggregate.completion_tokens.max(usage.completion_tokens);
                aggregate.total_tokens = aggregate.total_tokens.max(usage.total_tokens);
            }
            Err(e) => {
                warn!(file = %name, error = %e, "failed to parse log file");
            }
        }
    }

    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_token_line ──────────────────────────────────────────

    #[test]
    fn parse_plain_prompt_tokens() {
        let line = "prompt_tokens_count: 45760";
        assert_eq!(parse_token_line(line), Some(("prompt", 45760)));
    }

    #[test]
    fn parse_plain_completion_tokens() {
        let line = "completion_tokens_count: 658";
        assert_eq!(parse_token_line(line), Some(("completion", 658)));
    }

    #[test]
    fn parse_plain_total_tokens() {
        let line = "total_tokens_count: 46418";
        assert_eq!(parse_token_line(line), Some(("total", 46418)));
    }

    #[test]
    fn parse_json_embedded_tokens() {
        let line = r#"{"level":"info","prompt_tokens_count":12345,"ts":"2026-01-01"}"#;
        assert_eq!(parse_token_line(line), Some(("prompt", 12345)));
    }

    #[test]
    fn parse_json_with_spaces() {
        let line = r#"  "total_tokens_count": 99999,"#;
        assert_eq!(parse_token_line(line), Some(("total", 99999)));
    }

    #[test]
    fn parse_no_match_returns_none() {
        assert_eq!(parse_token_line("some other log line"), None);
        assert_eq!(parse_token_line("tokens_count: 100"), None);
        assert_eq!(parse_token_line(""), None);
    }

    #[test]
    fn parse_zero_value() {
        assert_eq!(
            parse_token_line("prompt_tokens_count: 0"),
            Some(("prompt", 0))
        );
    }

    #[test]
    fn parse_large_value() {
        assert_eq!(
            parse_token_line("total_tokens_count: 999999999"),
            Some(("total", 999999999))
        );
    }

    #[test]
    fn parse_value_with_trailing_text() {
        let line = "prompt_tokens_count: 500 (some note)";
        assert_eq!(parse_token_line(line), Some(("prompt", 500)));
    }

    #[test]
    fn parse_line_with_prefix_context() {
        let line = "2026-03-14T09:00:00Z INFO assistant_usage prompt_tokens_count: 45760";
        assert_eq!(parse_token_line(line), Some(("prompt", 45760)));
    }

    // ── scan_log_file ─────────────────────────────────────────────

    #[tokio::test]
    async fn scan_log_file_extracts_max_values() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("process-1.log");
        tokio::fs::write(
            &log_path,
            "\
2026-03-14 INFO starting session
prompt_tokens_count: 1000
completion_tokens_count: 100
total_tokens_count: 1100
prompt_tokens_count: 2000
completion_tokens_count: 200
total_tokens_count: 2200
",
        )
        .await
        .unwrap();

        let usage = scan_log_file(&log_path).await.unwrap();
        assert_eq!(usage.prompt_tokens, 2000);
        assert_eq!(usage.completion_tokens, 200);
        assert_eq!(usage.total_tokens, 2200);
    }

    #[tokio::test]
    async fn scan_log_file_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("empty.log");
        tokio::fs::write(&log_path, "").await.unwrap();

        let usage = scan_log_file(&log_path).await.unwrap();
        assert_eq!(usage, LogTokenUsage::default());
    }

    // ── scan_log_dir ──────────────────────────────────────────────

    #[tokio::test]
    async fn scan_log_dir_aggregates_across_files() {
        let dir = tempfile::tempdir().unwrap();

        tokio::fs::write(
            dir.path().join("process-1.log"),
            "prompt_tokens_count: 1000\ncompletion_tokens_count: 100\ntotal_tokens_count: 1100\n",
        )
        .await
        .unwrap();

        tokio::fs::write(
            dir.path().join("process-2.log"),
            "prompt_tokens_count: 3000\ncompletion_tokens_count: 50\ntotal_tokens_count: 3050\n",
        )
        .await
        .unwrap();

        // Non-log file should be ignored
        tokio::fs::write(dir.path().join("other.txt"), "prompt_tokens_count: 99999\n")
            .await
            .unwrap();

        let usage = scan_log_dir(dir.path()).await;
        assert_eq!(usage.prompt_tokens, 3000);
        assert_eq!(usage.completion_tokens, 100);
        assert_eq!(usage.total_tokens, 3050);
    }

    #[tokio::test]
    async fn scan_log_dir_missing_directory() {
        let usage = scan_log_dir(Path::new("/nonexistent/path/xyz")).await;
        assert_eq!(usage, LogTokenUsage::default());
    }
}
