//! Terminal status dashboard renderer.

use std::borrow::Cow;

use crate::orchestrator::{OrchestratorSnapshot, RetrySnapshot, RunningSnapshot};

/// Render an orchestrator snapshot as a human-readable terminal string.
pub fn render_dashboard(snapshot: &OrchestratorSnapshot) -> String {
    let mut out = String::new();

    out.push_str("═══ Rusty Status ═══\n\n");
    out.push_str(&format!(
        "Running: {}  |  Retrying: {}  |  Tokens: {} (in:{} out:{})\n",
        snapshot.running_count,
        snapshot.retrying_count,
        snapshot.agent_totals.total_tokens,
        snapshot.agent_totals.input_tokens,
        snapshot.agent_totals.output_tokens,
    ));
    out.push_str(&format!(
        "Runtime: {:.1}s\n\n",
        snapshot.agent_totals.seconds_running,
    ));

    if snapshot.running.is_empty() {
        out.push_str("No running sessions.\n");
    } else {
        out.push_str("── Running ──\n");
        for running in &snapshot.running {
            out.push_str(&format_running_entry(running));
        }
    }
    out.push('\n');

    if !snapshot.retrying.is_empty() {
        out.push_str("── Retry Queue ──\n");
        for retry in &snapshot.retrying {
            out.push_str(&format_retry_entry(retry));
        }
        out.push('\n');
    }

    out
}

fn format_running_entry(running: &RunningSnapshot) -> String {
    let pid = running
        .pid
        .map(|p| p.to_string())
        .unwrap_or_else(|| "-".to_string());
    let event = running
        .last_message
        .as_deref()
        .filter(|m| !m.trim().is_empty())
        .or(running.last_event.as_deref())
        .unwrap_or("-");
    let message = truncate(event, 60);
    let session = running
        .session_id
        .as_deref()
        .map(|sid| {
            if sid.len() > 10 {
                format!("{}...{}", &sid[..4], &sid[sid.len() - 6..])
            } else {
                sid.to_string()
            }
        })
        .unwrap_or_else(|| "-".to_string());

    format!(
        "  {} [{}] pid:{} turns:{} tokens:{} session:{} | {}\n",
        running.identifier,
        running.state,
        pid,
        running.turn_count,
        running.total_tokens,
        session,
        message,
    )
}

fn format_retry_entry(retry: &RetrySnapshot) -> String {
    let error = truncate(retry.error.as_deref().unwrap_or("-"), 50);

    format!(
        "  {} attempt:{} due:{} | {}\n",
        retry.identifier, retry.attempt, retry.due_at, error,
    )
}

fn truncate(value: &str, max_chars: usize) -> Cow<'_, str> {
    if value.chars().count() <= max_chars {
        return Cow::Borrowed(value);
    }

    Cow::Owned(value.chars().take(max_chars).collect())
}

/// Humanize a raw agent event name for display.
pub fn humanize_event(event: &str) -> &str {
    match event {
        "session_started" => "Started",
        "turn_completed" => "Turn done",
        "turn_failed" => "Turn FAILED",
        "turn_cancelled" => "Cancelled",
        "notification" => "Working",
        "approval_auto_approved" => "Auto-approved",
        _ => event,
    }
}
