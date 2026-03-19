//! Trait-based abstractions ("ports") for external dependencies.
//!
//! Each trait has a production implementation that delegates to the real
//! subsystem (reqwest, tokio::process, std::fs) and can be swapped with a
//! mock in tests.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;

// ---------------------------------------------------------------------------
// HttpClient
// ---------------------------------------------------------------------------

/// A minimal HTTP response surface used by our domain code.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    body: HttpBody,
}

#[derive(Debug, Clone)]
enum HttpBody {
    Bytes(Vec<u8>),
}

impl HttpResponse {
    pub fn new(status: u16, headers: HashMap<String, String>, body: Vec<u8>) -> Self {
        Self {
            status,
            headers,
            body: HttpBody::Bytes(body),
        }
    }

    /// Deserialize the body as JSON.
    pub fn json<T: serde::de::DeserializeOwned>(&self) -> Result<T, serde_json::Error> {
        let HttpBody::Bytes(ref bytes) = self.body;
        serde_json::from_slice(bytes)
    }

    /// Return the body as a UTF-8 string (lossy).
    pub fn text(&self) -> String {
        let HttpBody::Bytes(ref bytes) = self.body;
        String::from_utf8_lossy(bytes).into_owned()
    }

    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers.get(&key.to_lowercase()).map(|v| v.as_str())
    }
}

/// Abstraction over HTTP GET/POST used by tracker and TUI code.
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, HttpClientError>;

    async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpClientError>;
}

#[derive(Debug, thiserror::Error)]
pub enum HttpClientError {
    #[error("HTTP request failed: {0}")]
    Request(String),
}

/// Production implementation backed by `reqwest::Client`.
#[derive(Debug, Clone)]
pub struct ReqwestHttpClient {
    inner: reqwest::Client,
}

impl ReqwestHttpClient {
    pub fn new() -> Self {
        Self {
            inner: reqwest::Client::new(),
        }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { inner: client }
    }
}

impl Default for ReqwestHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_headers(response: &reqwest::Response) -> HashMap<String, String> {
    response
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_lowercase(), v.to_string()))
        })
        .collect()
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, HttpClientError> {
        let mut builder = self.inner.get(url);
        for &(key, value) in headers {
            builder = builder.header(key, value);
        }
        let response = builder
            .send()
            .await
            .map_err(|e| HttpClientError::Request(e.to_string()))?;
        let status = response.status().as_u16();
        let hdrs = collect_headers(&response);
        let body = response
            .bytes()
            .await
            .map_err(|e| HttpClientError::Request(e.to_string()))?
            .to_vec();
        Ok(HttpResponse::new(status, hdrs, body))
    }

    async fn post(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Result<HttpResponse, HttpClientError> {
        let mut builder = self.inner.post(url);
        for &(key, value) in headers {
            builder = builder.header(key, value);
        }
        if let Some(body) = body {
            builder = builder.body(body.to_vec());
        }
        let response = builder
            .send()
            .await
            .map_err(|e| HttpClientError::Request(e.to_string()))?;
        let status = response.status().as_u16();
        let hdrs = collect_headers(&response);
        let bytes = response
            .bytes()
            .await
            .map_err(|e| HttpClientError::Request(e.to_string()))?
            .to_vec();
        Ok(HttpResponse::new(status, hdrs, bytes))
    }
}

// ---------------------------------------------------------------------------
// ProcessRunner
// ---------------------------------------------------------------------------

/// Output from a subprocess.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub status_success: bool,
    pub status_code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Abstraction over async subprocess execution.
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(&self, cmd: &str, args: &[&str]) -> Result<ProcessOutput, ProcessRunnerError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessRunnerError {
    #[error("process execution failed: {0}")]
    Execution(String),
}

/// Production implementation using `tokio::process::Command`.
#[derive(Debug, Default)]
pub struct TokioProcessRunner;

#[async_trait]
impl ProcessRunner for TokioProcessRunner {
    async fn run(&self, cmd: &str, args: &[&str]) -> Result<ProcessOutput, ProcessRunnerError> {
        let output = tokio::process::Command::new(cmd)
            .args(args)
            .output()
            .await
            .map_err(|e| ProcessRunnerError::Execution(e.to_string()))?;
        Ok(ProcessOutput {
            status_success: output.status.success(),
            status_code: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

// ---------------------------------------------------------------------------
// FileSystem
// ---------------------------------------------------------------------------

/// Abstraction over filesystem operations used by workspace and logging code.
pub trait FileSystem: Send + Sync {
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()>;
    fn read_to_string(&self, path: &Path) -> std::io::Result<String>;
    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()>;
}

/// Production implementation delegating to `std::fs`.
#[derive(Debug, Default, Clone)]
pub struct RealFileSystem;

impl FileSystem for RealFileSystem {
    fn create_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn remove_dir_all(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(path)
    }

    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn write(&self, path: &Path, contents: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, contents)
    }
}
