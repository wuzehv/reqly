use std::{collections::HashMap, sync::Arc};

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

use crate::storage;

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
    workspace_mutations: Arc<Mutex<()>>,
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
            "variables": arguments.get("variables").cloned().unwrap_or_else(|| json!({})),
            "environmentId": arguments.get("environmentId").cloned().unwrap_or(Value::Null)
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
            "serverInfo": { "name": "reqly", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Environment workflow: before running requests in a named environment, call reqly_list_environments, select the exact returned environment ID without guessing, call reqly_switch_environment once, then call reqly_run_request without environmentId. The active environment persists for subsequent runs until explicitly switched again. If the requested environment name is missing or ambiguous, ask the user instead of choosing an ID."
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
            "name": "reqly_list_environments",
            "description": "List Reqly workspace environments and the active environment ID. Call this before switching environments; environment IDs must come from this result and must never be guessed.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "reqly_switch_environment",
            "description": "Switch Reqly's active workspace environment using an exact ID returned by reqly_list_environments. Call once when the desired environment changes. Subsequent reqly_run_request calls should omit environmentId and use this active environment.",
            "inputSchema": {
                "type": "object",
                "properties": { "environmentId": { "type": "string", "minLength": 1 } },
                "required": ["environmentId"], "additionalProperties": false
            }
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
                    "name": { "type": "string", "minLength": 1, "pattern": "\\S" },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] },
                    "url": { "type": "string", "minLength": 1 },
                    "params": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "headers": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "bodyType": { "type": "string", "enum": ["none", "json", "text", "form"] },
                    "body": { "description": "Raw JSON/text body, or an object for form body" },
                    "postScript": { "type": "string", "description": "Optional Post-response JavaScript for this request." }
                },
                "required": ["collectionId", "name", "method", "url"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_list_script_templates",
            "description": "List reusable Reqly Pre Script templates and the top-level collections that reference them.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }),
        json!({
            "name": "reqly_create_script_template",
            "description": "Create a reusable Pre Script template. Templates may contain {{placeholder}} expressions configured when bound to a collection.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1, "pattern": "\\S" },
                    "script": { "type": "string" }
                },
                "required": ["name", "script"], "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_update_script_template",
            "description": "Update the name and/or JavaScript of an existing Reqly Pre Script template.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "templateId": { "type": "string" },
                    "name": { "type": "string", "minLength": 1, "pattern": "\\S" },
                    "script": { "type": "string" }
                },
                "required": ["templateId"],
                "anyOf": [{ "required": ["name"] }, { "required": ["script"] }],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_bind_script_template",
            "description": "Bind or unbind a Pre Script template on a top-level collection. Set templateId to null to unbind. Optional placeholders replace the collection placeholder configuration.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "collectionId": { "type": "string" },
                    "templateId": { "type": ["string", "null"] },
                    "placeholders": {
                        "type": "object",
                        "description": "Map placeholder names to { sourceType: variable|literal, source: environment variable name or fixed value }.",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "sourceType": { "type": "string", "enum": ["variable", "literal"] },
                                "source": { "type": "string" }
                            },
                            "required": ["sourceType", "source"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["collectionId", "templateId"], "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_delete_request",
            "description": "Delete a saved Reqly request by its ID. The request will be permanently removed from its collection or subgroup.",
            "inputSchema": {
                "type": "object",
                "properties": { "requestId": { "type": "string" } },
                "required": ["requestId"], "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_update_request",
            "description": "Update selected fields of a saved Reqly request. Fields not provided remain unchanged.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "requestId": { "type": "string" },
                    "name": { "type": "string", "minLength": 1, "pattern": "\\S" },
                    "method": { "type": "string", "enum": ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"] },
                    "url": { "type": "string", "minLength": 1, "pattern": "\\S" },
                    "params": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "headers": { "type": "object", "additionalProperties": { "type": ["string", "number", "boolean", "null"] } },
                    "bodyType": { "type": "string", "enum": ["none", "json", "text", "form"] },
                    "body": { "description": "Raw JSON/text body, or an object for form body" },
                    "postScript": { "type": "string" },
                    "pinned": { "type": "boolean" },
                    "archived": { "type": "boolean" }
                },
                "required": ["requestId"],
                "anyOf": [
                    { "required": ["name"] }, { "required": ["method"] }, { "required": ["url"] },
                    { "required": ["params"] }, { "required": ["headers"] }, { "required": ["bodyType"] },
                    { "required": ["body"] }, { "required": ["postScript"] }, { "required": ["pinned"] },
                    { "required": ["archived"] }
                ],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "reqly_run_request",
            "description": "Run a saved Reqly request through the same execution pipeline as the Reqly UI, including environment variables and Pre-request/Post-response scripts. Normally omit environmentId: first use reqly_list_environments and reqly_switch_environment, then this tool uses the persisted active environment. environmentId remains available only for an explicit one-off override. Optional variables override environment values for this run.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "requestId": { "type": "string" },
                    "environmentId": { "type": "string" },
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
    let _mutation_guard = match name {
        "reqly_list_collections"
        | "reqly_list_environments"
        | "reqly_list_script_templates"
        | "reqly_run_request" => None,
        _ => Some(state.manager.workspace_mutations.lock().await),
    };
    let result = match name {
        "reqly_list_collections" => list_collections(&state.app),
        "reqly_list_environments" => list_environments(&state.app),
        "reqly_switch_environment" => switch_environment(&state.app, &arguments),
        "reqly_create_collection" => create_collection(&state.app, &arguments),
        "reqly_create_subgroup" => create_subgroup(&state.app, &arguments),
        "reqly_create_request" => create_request(&state.app, &arguments),
        "reqly_list_script_templates" => list_script_templates(&state.app),
        "reqly_create_script_template" => create_script_template(&state.app, &arguments),
        "reqly_update_script_template" => update_script_template(&state.app, &arguments),
        "reqly_bind_script_template" => bind_script_template(&state.app, &arguments),
        "reqly_delete_request" => delete_request(&state.app, &arguments),
        "reqly_update_request" => update_request(&state.app, &arguments),
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
    storage::load_workspace_index(app)?
        .ok_or_else(|| "Reqly workspace has not been saved yet; open Reqly once first".into())
}

fn save_workspace(app: &AppHandle, workspace: &Value) -> Result<(), String> {
    storage::save_workspace(app, workspace)?;
    let index = storage::load_workspace_index(app)?.ok_or("workspace index was not saved")?;
    app.emit("workspace-updated", index)
        .map_err(|error| error.to_string())
}

fn list_collections(app: &AppHandle) -> Result<Value, String> {
    let workspace = storage::load_workspace_with_summaries(app)?.ok_or_else(|| {
        "Reqly workspace has not been saved yet; open Reqly once first".to_string()
    })?;
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
            json!({
                "id": collection["id"], "name": collection["name"],
                "preScriptTemplateId": collection["preScriptProfileId"],
                "preScriptPlaceholders": collection["preScriptPlaceholders"],
                "requests": requests, "groups": groups
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "collections": collections }))
}

fn list_environments(app: &AppHandle) -> Result<Value, String> {
    let workspace = load_workspace(app)?;
    let environments = workspace["environments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|environment| {
            json!({
                "id": environment["id"], "name": environment["name"], "color": environment["color"]
            })
        })
        .collect::<Vec<_>>();
    Ok(
        json!({ "environments": environments, "activeEnvironmentId": workspace["activeEnvironmentId"] }),
    )
}

fn switch_environment(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let environment_id = required_string(arguments, "environmentId")?;
    let mut workspace = load_workspace(app)?;
    let environment = workspace["environments"]
        .as_array()
        .and_then(|environments| {
            environments
                .iter()
                .find(|environment| environment["id"].as_str() == Some(environment_id.as_str()))
        })
        .cloned()
        .ok_or_else(|| format!("Environment not found: {environment_id}"))?;
    let changed = workspace["activeEnvironmentId"].as_str() != Some(environment_id.as_str());
    if changed {
        workspace["activeEnvironmentId"] = Value::String(environment_id.clone());
        save_workspace(app, &workspace)?;
    }
    Ok(json!({
        "activeEnvironmentId": environment_id,
        "changed": changed,
        "environment": {
            "id": environment["id"], "name": environment["name"], "color": environment["color"]
        }
    }))
}

fn list_script_templates(app: &AppHandle) -> Result<Value, String> {
    let workspace = load_workspace(app)?;
    let collections = workspace["collections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let templates = workspace["preScriptProfiles"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|template| {
            let template_id = template["id"].as_str().unwrap_or_default();
            let referenced_by = collections
                .iter()
                .filter(|collection| collection["preScriptProfileId"].as_str() == Some(template_id))
                .map(|collection| json!({ "id": collection["id"], "name": collection["name"] }))
                .collect::<Vec<_>>();
            json!({
                "id": template["id"], "name": template["name"],
                "script": template["script"], "referencedBy": referenced_by
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "templates": templates }))
}

fn create_script_template(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "name")?;
    let script = arguments
        .get("script")
        .and_then(Value::as_str)
        .ok_or("script must be a string")?;
    let mut workspace = load_workspace(app)?;
    if !workspace["preScriptProfiles"].is_array() {
        workspace["preScriptProfiles"] = json!([]);
    }
    let template = json!({ "id": Uuid::new_v4().to_string(), "name": name, "script": script });
    workspace["preScriptProfiles"]
        .as_array_mut()
        .unwrap()
        .push(template.clone());
    save_workspace(app, &workspace)?;
    Ok(json!({ "template": template }))
}

fn update_script_template(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let template_id = required_string(arguments, "templateId")?;
    let name = arguments
        .get("name")
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or("name must be a non-empty string")
        })
        .transpose()?;
    let script = arguments
        .get("script")
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or("script must be a string")
        })
        .transpose()?;
    if name.is_none() && script.is_none() {
        return Err("Provide name and/or script to update".into());
    }
    let mut workspace = load_workspace(app)?;
    let template = workspace["preScriptProfiles"]
        .as_array_mut()
        .and_then(|templates| {
            templates
                .iter_mut()
                .find(|template| template["id"].as_str() == Some(template_id.as_str()))
        })
        .ok_or_else(|| format!("Pre Script template not found: {template_id}"))?;
    if let Some(name) = name {
        template["name"] = Value::String(name);
    }
    if let Some(script) = script {
        template["script"] = Value::String(script);
    }
    let updated = template.clone();
    save_workspace(app, &workspace)?;
    Ok(json!({ "template": updated }))
}

fn bind_script_template(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let collection_id = required_string(arguments, "collectionId")?;
    let template_id = match arguments.get("templateId") {
        Some(Value::Null) => String::new(),
        Some(Value::String(value)) if !value.trim().is_empty() => value.trim().to_string(),
        _ => return Err("templateId must be a template ID or null".into()),
    };
    let mut workspace = load_workspace(app)?;
    if !template_id.is_empty()
        && !workspace["preScriptProfiles"]
            .as_array()
            .is_some_and(|templates| {
                templates
                    .iter()
                    .any(|template| template["id"].as_str() == Some(template_id.as_str()))
            })
    {
        return Err(format!("Pre Script template not found: {template_id}"));
    }
    let placeholders = arguments
        .get("placeholders")
        .map(parse_script_placeholders)
        .transpose()?;
    let collection = workspace["collections"]
        .as_array_mut()
        .and_then(|collections| {
            collections
                .iter_mut()
                .find(|collection| collection["id"].as_str() == Some(collection_id.as_str()))
        })
        .ok_or_else(|| format!("Top-level collection not found: {collection_id}"))?;
    let binding_changed = collection["preScriptProfileId"].as_str() != Some(template_id.as_str());
    collection["preScriptProfileId"] = Value::String(template_id);
    if collection["preScriptProfileId"].as_str() == Some("") {
        collection["preScriptPlaceholders"] = json!([]);
    } else if let Some(placeholders) = placeholders {
        collection["preScriptPlaceholders"] = Value::Array(placeholders);
    } else if binding_changed {
        collection["preScriptPlaceholders"] = json!([]);
    }
    let updated = collection.clone();
    save_workspace(app, &workspace)?;
    Ok(json!({ "collection": updated }))
}

fn parse_script_placeholders(value: &Value) -> Result<Vec<Value>, String> {
    let placeholders = value.as_object().ok_or("placeholders must be an object")?;
    let mut names = std::collections::HashSet::new();
    placeholders
        .iter()
        .map(|(name, configuration)| {
            let name = name.trim();
            if name.is_empty() {
                return Err("placeholder names must not be empty".into());
            }
            if !names.insert(name.to_string()) {
                return Err(format!("Duplicate placeholder name: {name}"));
            }
            let source_type = required_string(configuration, "sourceType")?;
            if source_type != "variable" && source_type != "literal" {
                return Err(format!(
                    "Unsupported sourceType for placeholder {name}: {source_type}"
                ));
            }
            let source = configuration
                .get("source")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("source must be a string for placeholder {name}"))?;
            let source = if source_type == "variable" {
                let source = source.trim();
                if source.is_empty() {
                    return Err(format!(
                        "environment variable source must not be empty for placeholder {name}"
                    ));
                }
                source
            } else {
                source
            };
            Ok(json!({
                "id": Uuid::new_v4().to_string(), "name": name,
                "sourceType": source_type, "source": source
            }))
        })
        .collect()
}

fn create_collection(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let name = required_string(arguments, "name")?;
    let mut workspace = load_workspace(app)?;
    let collection = json!({
        "id": Uuid::new_v4().to_string(), "name": name, "expanded": true,
        "preScriptProfileId": "", "preScriptPlaceholders": [], "requests": [], "requestCount": 0,
        "requestsLoaded": true, "groups": []
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
        "archiveExpanded": false, "requests": [], "requestCount": 0, "requestsLoaded": true
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
    let post_script = match arguments.get("postScript") {
        None => DEFAULT_POST_SCRIPT,
        Some(Value::String(value)) => value,
        Some(_) => return Err("postScript must be a string".into()),
    };
    let bodies = json!({
        "json": if body_type == "json" { body.clone() } else { String::new() },
        "text": if body_type == "text" { body.clone() } else { String::new() }
    });
    let request = json!({
        "id": Uuid::new_v4().to_string(), "name": name, "method": method, "url": url,
        "params": if params.is_empty() { vec![empty_pair()] } else { params },
        "headers": headers, "bodyType": body_type, "body": body, "formBody": form_body,
        "bodies": bodies,
        "bodyEnvironmentValues": { "json": {}, "text": {} },
        "formData": [empty_pair()],
        "postScript": post_script,
        "batchData": "",
        "batchPlaceholderNames": [],
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
    let target =
        target.ok_or_else(|| format!("Collection or subgroup not found: {collection_id}"))?;
    target["requests"] = Value::Array(storage::load_container_requests(app, &collection_id)?);
    target["requestsLoaded"] = Value::Bool(true);
    target["requests"]
        .as_array_mut()
        .ok_or("Invalid group: requests must be an array")?
        .push(request.clone());
    save_workspace(app, &workspace)?;
    Ok(json!({ "request": request }))
}

fn update_request(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let request_id = required_string(arguments, "requestId")?;
    let mut request = storage::get_request(app, &request_id)?
        .ok_or_else(|| format!("Request not found: {request_id}"))?;
    ensure_request_body_storage(&mut request);

    if let Some(value) = arguments.get("name") {
        let name = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("name must be a non-empty string")?;
        request["name"] = Value::String(name.to_string());
    }
    if let Some(value) = arguments.get("method") {
        let method = value
            .as_str()
            .ok_or("method must be a string")?
            .to_uppercase();
        if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].contains(&method.as_str())
        {
            return Err(format!("Unsupported HTTP method: {method}"));
        }
        request["method"] = Value::String(method);
    }
    if let Some(value) = arguments.get("url") {
        let url = value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("url must be a non-empty string")?;
        request["url"] = Value::String(url.to_string());
    }
    if let Some(value) = arguments.get("params") {
        if !value.is_object() {
            return Err("params must be an object".into());
        }
        request["params"] = Value::Array(pairs_from_object(Some(value)));
    }
    if let Some(value) = arguments.get("headers") {
        if !value.is_object() {
            return Err("headers must be an object".into());
        }
        request["headers"] = Value::Array(pairs_from_object(Some(value)));
    }
    if let Some(value) = arguments.get("bodyType") {
        let body_type = value.as_str().ok_or("bodyType must be a string")?;
        if !["none", "json", "text", "form"].contains(&body_type) {
            return Err(format!("Unsupported bodyType: {body_type}"));
        }
        request["bodyType"] = Value::String(body_type.to_string());
        request["body"] = if body_type == "json" || body_type == "text" {
            request["bodies"][body_type].clone()
        } else {
            Value::String(String::new())
        };
    }
    if let Some(value) = arguments.get("body") {
        if request["bodyType"].as_str() == Some("form") {
            if !value.is_object() {
                return Err("body must be an object when bodyType is form".into());
            }
            request["body"] = Value::String(String::new());
            request["formBody"] = Value::Array(pairs_from_object(Some(value)));
        } else {
            let body = value_as_body(Some(value));
            request["body"] = Value::String(body.clone());
            if let Some(body_type) = request["bodyType"].as_str().map(str::to_string) {
                if body_type == "json" || body_type == "text" {
                    request["bodies"][&body_type] = Value::String(body);
                }
            }
        }
    }
    if let Some(value) = arguments.get("postScript") {
        request["postScript"] = Value::String(
            value
                .as_str()
                .ok_or("postScript must be a string")?
                .to_string(),
        );
    }
    for field in ["pinned", "archived"] {
        if let Some(value) = arguments.get(field) {
            request[field] = Value::Bool(
                value
                    .as_bool()
                    .ok_or_else(|| format!("{field} must be a boolean"))?,
            );
        }
    }

    storage::save_request(app, &request)?;
    let mut result = json!({ "request": request });
    if let Err(error) = app.emit("request-detail-updated", result["request"].clone()) {
        result["syncWarning"] = Value::String(error.to_string());
    }
    Ok(result)
}

fn ensure_request_body_storage(request: &mut Value) {
    let body_type = request["bodyType"].as_str().unwrap_or("none").to_string();
    let body = request["body"].as_str().unwrap_or_default().to_string();
    if !request["bodies"].is_object() {
        request["bodies"] = json!({ "json": "", "text": "" });
    }
    if !request["bodies"]["json"].is_string() {
        request["bodies"]["json"] = Value::String(String::new());
    }
    if !request["bodies"]["text"].is_string() {
        request["bodies"]["text"] = Value::String(String::new());
    }
    if body_type == "json" || body_type == "text" {
        request["bodies"][&body_type] = Value::String(body);
    }

    let environment_values = request["bodyEnvironmentValues"].clone();
    let nested = environment_values["json"].is_object() || environment_values["text"].is_object();
    if !nested {
        request["bodyEnvironmentValues"] = json!({ "json": {}, "text": {} });
        if (body_type == "json" || body_type == "text") && environment_values.is_object() {
            request["bodyEnvironmentValues"][&body_type] = environment_values;
        }
    } else {
        if !request["bodyEnvironmentValues"]["json"].is_object() {
            request["bodyEnvironmentValues"]["json"] = json!({});
        }
        if !request["bodyEnvironmentValues"]["text"].is_object() {
            request["bodyEnvironmentValues"]["text"] = json!({});
        }
    }
}

fn delete_request(app: &AppHandle, arguments: &Value) -> Result<Value, String> {
    let request_id = required_string(arguments, "requestId")?;
    let mut deleted = storage::delete_request(app, &request_id)?
        .ok_or_else(|| format!("Request not found: {request_id}"))?;
    let sync_warning = match storage::load_workspace_index(app) {
        Ok(Some(index)) => app
            .emit("workspace-updated", index)
            .err()
            .map(|error| error.to_string()),
        Ok(None) => Some("workspace index was not saved".to_string()),
        Err(error) => Some(error),
    };
    if let Some(warning) = sync_warning {
        deleted["syncWarning"] = Value::String(warning);
    }
    Ok(deleted)
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
