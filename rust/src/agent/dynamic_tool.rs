//! Dynamic tool handlers for agent sessions.
//! Currently implements: github_graphql

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{debug, warn};

/// Input for the github_graphql tool.
#[derive(Debug, Deserialize)]
pub struct GraphqlToolInput {
    pub query: String,
    #[serde(default)]
    pub variables: Option<Value>,
}

/// Result of a dynamic tool execution.
#[derive(Debug, Serialize)]
pub struct ToolResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Tool specification for advertising to the ACP session.
#[derive(Debug, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Get the tool spec for github_graphql.
pub fn github_graphql_spec() -> ToolSpec {
    ToolSpec {
        name: "github_graphql".to_string(),
        description:
            "Execute a GraphQL query against the GitHub API using Symphony's configured auth."
                .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "A single GraphQL query or mutation document"
                },
                "variables": {
                    "type": "object",
                    "description": "Optional GraphQL variables"
                }
            },
            "required": ["query"]
        }),
    }
}

/// Execute the github_graphql tool.
pub async fn execute_github_graphql(input: Value, token: &str, endpoint: &str) -> ToolResult {
    let parsed: GraphqlToolInput = match serde_json::from_value(input) {
        Ok(parsed) => parsed,
        Err(error) => {
            warn!(%error, "invalid github_graphql input");
            return ToolResult {
                success: false,
                data: None,
                error: Some(format!("invalid input: {error}")),
            };
        }
    };

    if parsed.query.trim().is_empty() {
        debug!("rejected github_graphql call with empty query");
        return ToolResult {
            success: false,
            data: None,
            error: Some("query must be non-empty".to_string()),
        };
    }

    let operation_markers = [
        "query ",
        "mutation ",
        "subscription ",
        "query{",
        "mutation{",
        "subscription{",
    ];
    let operation_count = operation_markers
        .iter()
        .filter(|marker| parsed.query.contains(**marker))
        .count()
        + usize::from(parsed.query.trim().starts_with('{'));

    if operation_count > 1 {
        debug!(
            operation_count,
            "rejected github_graphql multi-operation document"
        );
        return ToolResult {
            success: false,
            data: None,
            error: Some("query must contain exactly one GraphQL operation".to_string()),
        };
    }

    let mut body = serde_json::json!({ "query": parsed.query });
    if let Some(variables) = parsed.variables {
        body["variables"] = variables;
    }

    let graphql_url = if endpoint.ends_with("/graphql") {
        endpoint.to_string()
    } else {
        format!("{}/graphql", endpoint.trim_end_matches('/'))
    };

    debug!(graphql_url, "executing github_graphql request");
    let client = Client::new();
    let response = match client
        .post(&graphql_url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Content-Type", "application/json")
        .header("User-Agent", "symphony-rust/0.1")
        .json(&body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            warn!(%error, "github_graphql transport error");
            return ToolResult {
                success: false,
                data: None,
                error: Some(format!("transport error: {error}")),
            };
        }
    };

    let status = response.status().as_u16();
    let response_body: Value = match response.json().await {
        Ok(value) => value,
        Err(error) => {
            warn!(status, %error, "github_graphql response parse error");
            return ToolResult {
                success: false,
                data: None,
                error: Some(format!("response parse error (status {status}): {error}")),
            };
        }
    };

    if let Some(errors) = response_body.get("errors").and_then(Value::as_array) {
        if !errors.is_empty() {
            warn!(
                status,
                error_count = errors.len(),
                "github_graphql returned GraphQL errors"
            );
            return ToolResult {
                success: false,
                data: Some(response_body),
                error: Some("GraphQL errors in response".to_string()),
            };
        }
    }

    ToolResult {
        success: true,
        data: Some(response_body),
        error: None,
    }
}

/// Route a dynamic tool call to the appropriate handler.
/// Returns None if the tool name is not supported.
pub async fn handle_tool_call(
    tool_name: &str,
    input: Value,
    github_token: Option<&str>,
    github_endpoint: &str,
) -> Option<ToolResult> {
    match tool_name {
        "github_graphql" => {
            let token = match github_token {
                Some(token) if !token.is_empty() => token,
                _ => {
                    debug!("github_graphql requested without auth token");
                    return Some(ToolResult {
                        success: false,
                        data: None,
                        error: Some("github_graphql requires GITHUB_TOKEN auth".to_string()),
                    });
                }
            };

            Some(execute_github_graphql(input, token, github_endpoint).await)
        }
        _ => None,
    }
}
