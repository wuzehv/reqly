use std::{collections::HashMap, fs, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

use crate::state_path;

const DEFAULT_PRE_SCRIPT: &str =
    "// 发送集合内的请求前运行\n// pm.environment.set(\"token\", \"your-token\");";
const DEFAULT_POST_SCRIPT: &str =
    "// 收到响应后运行\n// if (pm.response.code === 401) { /* 刷新凭证 */ }";

#[derive(Clone)]
struct McpState {
    app: AppHandle,
    manager: McpManager,
}

#[derive(Clone, Default)]
pub(crate) struct McpManager {
    running: Arc<Mutex<Option<RunningServer>>>,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>>,
}

struct RunningServer {
    port: u16,
    shutdown: oneshot::Sender<()>,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpStatus {
    pub(crate) running: bool,
    pub(crate) port: Option<u16>,
}

impl McpManager {
    pub(crate) async fn configure(
        &self,
        app: AppHandle,
        enabled: bool,
        port: u16,
    ) -> Result<McpStatus, String> {
        if enabled && port < 1024 {
            return Err("MCP port must be between 1024 and 65535".into());
        }

        let mut running = self.running.lock().await;
        if let Some(current) = running.as_ref()
            && enabled
            && current.port == port
        {
            return Ok(McpStatus {
                running: true,
                port: Some(port),
            });
        }
        if let Some(current) = running.take() {
            let _ = current.shutdown.send(());
        }
        if !enabled {
            return Ok(McpStatus {
                running: false,
                port: None,
            });
        }

        let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
            .await
            .map_err(|error| format!("cannot bind 0.0.0.0:{port}: {error}"))?;
        let (shutdown, shutdown_receiver) = oneshot::channel();
        let router = mcp_router(app, self.clone());
        tauri::async_runtime::spawn(async move {
            let result = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = shutdown_receiver.await;
                })
                .await;
            if let Err(error) = result {
                eprintln!("Reqly MCP server failed: {error}");
            }
        });
        *running = Some(RunningServer { port, shutdown });
        println!("Reqly MCP listening on http://0.0.0.0:{port}/mcp");
        Ok(McpStatus {
            running: true,
            port: Some(port),
        })
    }

    pub(crate) async fn status(&self) -> McpStatus {
        let running = self.running.lock().await;
        McpStatus {
            running: running.is_some(),
            port: running.as_ref().map(|server| server.port),
        }
    }

    pub(crate) async fn complete_request(
        &self,
        call_id: String,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<(), String> {
        let sender = self
            .pending_requests
            .lock()
            .await
            .remove(&call_id)
            .ok_or_else(|| format!("MCP request is no longer pending: {call_id}"))?;
        let completion = error.map_or_else(
            || result.ok_or_else(|| "MCP request completed without a result".to_string()),
            Err,
        );
        let _ = sender.send(completion);
        Ok(())
    }

    async fn run_request(&self, app: &AppHandle, arguments: &Value) -> Result<Value, String> {
        let request_id = required_string(arguments, "requestId")?;
        let call_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.pending_requests
            .lock()
            .await
            .insert(call_id.clone(), sender);
        let payload = json!({
            "callId": call_id,
            "requestId": request_id,
            "variables": arguments.get("variables").cloned().unwrap_or_else(|| json!({}))
        });
        if let Err(error) = app.emit("mcp-run-request", payload) {
            self.pending_requests.lock().await.remove(&call_id);
            return Err(error.to_string());
        }
        match tokio::time::timeout(std::time::Duration::from_secs(120), receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Reqly closed before the request completed".into()),
            Err(_) => {
                self.pending_requests.lock().await.remove(&call_id);
                Err("Reqly request execution timed out after 120 seconds".into())
            }
        }
    }
}

fn mcp_router(app: AppHandle, manager: McpManager) -> Router {
    Router::new()
        .route("/mcp", post(handle_post).get(handle_get))
        .route("/health", get(|| async { Json(json!({ "status": "ok" })) }))
        .with_state(McpState { app, manager })
}

async fn handle_get() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        [("allow", "POST")],
        "This stateless MCP server does not provide an SSE stream",
    )
        .into_response()
}

async fn handle_post(
    State(state): State<McpState>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    if !origin_allowed(&headers) {
        return (StatusCode::FORBIDDEN, "Origin is not allowed").into_response();
    }

    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": negotiated_protocol_version(&message),
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": "reqly", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(&state, message.get("params").cloned().unwrap_or_default()).await,
        _ => Err((-32601, format!("Method not found: {method}"))),
    };

    let payload = match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }),
        Err((code, error)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": error }
        }),
    };
    (StatusCode::OK, Json(payload)).into_response()
}

fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    origin.starts_with("http://127.0.0.1:")
        || origin == "http://127.0.0.1"
        || origin.starts_with("http://localhost:")
        || origin == "http://localhost"
}

fn negotiated_protocol_version(message: &Value) -> &str {
    match message
        .pointer("/params/protocolVersion")
        .and_then(Value::as_str)
    {
        Some("2025-06-18") => "2025-06-18",
        _ => "2025-03-26",
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "reqly_list_collections",
            "description": "List Reqly collections and requests with their IDs, methods, and URLs.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "reqly_create_collection",
            "description": "Create an API collection in Reqly.",
            "inputSchema": {
                "type": "object",
                "properties": { "name": { "type": "string", "minLength": 1 } },
                "required": ["name"], "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_create_subgroup",
            "description": "Create a second-level group in a Reqly collection. Requests in the subgroup inherit the parent collection Pre-request script.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collectionId": { "type": "string" },
                    "name": { "type": "string", "minLength": 1 }
                },
                "required": ["collectionId", "name"], "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_create_request",
            "description": "Create an HTTP request in a Reqly collection or second-level group.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collectionId": { "type": "string" },
                    "name": { "type": "string", "minLength": 1 },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] },
                    "url": { "type": "string", "minLength": 1 },
                    "params": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "headers": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "bodyType": { "type": "string", "enum": ["none", "json", "text", "form"] },
                    "body": { "description": "Raw JSON/text body, or an object for form body" }
                },
                "required": ["collectionId", "name", "method", "url"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_run_request",
            "description": "Run a saved Reqly request through the same execution pipeline as the Reqly UI, including workspace variables and Pre-request/Post-response scripts. Optional variables override workspace values for this run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "requestId": { "type": "string" },
                    "variables": { "type": "object", "additionalProperties": true }
                },
                "required": ["requestId"], "additionalProperties": false
            }
        }),
    ]
}

async fn call_tool(state: &McpState, params: Value) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = match name {
        "reqly_list_collections" => list_collections(&state.app),
        "reqly_create_collection" => create_collection(&state.app, &arguments),
        "reqly_create_subgroup" => create_subgroup(&state.app, &arguments),
        "reqly_create_request" => create_request(&state.app, &arguments),
        "reqly_run_request" => state.manager.run_request(&state.app, &arguments).await,
        _ => return Err((-32602, format!("Unknown tool: {name}"))),
    };
    Ok(match result {
        Ok(value) => tool_result(value, false),
        Err(error) => tool_result(json!({ "error": error }), true),
    })
}

fn tool_result(value: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()) }],
        "structuredContent": value,
        "isError": is_error
    })
}

fn load_workspace(app: &AppHandle) -> Result<Value, String> {
    let path = state_path(app)?;
    if !path.exists() {
        return Err("Reqly workspace has not been saved yet; open Reqly once first".into());
    }
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str(&content).map_err(|error| error.to_string())
}

fn save_workspace(app: &AppHandle, workspace: &Value) -> Result<(), String> {
    let path = state_path(app)?;
    let temporary = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    let content = serde_json::to_vec_pretty(workspace).map_err(|error| error.to_string())?;
    fs::write(&temporary, content).map_err(|error| error.to_string())?;
    fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
    app.emit("workspace-updated", workspace.clone())
        .map_err(|error| error.to_string())
}

fn list_collections(app: &AppHandle) -> Result<Value, String> {
    let workspace = load_workspace(app)?;
    let collections = workspace["collections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|collection| {
            let requests = collection["requests"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|request| json!({
                    "id": request["id"], "name": request["name"], "method": request["method"],
                    "url": request["url"], "archived": request["archived"], "pinned": request["pinned"]
                }))
                .collect::<Vec<_>>();
            let groups = collection["groups"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|group| {
                    let requests = group["requests"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|request| json!({
                            "id": request["id"], "name": request["name"], "method": request["method"],
                            "url": request["url"], "archived": request["archived"], "pinned": request["pinned"]
                        }))
                        .collect::<Vec<_>>();
                    json!({ "id": group["id"], "name": group["name"], "requests": requests })
                })
                .collect::<Vec<_>>();
            json!({ "id": collection["id"], "name": collection["name"], "requests": requests, "groups": groups })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "collections": collections }))
}

fn create_collection(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "name")?;
    let mut workspace = load_workspace(app)?;
    let collection = json!({
        "id": Uuid::new_v4().to_string(), "name": name, "expanded": true,
        "preScript": DEFAULT_PRE_SCRIPT, "requests": [], "groups": []
    });
    workspace["collections"]
        .as_array_mut()
        .ok_or("Invalid workspace: collections must be an array")?
        .push(collection.clone());
    save_workspace(app, &workspace)?;
    Ok(json!({ "collection": collection }))
}

fn create_subgroup(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let collection_id = required_string(arguments, "collectionId")?;
    let name = required_string(arguments, "name")?;
    let mut workspace = load_workspace(app)?;
    let collection = workspace["collections"]
        .as_array_mut()
        .and_then(|items| {
            items
                .iter_mut()
                .find(|item| item["id"].as_str() == Some(collection_id.as_str()))
        })
        .ok_or_else(|| format!("Collection not found: {collection_id}"))?;
    let subgroup = json!({
        "id": Uuid::new_v4().to_string(), "name": name, "expanded": true,
        "archiveExpanded": false, "requests": []
    });
    if !collection["groups"].is_array() {
        collection["groups"] = json!([]);
    }
    collection["groups"]
        .as_array_mut()
        .unwrap()
        .push(subgroup.clone());
    save_workspace(app, &workspace)?;
    Ok(json!({ "subgroup": subgroup, "inheritsPreScriptFrom": collection_id }))
}

fn create_request(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let collection_id = required_string(arguments, "collectionId")?;
    let name = required_string(arguments, "name")?;
    let method = required_string(arguments, "method")?.to_uppercase();
    if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].contains(&method.as_str()) {
        return Err(format!("Unsupported HTTP method: {method}"));
    }
    let url = required_string(arguments, "url")?;
    let body_type = arguments
        .get("bodyType")
        .and_then(Value::as_str)
        .unwrap_or("none");
    if !["none", "json", "text", "form"].contains(&body_type) {
        return Err(format!("Unsupported bodyType: {body_type}"));
    }
    let params = pairs_from_object(arguments.get("params"));
    let mut headers = pairs_from_object(arguments.get("headers"));
    if headers.is_empty() {
        headers.push(pair_value("Accept", "application/json"));
    }
    let (body, form_body) = if body_type == "form" {
        (String::new(), pairs_from_object(arguments.get("body")))
    } else {
        (value_as_body(arguments.get("body")), vec![empty_pair()])
    };
    let request = json!({
        "id": Uuid::new_v4().to_string(), "name": name, "method": method, "url": url,
        "params": if params.is_empty() { vec![empty_pair()] } else { params },
        "headers": headers, "bodyType": body_type, "body": body, "formBody": form_body,
        "formData": [empty_pair()], "postScript": DEFAULT_POST_SCRIPT, "batchData": "",
        "pinned": false, "archived": false
    });
    let mut workspace = load_workspace(app)?;
    let collections = workspace["collections"]
        .as_array_mut()
        .ok_or("Invalid workspace: collections must be an array")?;
    let mut target = None;
    for collection in collections {
        if collection["id"].as_str() == Some(collection_id.as_str()) {
            target = Some(collection);
            break;
        }
        if let Some(group) = collection["groups"].as_array_mut().and_then(|groups| {
            groups
                .iter_mut()
                .find(|group| group["id"].as_str() == Some(collection_id.as_str()))
        }) {
            target = Some(group);
            break;
        }
    }
    target.ok_or_else(|| format!("Collection or subgroup not found: {collection_id}"))?["requests"]
        .as_array_mut()
        .ok_or("Invalid group: requests must be an array")?
        .push(request.clone());
    save_workspace(app, &workspace)?;
    Ok(json!({ "request": request }))
}

fn required_string(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("Missing required field: {key}"))
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(_) | Value::Number(_) => value.to_string(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn pair_value(key: &str, value: &str) -> Value {
    json!({ "id": Uuid::new_v4().to_string(), "key": key, "value": value, "enabled": true })
}

fn empty_pair() -> Value {
    pair_value("", "")
}

fn pairs_from_object(value: Option<&Value>) -> Vec<Value> {
    value
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .map(|(key, value)| pair_value(key, &scalar_text(value)))
                .collect()
        })
        .unwrap_or_default()
}

fn value_as_body(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(value) => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}
