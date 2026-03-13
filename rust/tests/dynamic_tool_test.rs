use rusty::agent::dynamic_tool::{
    execute_github_graphql, github_graphql_spec, handle_tool_call, GraphqlToolInput, ToolResult,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn github_graphql_spec_returns_expected_schema() {
    let spec = github_graphql_spec();

    assert_eq!(spec.name, "github_graphql");
    assert_eq!(
        spec.description,
        "Execute a GraphQL query against the GitHub API using Rusty's configured auth."
    );
    assert_eq!(spec.input_schema["type"], json!("object"));
    assert_eq!(spec.input_schema["required"], json!(["query"]));
    assert_eq!(
        spec.input_schema["properties"]["query"]["type"],
        json!("string")
    );
    assert_eq!(
        spec.input_schema["properties"]["variables"]["type"],
        json!("object")
    );
}

#[tokio::test]
async fn execute_github_graphql_empty_query_returns_error() {
    let result =
        execute_github_graphql(json!({ "query": "   " }), "token", "https://api.github.com").await;

    assert!(!result.success);
    assert_eq!(result.data, None);
    assert_eq!(result.error.as_deref(), Some("query must be non-empty"));
}

#[tokio::test]
async fn execute_github_graphql_multi_operation_query_is_rejected() {
    let result = execute_github_graphql(
        json!({ "query": "query One { viewer { login } } mutation Two { updateIssue(input: {}) { clientMutationId } }" }),
        "token",
        "https://api.github.com",
    )
    .await;

    assert!(!result.success);
    assert_eq!(result.data, None);
    assert_eq!(
        result.error.as_deref(),
        Some("query must contain exactly one GraphQL operation")
    );
}

#[tokio::test]
async fn handle_tool_call_returns_none_for_unsupported_tool() {
    let result = handle_tool_call(
        "unsupported_tool",
        json!({}),
        Some("token"),
        "https://api.github.com",
    )
    .await;

    assert!(result.is_none());
}

#[tokio::test]
async fn handle_tool_call_requires_token_for_github_graphql() {
    let result = handle_tool_call(
        "github_graphql",
        json!({ "query": "{ viewer { login } }" }),
        None,
        "https://api.github.com",
    )
    .await
    .expect("github_graphql should be handled");

    assert!(!result.success);
    assert_eq!(result.data, None);
    assert_eq!(
        result.error.as_deref(),
        Some("github_graphql requires GITHUB_TOKEN auth")
    );
}

#[test]
fn graphql_tool_input_deserializes_with_and_without_variables() {
    let with_variables: GraphqlToolInput = serde_json::from_value(json!({
        "query": "query Viewer($login: String!) { user(login: $login) { id } }",
        "variables": { "login": "octocat" }
    }))
    .expect("input with variables should deserialize");
    assert_eq!(
        with_variables.query,
        "query Viewer($login: String!) { user(login: $login) { id } }"
    );
    assert_eq!(
        with_variables.variables,
        Some(json!({ "login": "octocat" }))
    );

    let without_variables: GraphqlToolInput =
        serde_json::from_value(json!({ "query": "{ viewer { login } }" }))
            .expect("input without variables should deserialize");
    assert_eq!(without_variables.query, "{ viewer { login } }");
    assert_eq!(without_variables.variables, None);
}

#[test]
fn tool_result_serializes_for_success_and_error_cases() {
    let success = ToolResult {
        success: true,
        data: Some(json!({ "data": { "viewer": { "login": "octocat" } } })),
        error: None,
    };
    let success_value = serde_json::to_value(&success).expect("success result should serialize");
    assert_eq!(
        success_value,
        json!({
            "success": true,
            "data": { "data": { "viewer": { "login": "octocat" } } }
        })
    );

    let error = ToolResult {
        success: false,
        data: None,
        error: Some("GraphQL errors in response".to_string()),
    };
    let error_value = serde_json::to_value(&error).expect("error result should serialize");
    assert_eq!(
        error_value,
        json!({
            "success": false,
            "error": "GraphQL errors in response"
        })
    );
}

#[tokio::test]
async fn execute_github_graphql_success_with_mock_server() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {"viewer": {"login": "testuser"}}
        })))
        .mount(&server)
        .await;

    let result = execute_github_graphql(
        json!({"query": "{ viewer { login } }"}),
        "test-token",
        &server.uri(),
    )
    .await;

    assert!(result.success);
    assert_eq!(
        result.data,
        Some(json!({
            "data": {"viewer": {"login": "testuser"}}
        }))
    );
    assert_eq!(result.error, None);
}

#[tokio::test]
async fn execute_github_graphql_returns_failure_on_graphql_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{"message": "Field not found"}]
        })))
        .mount(&server)
        .await;

    let result =
        execute_github_graphql(json!({"query": "{ invalid }"}), "test-token", &server.uri()).await;

    assert!(!result.success);
    assert_eq!(
        result.data,
        Some(json!({
            "errors": [{"message": "Field not found"}]
        }))
    );
    assert_eq!(result.error.as_deref(), Some("GraphQL errors in response"));
}

#[tokio::test]
async fn execute_github_graphql_handles_transport_error() {
    let result = execute_github_graphql(
        json!({"query": "{ viewer { login } }"}),
        "test-token",
        "http://127.0.0.1:1",
    )
    .await;

    assert!(!result.success);
    assert_eq!(result.data, None);
    assert!(matches!(result.error.as_deref(), Some(error) if error.contains("transport")));
}

#[tokio::test]
#[ignore = "requires live GitHub GraphQL endpoint"]
async fn execute_github_graphql_live_request() {
    let _ = execute_github_graphql(
        json!({ "query": "{ viewer { login } }" }),
        "token",
        "https://api.github.com",
    )
    .await;
}
