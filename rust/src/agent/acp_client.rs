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
