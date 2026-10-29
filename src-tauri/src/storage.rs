use std::{collections::HashSet, fs, path::PathBuf};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const SCHEMA_VERSION: i64 = 3;

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    Ok(directory)
}

pub(crate) fn legacy_workspace_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("workspace.json"))
}

fn database_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app_data_dir(app)?.join("reqly.db"))
}

fn open(app: &AppHandle) -> Result<Connection, String> {
    let connection = Connection::open(database_path(app)?).map_err(|error| error.to_string())?;
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS workspace_state (id INTEGER PRIMARY KEY CHECK (id = 1), data TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS request_details (id TEXT PRIMARY KEY, data TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS request_index (
           id TEXT PRIMARY KEY,
           container_id TEXT NOT NULL,
           position INTEGER NOT NULL,
           name TEXT NOT NULL,
           method TEXT NOT NULL,
           url TEXT NOT NULL,
           archived INTEGER NOT NULL DEFAULT 0,
           pinned INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS idx_request_container ON request_index(container_id, position);
         CREATE INDEX IF NOT EXISTS idx_request_name ON request_index(name);
         CREATE VIRTUAL TABLE IF NOT EXISTS request_search USING fts5(id UNINDEXED, name, url, tokenize='trigram');",
    ).map_err(|error| error.to_string())?;
    Ok(connection)
}

pub(crate) fn initialize(app: &AppHandle) -> Result<(), String> {
    let mut connection = open(app)?;
    let has_state = connection
        .query_row("SELECT 1 FROM workspace_state WHERE id = 1", [], |_| Ok(()))
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    if !has_state {
        let legacy = legacy_workspace_path(app)?;
        if legacy.exists() {
            let backup = legacy.with_file_name("workspace.pre-sqlite-backup.json");
            if !backup.exists() {
                fs::copy(&legacy, &backup)
                    .map_err(|error| format!("cannot back up workspace.json: {error}"))?;
            }
            let workspace: Value = serde_json::from_str(
                &fs::read_to_string(&legacy).map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("cannot migrate workspace.json: {error}"))?;
            save_with_connection(&mut connection, &workspace)?;
        }
    }
    migrate_index(&mut connection)?;
    connection
        .execute(
            "INSERT INTO request_search(id,name,url)
         SELECT i.id,i.name,i.url FROM request_index i
         WHERE NOT EXISTS (SELECT 1 FROM request_search s WHERE s.id=i.id)",
            [],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO metadata(key, value) VALUES('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [SCHEMA_VERSION.to_string()],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn migrate_index(connection: &mut Connection) -> Result<(), String> {
    let version = connection
        .query_row(
            "SELECT value FROM metadata WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0);
    if version >= SCHEMA_VERSION {
        return Ok(());
    }
    let Some(data) = connection
        .query_row("SELECT data FROM workspace_state WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| error.to_string())?
    else {
        return Ok(());
    };
    let mut workspace: Value = serde_json::from_str(&data).map_err(|error| error.to_string())?;
    let embedded_requests = workspace["collections"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|collection| {
            collection["requests"].as_array().map_or(0, Vec::len)
                + collection["groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|group| group["requests"].as_array().map_or(0, Vec::len))
                    .sum::<usize>()
        })
        .sum::<usize>();
    if embedded_requests == 0 {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    extract_all_requests(&transaction, &mut workspace)?;
    write_state(&transaction, &workspace)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub(crate) fn load_workspace_index(app: &AppHandle) -> Result<Option<Value>, String> {
    let connection = open(app)?;
    let data = connection
        .query_row("SELECT data FROM workspace_state WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    data.map(|content| serde_json::from_str(&content).map_err(|error| error.to_string()))
        .transpose()
}

pub(crate) fn load_container_requests(
    app: &AppHandle,
    container_id: &str,
) -> Result<Vec<Value>, String> {
    let connection = open(app)?;
    query_container_requests(&connection, container_id)
}

pub(crate) fn search_requests(
    app: &AppHandle,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>, String> {
    let connection = open(app)?;
    let (sql, pattern) = if query.chars().count() >= 3 {
        (
            "SELECT i.id,i.container_id,i.name,i.method,i.url,i.archived,i.pinned
             FROM request_search s JOIN request_index i ON i.id=s.id
             WHERE request_search MATCH ?1 ORDER BY i.name LIMIT ?2",
            format!("\"{}\"", query.replace('"', "\"\"")),
        )
    } else {
        (
            "SELECT id,container_id,name,method,url,archived,pinned FROM request_index
             WHERE name LIKE ?1 OR url LIKE ?1 ORDER BY name LIMIT ?2",
            format!("%{query}%"),
        )
    };
    let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![pattern, limit as i64], row_to_search_result)
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

pub(crate) fn locate_request(app: &AppHandle, request_id: &str) -> Result<Option<Value>, String> {
    let connection = open(app)?;
    connection.query_row(
        "SELECT id, container_id, name, method, url, archived, pinned FROM request_index WHERE id = ?1",
        [request_id], row_to_search_result,
    ).optional().map_err(|error| error.to_string())
}

pub(crate) fn get_request(app: &AppHandle, request_id: &str) -> Result<Option<Value>, String> {
    let connection = open(app)?;
    let data = connection
        .query_row(
            "SELECT data FROM request_details WHERE id = ?1",
            [request_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    data.map(|content| serde_json::from_str(&content).map_err(|error| error.to_string()))
        .transpose()
}

pub(crate) fn save_request(app: &AppHandle, request: &Value) -> Result<(), String> {
    let id = request["id"].as_str().ok_or("request id is required")?;
    let mut connection = open(app)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute(
        "INSERT INTO request_details(id, data) VALUES(?1, ?2) ON CONFLICT(id) DO UPDATE SET data = excluded.data",
        params![id, serde_json::to_string(request).map_err(|error| error.to_string())?],
    ).map_err(|error| error.to_string())?;
    transaction.execute(
        "UPDATE request_index SET name=?2, method=?3, url=?4, archived=?5, pinned=?6 WHERE id=?1",
        params![id, text(request, "name"), text(request, "method"), text(request, "url"), bool_int(request, "archived"), bool_int(request, "pinned")],
    ).map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM request_search WHERE id=?1", [id])
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO request_search(id,name,url) VALUES(?1,?2,?3)",
            params![id, text(request, "name"), text(request, "url")],
        )
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())
}

pub(crate) fn save_workspace(app: &AppHandle, workspace: &Value) -> Result<(), String> {
    let mut connection = open(app)?;
    save_with_connection(&mut connection, workspace)
}

pub(crate) fn load_workspace_with_summaries(app: &AppHandle) -> Result<Option<Value>, String> {
    let Some(mut workspace) = load_workspace_index(app)? else {
        return Ok(None);
    };
    let connection = open(app)?;
    for_each_container_mut(&mut workspace, |container_id, container| {
        container["requests"] = Value::Array(query_container_requests(&connection, container_id)?);
        container["requestsLoaded"] = Value::Bool(true);
        Ok(())
    })?;
    Ok(Some(workspace))
}

pub(crate) fn import_artifact<F>(
    app: &AppHandle,
    path: &str,
    target_container_id: Option<&str>,
    mut progress: F,
) -> Result<Value, String>
where
    F: FnMut(u8, &str),
{
    progress(5, "正在读取导入文件");
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    progress(20, "正在解析 JSON");
    let payload: Value = serde_json::from_str(&content).map_err(|error| error.to_string())?;
    let format = payload["format"].as_str().ok_or("不支持的文件类型")?;
    let data = payload.get("data").cloned().ok_or("导入文件缺少 data")?;
    let total = artifact_request_count(format, &data).max(1);
    let mut processed = 0usize;
    let mut report = |processed: usize| {
        let percent = 25 + ((processed as f64 / total as f64) * 45.0).round() as u8;
        progress(percent.min(70), "正在生成接口副本");
    };

    let mut connection = open(app)?;
    let mut workspace = load_workspace_index(app)?.ok_or("工作区尚未初始化")?;
    let selected_request_id = match format {
        "reqly-request" => {
            let target_id = target_container_id
                .map(str::to_string)
                .or_else(|| first_container_id(&workspace))
                .ok_or("请先创建一个分组")?;
            let mut request = regenerate_request(data, &mut processed, &mut report)?;
            request["name"] = Value::String(format!("{} 副本", text(&request, "name")));
            let selected_request_id = text(&request, "id");
            let existing = query_container_requests(&connection, &target_id)?;
            let target = find_container_mut(&mut workspace, &target_id).ok_or("目标分组不存在")?;
            target["requests"] = Value::Array(existing.into_iter().chain([request]).collect());
            target["requestsLoaded"] = Value::Bool(true);
            selected_request_id
        }
        "reqly-subgroup" => {
            let parent_id = target_container_id
                .and_then(|id| parent_collection_id(&workspace, id))
                .or_else(|| {
                    workspace["collections"]
                        .as_array()
                        .and_then(|items| items.first())
                        .and_then(|item| item["id"].as_str())
                        .map(str::to_string)
                })
                .ok_or("请先创建一个一级分组")?;
            let group = regenerate_subgroup(data, &mut processed, &mut report)?;
            let selected_request_id = first_request_id(&group).unwrap_or_default();
            let parent = workspace["collections"]
                .as_array_mut()
                .and_then(|items| {
                    items
                        .iter_mut()
                        .find(|item| item["id"].as_str() == Some(parent_id.as_str()))
                })
                .ok_or("目标一级分组不存在")?;
            parent["groups"]
                .as_array_mut()
                .ok_or("一级分组 groups 格式无效")?
                .push(group);
            selected_request_id
        }
        "reqly-collection" => {
            let collection = regenerate_collection(data, &mut processed, &mut report)?;
            let selected_request_id = first_request_id(&collection).unwrap_or_default();
            workspace["collections"]
                .as_array_mut()
                .ok_or("collections 格式无效")?
                .push(collection);
            selected_request_id
        }
        _ => return Err("不支持的文件类型，请选择 Reqly 分组或接口导出文件".into()),
    };

    progress(75, "正在写入 SQLite 数据库");
    save_with_connection(&mut connection, &workspace)?;
    progress(96, "正在刷新分组索引");
    let index = load_workspace_index(app)?.ok_or("无法读取导入后的工作区")?;
    progress(100, "导入完成");
    Ok(json!({ "workspace": index, "selectedRequestId": selected_request_id }))
}

fn artifact_request_count(format: &str, data: &Value) -> usize {
    match format {
        "reqly-request" => 1,
        "reqly-subgroup" => data["requests"].as_array().map_or(0, Vec::len),
        "reqly-collection" => {
            data["requests"].as_array().map_or(0, Vec::len)
                + data["groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|group| group["requests"].as_array().map_or(0, Vec::len))
                    .sum::<usize>()
        }
        _ => 0,
    }
}

fn regenerate_request<F>(
    mut request: Value,
    processed: &mut usize,
    progress: &mut F,
) -> Result<Value, String>
where
    F: FnMut(usize),
{
    if !request.is_object() {
        return Err("接口数据格式无效".into());
    }
    request["id"] = Value::String(Uuid::new_v4().to_string());
    for field in ["params", "headers", "formBody", "formData"] {
        if let Some(items) = request[field].as_array_mut() {
            for item in items {
                item["id"] = Value::String(Uuid::new_v4().to_string());
            }
        }
    }
    *processed += 1;
    if (*processed).is_multiple_of(250) {
        progress(*processed);
    }
    Ok(request)
}

fn regenerate_subgroup<F>(
    mut group: Value,
    processed: &mut usize,
    progress: &mut F,
) -> Result<Value, String>
where
    F: FnMut(usize),
{
    if !group.is_object() {
        return Err("二级分组数据格式无效".into());
    }
    group["id"] = Value::String(Uuid::new_v4().to_string());
    group["name"] = Value::String(format!("{} 副本", text(&group, "name")));
    group["expanded"] = Value::Bool(false);
    group["archiveExpanded"] = Value::Bool(false);
    let requests = group["requests"].as_array().cloned().unwrap_or_default();
    let mut regenerated = Vec::with_capacity(requests.len());
    for request in requests {
        regenerated.push(regenerate_request(request, processed, progress)?);
    }
    group["requestCount"] = Value::from(regenerated.len());
    group["requestsLoaded"] = Value::Bool(true);
    group["requests"] = Value::Array(regenerated);
    Ok(group)
}

fn regenerate_collection<F>(
    mut collection: Value,
    processed: &mut usize,
    progress: &mut F,
) -> Result<Value, String>
where
    F: FnMut(usize),
{
    if !collection.is_object() {
        return Err("分组数据格式无效".into());
    }
    collection["id"] = Value::String(Uuid::new_v4().to_string());
    collection["name"] = Value::String(format!("{} 副本", text(&collection, "name")));
    collection["expanded"] = Value::Bool(true);
    let requests = collection["requests"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut regenerated = Vec::with_capacity(requests.len());
    for request in requests {
        regenerated.push(regenerate_request(request, processed, progress)?);
    }
    collection["requestCount"] = Value::from(regenerated.len());
    collection["requestsLoaded"] = Value::Bool(true);
    collection["requests"] = Value::Array(regenerated);
    let groups = collection["groups"].as_array().cloned().unwrap_or_default();
    let mut regenerated_groups = Vec::with_capacity(groups.len());
    for group in groups {
        regenerated_groups.push(regenerate_subgroup(group, processed, progress)?);
    }
    collection["groups"] = Value::Array(regenerated_groups);
    Ok(collection)
}

fn find_container_mut<'a>(workspace: &'a mut Value, container_id: &str) -> Option<&'a mut Value> {
    for collection in workspace["collections"].as_array_mut()? {
        if collection["id"].as_str() == Some(container_id) {
            return Some(collection);
        }
        if let Some(group) = collection["groups"]
            .as_array_mut()?
            .iter_mut()
            .find(|group| group["id"].as_str() == Some(container_id))
        {
            return Some(group);
        }
    }
    None
}

fn parent_collection_id(workspace: &Value, container_id: &str) -> Option<String> {
    workspace["collections"]
        .as_array()?
        .iter()
        .find(|collection| {
            collection["id"].as_str() == Some(container_id)
                || collection["groups"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|group| group["id"].as_str() == Some(container_id))
        })
        .and_then(|collection| collection["id"].as_str())
        .map(str::to_string)
}

fn first_container_id(workspace: &Value) -> Option<String> {
    workspace["collections"]
        .as_array()?
        .first()
        .and_then(|collection| collection["id"].as_str())
        .map(str::to_string)
}

fn first_request_id(container: &Value) -> Option<String> {
    container["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(
            container["groups"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|group| group["requests"].as_array().into_iter().flatten()),
        )
        .find_map(|request| request["id"].as_str().map(str::to_string))
}

fn save_with_connection(connection: &mut Connection, workspace: &Value) -> Result<(), String> {
    let mut index = workspace.clone();
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let mut valid_containers = HashSet::new();
    for_each_container_mut(&mut index, |container_id, container| {
        valid_containers.insert(container_id.to_string());
        let loaded = container["requestsLoaded"].as_bool().unwrap_or_else(|| {
            container["requests"]
                .as_array()
                .is_some_and(|requests| !requests.is_empty())
        });
        if loaded {
            sync_container(
                &transaction,
                container_id,
                container["requests"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            )?;
        }
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM request_index WHERE container_id=?1",
                [container_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        container["requestCount"] = Value::from(count);
        container["requests"] = json!([]);
        container["requestsLoaded"] = Value::Bool(false);
        Ok(())
    })?;
    let mut statement = transaction
        .prepare("SELECT DISTINCT container_id FROM request_index")
        .map_err(|error| error.to_string())?;
    let containers = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    drop(statement);
    for container_id in containers {
        if !valid_containers.contains(&container_id) {
            transaction.execute(
                "DELETE FROM request_search WHERE id IN (SELECT id FROM request_index WHERE container_id=?1)",
                [&container_id],
            ).map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "DELETE FROM request_index WHERE container_id=?1",
                    [&container_id],
                )
                .map_err(|error| error.to_string())?;
        }
    }
    transaction
        .execute(
            "DELETE FROM request_details WHERE id NOT IN (SELECT id FROM request_index)",
            [],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM request_search WHERE id NOT IN (SELECT id FROM request_index)",
            [],
        )
        .map_err(|error| error.to_string())?;
    write_state(&transaction, &index)?;
    transaction.commit().map_err(|error| error.to_string())
}

fn extract_all_requests(transaction: &Transaction, workspace: &mut Value) -> Result<(), String> {
    for_each_container_mut(workspace, |container_id, container| {
        let requests = container["requests"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        sync_container(transaction, container_id, requests)?;
        let count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM request_index WHERE container_id=?1",
                [container_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        container["requestCount"] = Value::from(count);
        container["requests"] = json!([]);
        container["requestsLoaded"] = Value::Bool(false);
        Ok(())
    })
}

fn sync_container(
    transaction: &Transaction,
    container_id: &str,
    requests: Vec<Value>,
) -> Result<(), String> {
    transaction
        .execute(
            "DELETE FROM request_search WHERE id IN (SELECT id FROM request_index WHERE container_id=?1)",
            [container_id],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "DELETE FROM request_index WHERE container_id=?1",
            [container_id],
        )
        .map_err(|error| error.to_string())?;
    for (position, request) in requests.into_iter().enumerate() {
        let id = request["id"].as_str().ok_or("request id is required")?;
        transaction.execute(
            "INSERT INTO request_index(id, container_id, position, name, method, url, archived, pinned)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET container_id=excluded.container_id, position=excluded.position,
             name=excluded.name, method=excluded.method, url=excluded.url, archived=excluded.archived, pinned=excluded.pinned",
            params![id, container_id, position as i64, text(&request, "name"), text(&request, "method"), text(&request, "url"), bool_int(&request, "archived"), bool_int(&request, "pinned")],
        ).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO request_search(id,name,url) VALUES(?1,?2,?3)",
                params![id, text(&request, "name"), text(&request, "url")],
            )
            .map_err(|error| error.to_string())?;
        if request.get("params").is_some() || request.get("bodyType").is_some() {
            transaction.execute(
                "INSERT INTO request_details(id, data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![id, serde_json::to_string(&request).map_err(|error| error.to_string())?],
            ).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn query_container_requests(
    connection: &Connection,
    container_id: &str,
) -> Result<Vec<Value>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, name, method, url, archived, pinned FROM request_index
         WHERE container_id=?1 ORDER BY pinned DESC, position",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([container_id], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?, "name": row.get::<_, String>(1)?,
                "method": row.get::<_, String>(2)?, "url": row.get::<_, String>(3)?,
                "archived": row.get::<_, i64>(4)? != 0, "pinned": row.get::<_, i64>(5)? != 0
            }))
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn row_to_search_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    Ok(json!({
        "id": row.get::<_, String>(0)?, "containerId": row.get::<_, String>(1)?,
        "name": row.get::<_, String>(2)?, "method": row.get::<_, String>(3)?,
        "url": row.get::<_, String>(4)?, "archived": row.get::<_, i64>(5)? != 0,
        "pinned": row.get::<_, i64>(6)? != 0
    }))
}

fn write_state(transaction: &Transaction, workspace: &Value) -> Result<(), String> {
    transaction.execute(
        "INSERT INTO workspace_state(id,data) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
        [serde_json::to_string(workspace).map_err(|error| error.to_string())?],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn for_each_container_mut<F>(workspace: &mut Value, mut visitor: F) -> Result<(), String>
where
    F: FnMut(&str, &mut Value) -> Result<(), String>,
{
    for collection in workspace["collections"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        let collection_id = collection["id"]
            .as_str()
            .ok_or("collection id is required")?
            .to_string();
        visitor(&collection_id, collection)?;
        for group in collection["groups"].as_array_mut().into_iter().flatten() {
            let group_id = group["id"]
                .as_str()
                .ok_or("group id is required")?
                .to_string();
            visitor(&group_id, group)?;
        }
    }
    Ok(())
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_string()
}
fn bool_int(value: &Value, key: &str) -> i64 {
    i64::from(value[key].as_bool().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_embedded_requests_to_lazy_index() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "CREATE TABLE request_details(id TEXT PRIMARY KEY, data TEXT NOT NULL);
             CREATE TABLE request_index(id TEXT PRIMARY KEY, container_id TEXT NOT NULL, position INTEGER NOT NULL,
             name TEXT NOT NULL, method TEXT NOT NULL, url TEXT NOT NULL, archived INTEGER NOT NULL, pinned INTEGER NOT NULL);
             CREATE VIRTUAL TABLE request_search USING fts5(id UNINDEXED, name, url, tokenize='trigram');"
        ).unwrap();
        let mut workspace = json!({
            "collections": [{
                "id": "collection-1", "name": "API", "groups": [],
                "requests": [{
                    "id": "request-1", "name": "Login", "method": "POST", "url": "https://example.com/login",
                    "archived": false, "pinned": false, "params": [], "headers": [], "bodyType": "json", "body": "{}"
                }]
            }]
        });
        let transaction = connection.transaction().unwrap();
        extract_all_requests(&transaction, &mut workspace).unwrap();
        transaction.commit().unwrap();

        assert_eq!(workspace["collections"][0]["requestCount"], 1);
        assert_eq!(workspace["collections"][0]["requests"], json!([]));
        assert_eq!(workspace["collections"][0]["requestsLoaded"], false);
        let indexed: i64 = connection
            .query_row("SELECT COUNT(*) FROM request_index", [], |row| row.get(0))
            .unwrap();
        let detailed: i64 = connection
            .query_row("SELECT COUNT(*) FROM request_details", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed, 1);
        assert_eq!(detailed, 1);
    }

    #[test]
    fn imported_groups_keep_request_counts() {
        let source = json!({
            "id": "old-collection", "name": "Performance", "requests": [],
            "groups": [{
                "id": "old-group", "name": "Group", "requests": [
                    { "id": "old-1", "name": "One", "method": "GET", "url": "https://example.com/1", "params": [] },
                    { "id": "old-2", "name": "Two", "method": "GET", "url": "https://example.com/2", "params": [] }
                ]
            }]
        });
        let mut processed = 0;
        let imported = regenerate_collection(source, &mut processed, &mut |_| {}).unwrap();
        assert_eq!(processed, 2);
        assert_eq!(imported["groups"][0]["requestCount"], 2);
        assert_eq!(imported["groups"][0]["requestsLoaded"], true);
        assert_eq!(
            imported["groups"][0]["requests"].as_array().unwrap().len(),
            2
        );
        assert_ne!(imported["groups"][0]["requests"][0]["id"], "old-1");
    }
}
