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
        let mut params = serde_json::json!({
            "approvalPolicy": approval_policy,
            "cwd": cwd.to_string_lossy(),
        });
        if let Some(sb) = sandbox {
            params["sandbox"] = serde_json::json!(sb);
        }

        let id = self.send_request("session/create", Some(params)).await?;
        let response = self.read_response(id, read_timeout_ms).await?;

        if let Some(err) = &response.error {
            return Err(AgentError::TurnFailed(format!(
                "session/create failed: {err}"
            )));
        }

        let session_id = response
            .result
            .as_ref()
            .and_then(|r| r.get("session").or_else(|| r.get("thread")))
            .and_then(|s| s.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AgentError::TurnFailed("no session ID in response".to_string()))?;

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
        let mut params = serde_json::json!({
            "threadId": session_id,
            "input": [{"type": "text", "text": prompt}],
            "cwd": cwd.to_string_lossy(),
            "title": title,
            "approvalPolicy": approval_policy,
        });
        if let Some(sp) = sandbox_policy {
            params["sandboxPolicy"] = sp.clone();
        }

        let id = self
            .send_request("session/message/send", Some(params))
            .await?;

        let ack = self.read_response(id, 10_000).await?;
        if let Some(err) = &ack.error {
            return Err(AgentError::TurnFailed(format!("turn start failed: {err}")));
        }

        let turn_id = ack
            .result
            .as_ref()
            .and_then(|r| r.get("turn").or_else(|| r.get("id")))
            .and_then(|t| {
                if let Some(id) = t.as_str() {
                    Some(id.to_string())
                } else {
                    t.get("id").and_then(Value::as_str).map(ToOwned::to_owned)
                }
            })
            .unwrap_or_else(|| "unknown".to_string());

        debug!(%turn_id, "turn started, streaming events");

        let timeout = tokio::time::Duration::from_millis(turn_timeout_ms);
        let result = tokio::time::timeout(timeout, async {
            loop {
                match self.read_message().await? {
                    Some(msg) => {
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
                                if let Some(id) = payload.get("id").and_then(Value::as_str) {
                                    let _ = self
                                        .send_request(
                                            "approval/respond",
                                            Some(serde_json::json!({"id": id, "approved": true})),
                                        )
                                        .await;
                                }
                            }
                            _ => {}
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
        "turn/completed" | "session/message/completed" => AgentEvent::TurnCompleted,
        "turn/failed" | "session/message/failed" => {
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
        "turn/cancelled" | "session/message/cancelled" => AgentEvent::TurnCancelled,
        "item/tool/requestUserInput" | "session/userInputRequired" => AgentEvent::UserInputRequired,
        "item/tool/approvalRequired" | "session/approvalRequired" => {
            AgentEvent::ApprovalRequired(msg.params.clone().unwrap_or_default())
        }
        "thread/tokenUsage/updated" | "session/tokenUsage" => {
            let (input, output, total) = extract_token_usage(msg);
            AgentEvent::TokenUsage {
                input,
                output,
                total,
            }
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
    let get = |key: &str| -> u64 {
        params
            .and_then(|p| {
                p.get(key)
                    .or_else(|| p.get("usage").and_then(|u| u.get(key)))
            })
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    (
        get("input_tokens"),
        get("output_tokens"),
        get("total_tokens"),
    )
}
