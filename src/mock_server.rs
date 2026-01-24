use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::any;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::info;

#[derive(Debug, Clone)]
pub struct MockServerOptions {
    pub host: String,
    pub port: u16,
}

#[derive(Debug)]
pub struct MockServerHandle {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<Result<(), String>>,
}

#[derive(Debug, Clone)]
struct MockState {
    accounts: Vec<(String, String)>,
    roles: HashMap<String, Vec<String>>,
}

impl Default for MockServerOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 7777,
        }
    }
}

pub async fn run_mock_server(options: MockServerOptions) -> Result<(), String> {
    let state = Arc::new(default_state());
    let app = build_router(state);

    let addr: SocketAddr = format!("{}:{}", options.host, options.port)
        .parse()
        .map_err(|err: std::net::AddrParseError| err.to_string())?;
    info!(%addr, "starting mock server");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| err.to_string())?;
    axum::serve(listener, app)
        .await
        .map_err(|err| err.to_string())
}

pub async fn start_mock_server(options: MockServerOptions) -> Result<MockServerHandle, String> {
    let state = Arc::new(default_state());
    let app = build_router(state);
    let addr: SocketAddr = format!("{}:{}", options.host, options.port)
        .parse()
        .map_err(|err: std::net::AddrParseError| err.to_string())?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| err.to_string())?;
    let addr = listener.local_addr().map_err(|err| err.to_string())?;
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .map_err(|err| err.to_string())
    });
    Ok(MockServerHandle {
        addr,
        shutdown: Some(shutdown_tx),
        task,
    })
}

impl MockServerHandle {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub async fn shutdown(mut self) -> Result<(), String> {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        match self.task.await {
            Ok(result) => result,
            Err(err) => Err(err.to_string()),
        }
    }
}

async fn handle_root(
    State(state): State<Arc<MockState>>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> impl IntoResponse {
    let target = headers
        .get("x-amz-target")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let payload = match parse_json(body).await {
        Ok(value) => value,
        Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
    };

    let resolved = resolve_target(target, &payload, &uri);

    match resolved.as_str() {
        "SSOOIDCService.RegisterClient" | "AWSSSOOIDCService.RegisterClient" => Json(json!({
            "clientId": "mock-client",
            "clientSecret": "mock-secret",
            "clientSecretExpiresAt": epoch_seconds() + 86400,
        }))
        .into_response(),
        "SSOOIDCService.StartDeviceAuthorization"
        | "AWSSSOOIDCService.StartDeviceAuthorization" => {
            Json(json!({
                "deviceCode": "mock-device",
                "userCode": "MOCK-1234",
                "verificationUriComplete": "https://mock.awsapps.com/start/#/device?user_code=MOCK-1234",
                "expiresIn": 600,
                "interval": 1,
            }))
            .into_response()
        }
        "SSOOIDCService.CreateToken" | "AWSSSOOIDCService.CreateToken" => Json(json!({
            "accessToken": "mock-access-token",
            "expiresIn": 28800,
        }))
        .into_response(),
        "AWSSSOService.ListAccounts" => {
            let accounts = state
                .accounts
                .iter()
                .map(|(id, name)| json!({ "accountId": id, "accountName": name }))
                .collect::<Vec<_>>();
            Json(json!({
                "accountList": accounts,
                "nextToken": null,
            }))
            .into_response()
        }
        "AWSSSOService.ListAccountRoles" => {
            let account_id = payload
                .get("accountId")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string())
                .or_else(|| query_value(uri.query(), &["account_id", "accountId"]))
                .unwrap_or_default();
            let roles = state
                .roles
                .get(&account_id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|role| json!({ "roleName": role, "accountId": account_id }))
                .collect::<Vec<_>>();
            Json(json!({
                "roleList": roles,
                "nextToken": null,
            }))
            .into_response()
        }
        "AWSSSOService.GetRoleCredentials" => {
            let account_id = payload
                .get("accountId")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string())
                .or_else(|| query_value(uri.query(), &["account_id", "accountId"]))
                .unwrap_or_default();
            let role_name = payload
                .get("roleName")
                .and_then(|v| v.as_str())
                .map(|value| value.to_string())
                .or_else(|| query_value(uri.query(), &["role_name", "roleName"]))
                .unwrap_or_default();
            let access_key_id = "ASIAMOCKACCESSKEY";
            let secret_access_key = "mock-secret-access-key";
            let session_token = "mock-session-token";
            let expiration = epoch_millis() + 8 * 60 * 60 * 1000;
            Json(json!({
                "roleCredentials": {
                    "accessKeyId": access_key_id,
                    "secretAccessKey": secret_access_key,
                    "sessionToken": session_token,
                    "expiration": expiration,
                    "accountId": account_id,
                    "roleName": role_name,
                }
            }))
            .into_response()
        }
        _ => (StatusCode::BAD_REQUEST, format!("unknown target: {target}")).into_response(),
    }
}

async fn parse_json(bytes: Bytes) -> Result<Value, String> {
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|err| err.to_string())
}

fn default_state() -> MockState {
    let accounts = vec![
        ("111111111111".to_string(), "Mock Platform".to_string()),
        ("222222222222".to_string(), "Mock Data".to_string()),
        ("333333333333".to_string(), "Mock Sandbox".to_string()),
    ];
    let mut roles = HashMap::new();
    roles.insert(
        "111111111111".to_string(),
        vec!["Admin".to_string(), "ReadOnly".to_string()],
    );
    roles.insert(
        "222222222222".to_string(),
        vec!["Engineer".to_string(), "Billing".to_string()],
    );
    roles.insert(
        "333333333333".to_string(),
        vec!["Sandbox".to_string()],
    );
    MockState { accounts, roles }
}

fn resolve_target(target: &str, payload: &Value, uri: &Uri) -> String {
    if !target.is_empty() {
        return target.to_string();
    }
    if payload.get("clientName").is_some() || payload.get("clientType").is_some() {
        return "SSOOIDCService.RegisterClient".to_string();
    }
    if payload.get("startUrl").is_some() {
        return "SSOOIDCService.StartDeviceAuthorization".to_string();
    }
    if payload.get("grantType").is_some() || payload.get("grant_type").is_some() {
        return "SSOOIDCService.CreateToken".to_string();
    }
    if payload.get("accountId").is_some() && payload.get("roleName").is_some() {
        return "AWSSSOService.GetRoleCredentials".to_string();
    }
    if payload.get("accountId").is_some() {
        return "AWSSSOService.ListAccountRoles".to_string();
    }
    if payload.get("accessToken").is_some() {
        return "AWSSSOService.ListAccounts".to_string();
    }
    if let Some(query) = uri.query() {
        if has_query_param(query, &["role_name", "roleName"]) {
            return "AWSSSOService.GetRoleCredentials".to_string();
        }
        if has_query_param(query, &["account_id", "accountId"]) {
            return "AWSSSOService.ListAccountRoles".to_string();
        }
    }
    if uri.path().contains("credential") {
        return "AWSSSOService.GetRoleCredentials".to_string();
    }
    if uri.path().contains("role") {
        return "AWSSSOService.ListAccountRoles".to_string();
    }
    if uri.path().contains("account") {
        return "AWSSSOService.ListAccounts".to_string();
    }
    "AWSSSOService.ListAccounts".to_string()
}

fn has_query_param(query: &str, keys: &[&str]) -> bool {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .any(|(key, _)| keys.iter().any(|candidate| candidate == &key))
}

fn query_value(query: Option<&str>, keys: &[&str]) -> Option<String> {
    let query = query?;
    for (key, value) in query.split('&').filter_map(|pair| pair.split_once('=')) {
        if keys.iter().any(|candidate| candidate == &key) {
            return Some(value.to_string());
        }
    }
    None
}

fn build_router(state: Arc<MockState>) -> Router {
    Router::new()
        .route("/", any(handle_root))
        .route("/*path", any(handle_root))
        .with_state(state)
}

fn epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
