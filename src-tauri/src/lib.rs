use reqwest::{Method, Proxy, header::HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, error::Error, fs, net::IpAddr, time::Instant};
use tauri::{AppHandle, Emitter, Manager, State};

mod mcp;
mod storage;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct KeyValue {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProxyConfig {
    pub(crate) enabled: bool,
    pub(crate) url: String,
    pub(crate) username: String,
    pub(crate) password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequestInput {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) headers: Vec<KeyValue>,
    pub(crate) body: String,
    pub(crate) proxy: ProxyConfig,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseOutput {
    status: u16,
    status_text: String,
    duration_ms: u128,
    size: usize,
    headers: Vec<(String, String)>,
    body: String,
}

fn request_error(error: reqwest::Error) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(cause) = source {
        let message = cause.to_string();
        if !messages.contains(&message) {
            messages.push(message);
        }
        source = cause.source();
    }
    format!("请求失败：{}", messages.join("；"))
}

#[tauri::command]
fn load_workspace(app: AppHandle) -> Result<Option<Value>, String> {
    storage::load_workspace_index(&app)
}

#[tauri::command]
fn save_workspace(app: AppHandle, workspace: Value) -> Result<(), String> {
    storage::save_workspace(&app, &workspace)
}

#[tauri::command]
fn load_request(app: AppHandle, request_id: String) -> Result<Option<Value>, String> {
    storage::get_request(&app, &request_id)
}

#[tauri::command]
fn load_group_requests(app: AppHandle, group_id: String) -> Result<Vec<Value>, String> {
    storage::load_container_requests(&app, &group_id)
}

#[tauri::command]
fn locate_request(app: AppHandle, request_id: String) -> Result<Option<Value>, String> {
    storage::locate_request(&app, &request_id)
}

#[tauri::command]
fn search_requests(
    app: AppHandle,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<Value>, String> {
    storage::search_requests(&app, &query, limit.unwrap_or(500))
}

#[tauri::command]
fn save_request(app: AppHandle, request: Value) -> Result<(), String> {
    storage::save_request(&app, &request)
}

#[tauri::command]
async fn configure_mcp(
    app: AppHandle,
    manager: State<'_, mcp::McpManager>,
    enabled: bool,
    port: u16,
) -> Result<mcp::McpStatus, String> {
    manager.configure(app, enabled, port).await
}

#[tauri::command]
async fn get_mcp_status(manager: State<'_, mcp::McpManager>) -> Result<mcp::McpStatus, String> {
    Ok(manager.status().await)
}

#[tauri::command]
async fn complete_mcp_request(
    manager: State<'_, mcp::McpManager>,
    call_id: String,
    result: Option<Value>,
    error: Option<String>,
) -> Result<(), String> {
    manager.complete_request(call_id, result, error).await
}

#[tauri::command]
fn get_mcp_addresses(port: u16) -> Result<Vec<String>, String> {
    let mut addresses = BTreeSet::new();
    for interface in if_addrs::get_if_addrs().map_err(|error| error.to_string())? {
        let ip = interface.ip();
        if ip.is_unspecified() {
            continue;
        }
        if let IpAddr::V4(address) = ip {
            addresses.insert(format!("http://{address}:{port}/mcp"));
        }
    }
    Ok(addresses.into_iter().collect())
}

#[tauri::command]
fn write_export_file(path: String, content: String) -> Result<(), String> {
    fs::write(path, content).map_err(|error| error.to_string())
}

#[tauri::command]
async fn import_artifact(
    app: AppHandle,
    path: String,
    target_group_id: Option<String>,
) -> Result<Value, String> {
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        storage::import_artifact(&app, &path, target_group_id.as_deref(), |percent, label| {
            let _ = progress_app.emit(
                "artifact-import-progress",
                serde_json::json!({
                    "percent": percent,
                    "label": label,
                }),
            );
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn execute_request(input: RequestInput) -> Result<ResponseOutput, String> {
    let mut client = reqwest::Client::builder();
    if input.proxy.enabled {
        if input.proxy.url.trim().is_empty() {
            return Err("代理已启用，但代理地址为空".into());
        }
        let configured_url = input.proxy.url.trim();
        let proxy_url = if configured_url.contains("://") {
            configured_url.to_string()
        } else {
            format!("http://{configured_url}")
        };
        let mut proxy = Proxy::all(&proxy_url)
            .map_err(|error| format!("代理地址无效（{proxy_url}）：{error}"))?;
        if !input.proxy.username.is_empty() {
            proxy = proxy.basic_auth(&input.proxy.username, &input.proxy.password);
        }
        client = client.proxy(proxy);
    }

    let client = client.build().map_err(|error| error.to_string())?;
    let method = Method::from_bytes(input.method.as_bytes()).map_err(|error| error.to_string())?;
    let mut headers = HeaderMap::new();
    for item in input
        .headers
        .into_iter()
        .filter(|item| item.enabled && !item.key.trim().is_empty())
    {
        let name = reqwest::header::HeaderName::from_bytes(item.key.trim().as_bytes())
            .map_err(|error| format!("请求头 {} 无效: {error}", item.key))?;
        let value = reqwest::header::HeaderValue::from_str(&item.value)
            .map_err(|error| format!("请求头 {} 的值无效: {error}", item.key))?;
        headers.append(name, value);
    }

    let mut request = client.request(method, input.url.trim()).headers(headers);
    if !input.body.is_empty() {
        request = request.body(input.body);
    }

    let started = Instant::now();
    let response = request.send().await.map_err(request_error)?;
    let status = response.status();
    let response_headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value.to_str().unwrap_or("<binary>").to_string(),
            )
        })
        .collect();
    let bytes = response.bytes().await.map_err(request_error)?;
    let size = bytes.len();

    Ok(ResponseOutput {
        status: status.as_u16(),
        status_text: status.canonical_reason().unwrap_or("").to_string(),
        duration_ms: started.elapsed().as_millis(),
        size,
        headers: response_headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

#[tauri::command]
async fn send_request(input: RequestInput) -> Result<ResponseOutput, String> {
    execute_request(input).await
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(mcp::McpManager::default())
        .setup(|app| {
            let app_handle = app.handle().clone();
            storage::initialize(&app_handle).map_err(std::io::Error::other)?;
            let manager = app.state::<mcp::McpManager>().inner().clone();
            let (enabled, port) = storage::load_workspace_index(&app_handle)
                .ok()
                .flatten()
                .map(|workspace| {
                    (
                        workspace
                            .pointer("/mcp/enabled")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        workspace
                            .pointer("/mcp/port")
                            .and_then(Value::as_u64)
                            .and_then(|value| u16::try_from(value).ok())
                            .unwrap_or(3765),
                    )
                })
                .unwrap_or((true, 3765));
            tauri::async_runtime::spawn(async move {
                if let Err(error) = manager.configure(app_handle, enabled, port).await {
                    eprintln!("Reqly MCP server failed: {error}");
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_workspace,
            save_workspace,
            load_request,
            load_group_requests,
            locate_request,
            search_requests,
            save_request,
            send_request,
            configure_mcp,
            get_mcp_status,
            complete_mcp_request,
            get_mcp_addresses,
            write_export_file,
            import_artifact
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
