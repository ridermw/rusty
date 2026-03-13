use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tracing::{debug, warn};

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("agent not found: {0}")]
    NotFound(String),
    #[error("invalid workspace cwd: {0}")]
    InvalidWorkspaceCwd(PathBuf),
    #[error("response timeout")]
    ResponseTimeout,
    #[error("turn timeout")]
    TurnTimeout,
    #[error("turn failed: {0}")]
    TurnFailed(String),
    #[error("turn requires user input")]
    TurnInputRequired,
    #[error("agent process exited with code {0}")]
    ProcessExit(i32),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize, PartialEq)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// JSON-RPC 2.0 response or notification payload.
#[derive(Debug, Deserialize, PartialEq)]
pub struct JsonRpcMessage {
    pub jsonrpc: Option<String>,
    pub id: Option<Value>,
    pub method: Option<String>,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub params: Option<Value>,
}

impl JsonRpcMessage {
    /// Parse and sanitize a newline-delimited JSON-RPC message.
    pub fn parse_line(line: &str) -> Result<Option<Self>, AgentError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        Ok(Some(serde_json::from_str(trimmed)?))
    }
}

/// Guard that kills the child process on drop.
pub struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    /// Take ownership of the child (prevents kill on drop).
    pub fn take(&mut self) -> Option<Child> {
        self.child.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

/// Classified agent event from the ACP stream.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TurnCompleted,
    TurnFailed(String),
    TurnCancelled,
    UserInputRequired,
    ApprovalRequired(serde_json::Value),
    TokenUsage { input: u64, output: u64, total: u64 },
    Notification { message: String },
    Other(String),
}

/// Result of a completed turn.
#[derive(Debug, Clone)]
pub enum TurnResult {
    Completed { turn_id: String },
    Failed { turn_id: String, reason: String },
    Cancelled { turn_id: String },
}

/// ACP client manages a Copilot CLI subprocess.
pub struct AcpClient {
    guard: ChildGuard,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl AcpClient {
    /// Launch a Copilot CLI process directly (no shell wrapper).
    pub fn launch(command: &str, args: &[&str], cwd: &Path) -> Result<Self, AgentError> {
        if !cwd.is_dir() {
            return Err(AgentError::InvalidWorkspaceCwd(cwd.to_path_buf()));
        }

        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AgentError::NotFound(command.to_string())
            } else {
                AgentError::Io(e)
            }
        })?;

        let stdin = child.stdin.take().expect("stdin should be piped");
        let stdout = child.stdout.take().expect("stdout should be piped");

        Ok(Self {
            guard: ChildGuard::new(child),
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Send a JSON-RPC request (with ID) and return the ID used.
    pub async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<u64, AgentError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        debug!(method, id, "sent JSON-RPC request");
        Ok(id)
    }

    /// Send a JSON-RPC notification (no ID, no response expected).
    pub async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), AgentError> {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: None,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        debug!(method, "sent JSON-RPC notification");
        Ok(())
    }

    /// Read the next JSON-RPC message from stdout.
    /// Returns None only when the stream is closed (process exited).
    pub async fn read_message(&mut self) -> Result<Option<JsonRpcMessage>, AgentError> {
        loop {
            let mut line = String::new();
            let bytes_read = self.reader.read_line(&mut line).await?;
            if bytes_read == 0 {
                return Ok(None);
            }

            match JsonRpcMessage::parse_line(&line) {
                Ok(Some(msg)) => return Ok(Some(msg)),
                Ok(None) => continue,
                Err(AgentError::Json(_)) => {
                    warn!(line = line.trim(), "non-JSON line on stdout, ignoring");
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Read a response with a specific ID, with timeout.
    pub async fn read_response(
        &mut self,
        expected_id: u64,
        timeout_ms: u64,
    ) -> Result<JsonRpcMessage, AgentError> {
        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.read_message().await? {
                    Some(msg) => {
                        if let Some(id) = &msg.id {
                            if id.as_u64() == Some(expected_id) {
                                return Ok(msg);
                            }
                        }
                    }
                    None => return Err(AgentError::ProcessExit(-1)),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(AgentError::ResponseTimeout),
        }
    }

    /// Perform the ACP initialize handshake.
    /// Sends initialize request, waits for response, sends initialized notification.
    pub async fn handshake(&mut self, read_timeout_ms: u64) -> Result<JsonRpcMessage, AgentError> {
        let init_params = serde_json::json!({
            "protocolVersion": 1,
            "clientInfo": {
                "name": "rusty",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": {}
        });

        let id = self.send_request("initialize", Some(init_params)).await?;
        let response = self.read_response(id, read_timeout_ms).await?;

        if let Some(err) = &response.error {
            return Err(AgentError::TurnFailed(format!("initialize failed: {err}")));
        }

        if let Some(result) = &response.result {
            tracing::info!(capabilities = %result, "ACP server capabilities received");
        }

        self.send_notification("initialized", None).await?;

        Ok(response)
    }

    /// Create a new ACP session (equivalent to Codex thread/start).
    /// Returns the session/thread ID.
    pub async fn create_session(
        &mut self,
        cwd: &Path,
        approval_policy: &str,
        sandbox: Option<&str>,
        read_timeout_ms: u64,
    ) -> Result<String, AgentError> {
        let params = serde_json::json!({
            "cwd": cwd.to_string_lossy(),
            "mcpServers": [],
        });

        let id = self.send_request("session/new", Some(params)).await?;
        let response = self.read_response(id, read_timeout_ms).await?;

        if let Some(err) = &response.error {
            return Err(AgentError::TurnFailed(format!("session/new failed: {err}")));
        }

        // Extract session ID — check multiple response shapes
        let session_id = response
            .result
            .as_ref()
            .and_then(|r| {
                // Try: result.sessionId (string)
                r.get("sessionId")
                    .and_then(Value::as_str)
                    // Try: result.session.id
                    .or_else(|| {
                        r.get("session")
                            .and_then(|s| s.get("id"))
                            .and_then(Value::as_str)
                    })
                    // Try: result.id
                    .or_else(|| r.get("id").and_then(Value::as_str))
            })
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                let result_str = response
                    .result
                    .as_ref()
                    .map(|r| r.to_string())
                    .unwrap_or_default();
                AgentError::TurnFailed(format!(
                    "no session ID in session.create response: {result_str}"
                ))
            })?;

        tracing::info!(%session_id, "ACP session created");
        Ok(session_id)
    }

    /// Send a message/turn to the session and stream responses.
    /// Returns when the turn completes, fails, or times out.
    /// Calls `on_event` for each streamed event.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_turn(
        &mut self,
        session_id: &str,
        prompt: &str,
        title: &str,
        cwd: &Path,
        approval_policy: &str,
        sandbox_policy: Option<&serde_json::Value>,
        turn_timeout_ms: u64,
        mut on_event: impl FnMut(AgentEvent),
    ) -> Result<TurnResult, AgentError> {
        // ACP session/prompt params: sessionId + prompt array
        let params = serde_json::json!({
            "sessionId": session_id,
            "prompt": [{"type": "text", "text": prompt}],
        });

        let prompt_id = self.send_request("session/prompt", Some(params)).await?;

        // session/prompt streams session/update notifications before returning
        // a final response with stopReason. Use the full turn timeout.
        let turn_id = "turn-1".to_string();

        debug!(%turn_id, "prompt sent, streaming events");

        let timeout = tokio::time::Duration::from_millis(turn_timeout_ms);
        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.read_message().await? {
                    Some(msg) => {
                        // Check if this is the final response to our prompt request
                        if let Some(id) = &msg.id {
                            if id.as_u64() == Some(prompt_id) {
                                // This is the prompt result
                                if let Some(err) = &msg.error {
                                    return Ok(TurnResult::Failed {
                                        turn_id: turn_id.clone(),
                                        reason: err.to_string(),
                                    });
                                }
                                let stop_reason = msg
                                    .result
                                    .as_ref()
                                    .and_then(|r| r.get("stopReason"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("end_turn");
                                return match stop_reason {
                                    "end_turn" => Ok(TurnResult::Completed {
                                        turn_id: turn_id.clone(),
                                    }),
                                    "cancelled" => Ok(TurnResult::Cancelled {
                                        turn_id: turn_id.clone(),
                                    }),
                                    _ => Ok(TurnResult::Completed {
                                        turn_id: turn_id.clone(),
                                    }),
                                };
                            }
                        }

                        // Otherwise it's a streaming notification
                        let event = classify_event(&msg);
                        on_event(event.clone());

                        match &event {
                            AgentEvent::TurnCompleted => {
                                return Ok(TurnResult::Completed {
                                    turn_id: turn_id.clone(),
                                });
                            }
                            AgentEvent::TurnFailed(reason) => {
                                return Ok(TurnResult::Failed {
                                    turn_id: turn_id.clone(),
                                    reason: reason.clone(),
                                });
                            }
                            AgentEvent::TurnCancelled => {
                                return Ok(TurnResult::Cancelled {
                                    turn_id: turn_id.clone(),
                                });
                            }
                            AgentEvent::UserInputRequired => {
                                return Err(AgentError::TurnInputRequired);
                            }
                            AgentEvent::ApprovalRequired(payload) => {
                                // Auto-approve permission requests
                                if let Some(req_id) = payload.get("id").and_then(Value::as_str) {
                                    let _ = self
                                        .send_request(
                                            "session/request_permission",
                                            Some(serde_json::json!({
                                                "id": req_id,
                                                "outcome": {"outcome": "approved"}
                                            })),
                                        )
                                        .await;
                                }
                            }
                            _ => {} // Notifications — continue streaming
                        }
                    }
                    None => return Err(AgentError::ProcessExit(-1)),
                }
            }
        })
        .await;

        match result {
            Ok(inner) => inner,
            Err(_) => Err(AgentError::TurnTimeout),
        }
    }

    /// Gracefully stop the subprocess.
    pub async fn stop(mut self) -> Result<(), AgentError> {
        drop(self.stdin);

        if let Some(mut child) = self.guard.take() {
            let wait = child.wait();
            if tokio::time::timeout(tokio::time::Duration::from_secs(5), wait)
                .await
                .is_err()
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
        }

        Ok(())
    }
}

/// Classify a raw JSON-RPC message into a typed event.
pub fn classify_event(msg: &JsonRpcMessage) -> AgentEvent {
    let method = msg.method.as_deref().unwrap_or("");

    match method {
        "turn/completed" => AgentEvent::TurnCompleted,
        "turn/failed" => {
            let reason = msg
                .params
                .as_ref()
                .and_then(|p| p.get("error").or_else(|| p.get("message")))
                .map(|value| {
                    value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| value.to_string())
                })
                .unwrap_or_else(|| "unknown".to_string());
            AgentEvent::TurnFailed(reason)
        }
        "turn/cancelled" | "session/cancel" => AgentEvent::TurnCancelled,
        "session/request_permission" => {
            // Could be approval or user input — check payload
            let params = msg.params.as_ref();
            let is_user_input = params
                .and_then(|p| p.get("type"))
                .and_then(Value::as_str)
                .map_or(false, |t| t == "userInput");
            if is_user_input {
                AgentEvent::UserInputRequired
            } else {
                AgentEvent::ApprovalRequired(msg.params.clone().unwrap_or_default())
            }
        }
        "item/tool/requestUserInput" => AgentEvent::UserInputRequired,
        "item/tool/approvalRequired" => {
            AgentEvent::ApprovalRequired(msg.params.clone().unwrap_or_default())
        }
        "thread/tokenUsage/updated" => {
            let (input, output, total) = extract_token_usage(msg);
            AgentEvent::TokenUsage {
                input,
                output,
                total,
            }
        }
        "session/update" => {
            // General update — inspect payload to determine type
            let params = msg.params.as_ref();

            // Check for token usage
            if params.map_or(false, |p| {
                p.get("tokenUsage").is_some() || p.get("usage").is_some()
            }) {
                let (input, output, total) = extract_token_usage(msg);
                return AgentEvent::TokenUsage {
                    input,
                    output,
                    total,
                };
            }

            // Check for completion/status
            if let Some(status) = params.and_then(|p| p.get("status")).and_then(Value::as_str) {
                return match status {
                    "completed" | "done" => AgentEvent::TurnCompleted,
                    "failed" | "error" => {
                        let reason = params
                            .and_then(|p| p.get("error"))
                            .map(|e| e.to_string())
                            .unwrap_or_else(|| "unknown".to_string());
                        AgentEvent::TurnFailed(reason)
                    }
                    "cancelled" => AgentEvent::TurnCancelled,
                    _ => AgentEvent::Notification {
                        message: format!("status: {status}"),
                    },
                };
            }

            // Generic notification
            let message = params
                .and_then(|p| p.get("message").or_else(|| p.get("text")))
                .and_then(Value::as_str)
                .unwrap_or("session update")
                .to_string();
            AgentEvent::Notification { message }
        }
        _ => {
            let message = msg
                .params
                .as_ref()
                .and_then(|p| p.get("message").or_else(|| p.get("text")))
                .and_then(Value::as_str)
                .unwrap_or(method)
                .to_string();
            if method.is_empty() && msg.result.is_some() {
                AgentEvent::Other("response".to_string())
            } else {
                AgentEvent::Notification { message }
            }
        }
    }
}

pub fn extract_token_usage(msg: &JsonRpcMessage) -> (u64, u64, u64) {
    let params = msg.params.as_ref();

    // Try multiple payload shapes:
    // 1. Top-level snake_case: params.input_tokens
    // 2. Nested snake_case: params.usage.input_tokens
    // 3. Top-level camelCase: params.inputTokens
    // 4. Nested camelCase: params.tokenUsage.total.inputTokens
    let get = |snake: &str, camel: &str| -> u64 {
        params
            .and_then(|p| {
                p.get(snake)
                    .or_else(|| p.get("usage").and_then(|u| u.get(snake)))
                    .or_else(|| p.get(camel))
                    .or_else(|| {
                        p.get("tokenUsage")
                            .and_then(|tu| tu.get("total").and_then(|t| t.get(camel)))
                    })
                    .or_else(|| {
                        p.get("total_token_usage")
                            .and_then(|tu| tu.get(snake).or_else(|| tu.get(camel)))
                    })
            })
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    (
        get("input_tokens", "inputTokens"),
        get("output_tokens", "outputTokens"),
        get("total_tokens", "totalTokens"),
    )
}
