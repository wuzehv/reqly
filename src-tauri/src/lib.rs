use curl::easy::{Easy, Form, InfoType, List, SslOpt};
use http::{HeaderName, HeaderValue, Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, State};

mod mcp;
mod storage;

const RAW_REQUEST_CAPTURE_LIMIT: usize = 1024 * 1024;
static COOKIE_JAR_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

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
    pub(crate) body_file_path: Option<String>,
    #[serde(default)]
    pub(crate) multipart: Vec<MultipartField>,
    #[serde(default)]
    pub(crate) cookie_scope: Option<String>,
    pub(crate) proxy: ProxyConfig,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MultipartField {
    name: String,
    value: String,
    kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseOutput {
    status: u16,
    status_text: String,
    status_line: String,
    raw_request: String,
    duration_ms: u128,
    size: usize,
    request_headers: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: String,
    timings: ResponseTimings,
    connection: ConnectionInfo,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResponseTimings {
    dns_ms: f64,
    tcp_ms: f64,
    ssl_ms: Option<f64>,
    request_ms: f64,
    waiting_ms: f64,
    download_ms: f64,
    total_ms: f64,
    redirect_ms: f64,
    redirect_count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionInfo {
    effective_url: String,
    remote_address: Option<String>,
    local_address: Option<String>,
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn elapsed_between(later: Duration, earlier: Duration) -> f64 {
    duration_ms(later.saturating_sub(earlier))
}

fn request_error(error: curl::Error) -> String {
    format!("请求失败：{error}")
}

fn format_endpoint(address: Option<&str>, port: u16) -> Option<String> {
    address.map(|address| {
        if port == 0 {
            address.to_string()
        } else if address.contains(':') {
            format!("[{address}]:{port}")
        } else {
            format!("{address}:{port}")
        }
    })
}

fn append_capped(target: &mut Vec<u8>, data: &[u8], truncated: &mut bool) {
    let remaining = RAW_REQUEST_CAPTURE_LIMIT.saturating_sub(target.len());
    if remaining == 0 {
        *truncated = true;
        return;
    }
    let length = remaining.min(data.len());
    target.extend_from_slice(&data[..length]);
    if length < data.len() {
        *truncated = true;
    }
}

fn cookie_jar_path(app: &AppHandle, scope: &str) -> Result<PathBuf, String> {
    let file_name = scope
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if file_name.is_empty() {
        return Err("Cookie 作用域无效".into());
    }
    let directory = storage::app_data_dir(app)?.join("cookies");
    fs::create_dir_all(&directory).map_err(|error| format!("创建 Cookie 存储目录失败：{error}"))?;
    Ok(directory.join(format!("{file_name}.txt")))
}

fn cookie_jar_lock(path: &Path) -> Result<Arc<Mutex<()>>, String> {
    let mut locks = COOKIE_JAR_LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| "Cookie 存储锁不可用".to_string())?;
    Ok(locks
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone())
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
fn inspect_artifact_import(app: AppHandle, path: String) -> Result<Option<Value>, String> {
    storage::inspect_artifact_import(&app, &path)
}

#[tauri::command]
async fn import_artifact(
    app: AppHandle,
    path: String,
    target_group_id: Option<String>,
    pre_script_strategy: Option<String>,
) -> Result<Value, String> {
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        storage::import_artifact(
            &app,
            &path,
            target_group_id.as_deref(),
            pre_script_strategy.as_deref(),
            |percent, label| {
                let _ = progress_app.emit(
                    "artifact-import-progress",
                    serde_json::json!({
                        "percent": percent,
                        "label": label,
                    }),
                );
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

pub(crate) async fn execute_request(
    input: RequestInput,
    cookie_path: Option<PathBuf>,
) -> Result<ResponseOutput, String> {
    tauri::async_runtime::spawn_blocking(move || execute_request_blocking(input, cookie_path))
        .await
        .map_err(|error| error.to_string())?
}

fn execute_request_blocking(
    input: RequestInput,
    cookie_path: Option<PathBuf>,
) -> Result<ResponseOutput, String> {
    let method = Method::from_bytes(input.method.as_bytes()).map_err(|error| error.to_string())?;
    let cookie_lock = cookie_path.as_deref().map(cookie_jar_lock).transpose()?;
    let _cookie_guard = cookie_lock
        .as_deref()
        .map(|lock| lock.lock().map_err(|_| "Cookie 存储锁不可用".to_string()))
        .transpose()?;
    let mut easy = Easy::new();
    easy.url(input.url.trim()).map_err(request_error)?;
    easy.follow_location(true).map_err(request_error)?;
    easy.max_redirections(10).map_err(request_error)?;
    easy.accept_encoding("").map_err(request_error)?;
    easy.verbose(true).map_err(request_error)?;
    if let Some(path) = cookie_path.as_deref() {
        easy.cookie_file(path).map_err(request_error)?;
        easy.cookie_jar(path).map_err(request_error)?;
    }

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
        easy.proxy(&proxy_url)
            .map_err(|error| format!("代理地址无效（{proxy_url}）：{error}"))?;
        let mut ssl_options = SslOpt::new();
        ssl_options.revoke_best_effort(true);
        easy.ssl_options(&ssl_options).map_err(request_error)?;
        easy.proxy_ssl_options(&ssl_options)
            .map_err(request_error)?;
        if !input.proxy.username.is_empty() {
            easy.proxy_username(&input.proxy.username)
                .map_err(request_error)?;
            easy.proxy_password(&input.proxy.password)
                .map_err(request_error)?;
        }
    }

    let mut header_list = List::new();
    for item in input
        .headers
        .iter()
        .filter(|item| item.enabled && !item.key.trim().is_empty())
    {
        let name = HeaderName::from_bytes(item.key.trim().as_bytes())
            .map_err(|error| format!("请求头 {} 无效: {error}", item.key))?;
        let value = HeaderValue::from_str(&item.value)
            .map_err(|error| format!("请求头 {} 的值无效: {error}", item.key))?;
        let value = value
            .to_str()
            .map_err(|error| format!("请求头 {} 的值无效: {error}", item.key))?;
        header_list
            .append(&format!("{name}: {value}"))
            .map_err(request_error)?;
    }
    easy.http_headers(header_list).map_err(request_error)?;

    let body_bytes = if let Some(path) = input
        .body_file_path
        .as_deref()
        .filter(|path| !path.trim().is_empty())
    {
        Some(fs::read(path).map_err(|error| format!("读取文件失败（{path}）：{error}"))?)
    } else if !input.body.is_empty() {
        Some(input.body.into_bytes())
    } else {
        None
    };
    let mut multipart_form = None;
    if !input.multipart.is_empty() {
        let mut form = Form::new();
        for field in &input.multipart {
            let mut part = form.part(&field.name);
            let mime = (field.kind == "file")
                .then(|| mime_guess::from_path(field.value.trim()).first_or_octet_stream());
            if field.kind == "file" {
                let path = field.value.trim();
                if path.is_empty() {
                    return Err(format!("文件字段 {} 未选择文件", field.name));
                }
                let file_name = Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("file");
                part.file(path)
                    .filename(file_name)
                    .content_type(mime.as_ref().expect("file MIME is initialized").as_ref());
            } else {
                part.contents(field.value.as_bytes());
            }
            part.add()
                .map_err(|error| format!("构建表单字段 {} 失败：{error}", field.name))?;
        }
        multipart_form = Some(form);
    }
    let has_multipart = multipart_form.is_some();
    let has_body = body_bytes.is_some();
    if let Some(form) = multipart_form {
        easy.httppost(form).map_err(request_error)?;
    } else if let Some(body) = body_bytes.as_deref() {
        easy.post_fields_copy(body).map_err(request_error)?;
    }
    if method == Method::GET && !has_multipart && !has_body {
        easy.get(true).map_err(request_error)?;
    } else if method == Method::HEAD && !has_multipart && !has_body {
        easy.nobody(true).map_err(request_error)?;
    } else if method == Method::POST {
        if !has_multipart && !has_body {
            easy.post(true).map_err(request_error)?;
        }
    } else {
        easy.custom_request(method.as_str())
            .map_err(request_error)?;
    }

    let mut response_body = Vec::new();
    let mut request_headers = Vec::new();
    let mut response_headers = Vec::new();
    let mut status_text = String::new();
    let mut status_line = String::new();
    let mut raw_request = Vec::new();
    let mut raw_request_truncated = false;
    {
        let mut transfer = easy.transfer();
        transfer
            .debug_function(|kind, data| match kind {
                InfoType::HeaderOut => {
                    append_capped(&mut raw_request, data, &mut raw_request_truncated);
                    for line in String::from_utf8_lossy(data).lines() {
                        let line = line.trim_end_matches('\r');
                        if line.rsplit_once(" HTTP/").is_some() {
                            request_headers.clear();
                        } else if let Some(colon) = line.find(':') {
                            request_headers.push((
                                line[..colon].trim().to_string(),
                                line[colon + 1..].trim().to_string(),
                            ));
                        }
                    }
                }
                InfoType::DataOut => {
                    append_capped(&mut raw_request, data, &mut raw_request_truncated);
                }
                _ => {}
            })
            .map_err(request_error)?;
        transfer
            .write_function(|data| {
                response_body.extend_from_slice(data);
                Ok(data.len())
            })
            .map_err(request_error)?;
        transfer
            .header_function(|line| {
                if line.starts_with(b"HTTP/") {
                    response_headers.clear();
                    status_line = String::from_utf8_lossy(line).trim().to_string();
                    status_text = status_line
                        .trim()
                        .splitn(3, ' ')
                        .nth(2)
                        .unwrap_or("")
                        .to_string();
                } else if let Some(colon) = line.iter().position(|byte| *byte == b':') {
                    let name = String::from_utf8_lossy(&line[..colon]).trim().to_string();
                    let value = String::from_utf8_lossy(&line[colon + 1..])
                        .trim()
                        .to_string();
                    response_headers.push((name, value));
                }
                true
            })
            .map_err(request_error)?;
        transfer.perform().map_err(request_error)?;
    }

    let namelookup = easy.namelookup_time().map_err(request_error)?;
    let connect = easy.connect_time().map_err(request_error)?;
    let appconnect = easy.appconnect_time().map_err(request_error)?;
    let pretransfer = easy.pretransfer_time().map_err(request_error)?;
    let starttransfer = easy.starttransfer_time().map_err(request_error)?;
    let total = easy.total_time().map_err(request_error)?;
    let redirect_count = easy.redirect_count().map_err(request_error)?;
    let redirect_time = easy.redirect_time().map_err(request_error)?;
    let secure = input
        .url
        .trim()
        .to_ascii_lowercase()
        .starts_with("https://");
    let connection_ready = if secure && !appconnect.is_zero() {
        appconnect
    } else {
        connect
    };
    let remote_address = format_endpoint(
        easy.primary_ip().map_err(request_error)?,
        easy.primary_port().map_err(request_error)?,
    );
    let local_address = format_endpoint(
        easy.local_ip().map_err(request_error)?,
        easy.local_port().map_err(request_error)?,
    );
    let effective_url = easy
        .effective_url()
        .map_err(request_error)?
        .unwrap_or(input.url.trim())
        .to_string();
    let status = easy.response_code().map_err(request_error)?;
    if status_text.is_empty() {
        status_text = u16::try_from(status)
            .ok()
            .and_then(|status| StatusCode::from_u16(status).ok())
            .and_then(|status| status.canonical_reason())
            .unwrap_or("")
            .to_string();
    }
    if status_line.is_empty() {
        status_line = format!("HTTP {status} {status_text}")
            .trim_end()
            .to_string();
    }
    let total_ms = duration_ms(total);
    let size = response_body.len();
    let mut raw_request = String::from_utf8_lossy(&raw_request).into_owned();
    if raw_request_truncated {
        raw_request.push_str("\r\n[请求报文已在 1 MiB 处截断]");
    }

    Ok(ResponseOutput {
        status: u16::try_from(status).unwrap_or(u16::MAX),
        status_text,
        status_line,
        raw_request,
        duration_ms: total_ms.round() as u128,
        size,
        request_headers,
        headers: response_headers,
        body: String::from_utf8_lossy(&response_body).into_owned(),
        timings: ResponseTimings {
            dns_ms: duration_ms(namelookup),
            tcp_ms: elapsed_between(connect, namelookup),
            ssl_ms: secure.then(|| elapsed_between(appconnect, connect)),
            request_ms: elapsed_between(pretransfer, connection_ready),
            waiting_ms: elapsed_between(starttransfer, pretransfer),
            download_ms: elapsed_between(total, starttransfer),
            total_ms,
            redirect_ms: duration_ms(redirect_time),
            redirect_count,
        },
        connection: ConnectionInfo {
            effective_url,
            remote_address,
            local_address,
        },
    })
}

#[tauri::command]
async fn send_request(app: AppHandle, input: RequestInput) -> Result<ResponseOutput, String> {
    let cookie_path = input
        .cookie_scope
        .as_deref()
        .filter(|scope| !scope.trim().is_empty())
        .map(|scope| cookie_jar_path(&app, scope))
        .transpose()?;
    execute_request(input, cookie_path).await
}

#[tauri::command]
fn clear_cookie_scope(app: AppHandle, scope: String) -> Result<(), String> {
    let path = cookie_jar_path(&app, scope.trim())?;
    let lock = cookie_jar_lock(&path)?;
    let _guard = lock.lock().map_err(|_| "Cookie 存储锁不可用".to_string())?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清空 Cookie 失败：{error}")),
    }
}

#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            clear_cookie_scope,
            configure_mcp,
            get_mcp_status,
            complete_mcp_request,
            get_mcp_addresses,
            write_export_file,
            inspect_artifact_import,
            import_artifact
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
