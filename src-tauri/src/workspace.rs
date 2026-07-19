use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::StreamExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::config::{app_dir, managed_skills_dir, path_for, read_json, skill_backups_dir, workspace_path, write_json, write_text};
use crate::error::AppError;

const WORKSPACE_SCHEMA_VERSION: u32 = 1;
const SUPPORTED_APPS: [&str; 6] = ["codex", "claude", "gemini", "opencode", "openclaw", "hermes"];
const MAX_ITEMS: usize = 200;
const MAX_SKILL_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
const MAX_SKILL_ARCHIVE_ENTRIES: usize = 4_096;
const MAX_SKILL_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_SKILL_EXTRACTED_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
  pub id: String,
  pub name: String,
  #[serde(default = "default_stdio")]
  pub transport: String,
  #[serde(default)]
  pub command: String,
  #[serde(default)]
  pub args: Vec<String>,
  #[serde(default)]
  pub env: HashMap<String, String>,
  #[serde(default)]
  pub url: String,
  #[serde(default)]
  pub enabled_apps: Vec<String>,
  #[serde(default)]
  pub updated_at: u64,
}

fn default_stdio() -> String {
  "stdio".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PromptPreset {
  pub id: String,
  pub name: String,
  pub content: String,
  #[serde(default)]
  pub enabled_apps: Vec<String>,
  #[serde(default)]
  pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SkillPackage {
  pub id: String,
  pub name: String,
  #[serde(default)]
  pub description: String,
  #[serde(default)]
  pub source: String,
  #[serde(default)]
  pub enabled_apps: Vec<String>,
  #[serde(default)]
  pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSnapshot {
  pub id: String,
  pub name: String,
  pub profile_id: Option<String>,
  #[serde(default)]
  pub mcp_apps: HashMap<String, Vec<String>>,
  #[serde(default)]
  pub prompt_apps: HashMap<String, Vec<String>>,
  #[serde(default)]
  pub skill_apps: HashMap<String, Vec<String>>,
  #[serde(default)]
  pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEndpoint {
  pub id: String,
  pub name: String,
  pub base_url: String,
  #[serde(default)]
  pub priority: u32,
  #[serde(default = "default_true")]
  pub enabled: bool,
}

fn default_true() -> bool {
  true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySettings {
  #[serde(default = "default_true")]
  pub auto_failover: bool,
  #[serde(default = "default_request_timeout")]
  pub request_timeout_ms: u64,
  #[serde(default = "default_connect_timeout")]
  pub connect_timeout_ms: u64,
  #[serde(default = "default_retry_count")]
  pub max_retries: u32,
  #[serde(default = "default_circuit_failures")]
  pub circuit_failure_threshold: u32,
  #[serde(default = "default_circuit_seconds")]
  pub circuit_open_seconds: u64,
  /// Token required by clients connecting to the local proxy.
  ///
  /// It is loaded from a private app-owned file and never returned to the
  /// renderer as part of WorkspaceData.
  #[serde(default, skip_serializing)]
  pub local_proxy_token: String,
  #[serde(default)]
  pub endpoints: Vec<ProxyEndpoint>,
}

fn default_request_timeout() -> u64 {
  120_000
}
fn default_connect_timeout() -> u64 {
  8_000
}
fn default_retry_count() -> u32 {
  2
}
fn default_circuit_failures() -> u32 {
  3
}
fn default_circuit_seconds() -> u64 {
  30
}

fn default_local_proxy_token() -> String {
  let mut bytes = [0_u8; 32];
  rand::thread_rng().fill_bytes(&mut bytes);
  URL_SAFE_NO_PAD.encode(bytes)
}

fn load_or_create_local_proxy_token() -> Result<String, AppError> {
  let path = app_dir().join("local-proxy.token");
  if path.exists() {
    let token = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    let token = token.trim().to_string();
    if token.len() < 32 || token.len() > 256 || !token.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')) {
      return Err(AppError::Message("local proxy token file is invalid".into()));
    }
    return Ok(token);
  }
  let token = default_local_proxy_token();
  write_text(&path, &token)?;
  Ok(token)
}

impl Default for ProxySettings {
  fn default() -> Self {
    Self {
      auto_failover: true,
      request_timeout_ms: default_request_timeout(),
      connect_timeout_ms: default_connect_timeout(),
      max_retries: default_retry_count(),
      circuit_failure_threshold: default_circuit_failures(),
      circuit_open_seconds: default_circuit_seconds(),
      local_proxy_token: default_local_proxy_token(),
      endpoints: Vec::new(),
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
  pub model: String,
  #[serde(default)]
  pub input_per_million: f64,
  #[serde(default)]
  pub output_per_million: f64,
  #[serde(default)]
  pub cached_input_per_million: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceData {
  #[serde(default = "workspace_schema_version")]
  pub schema_version: u32,
  #[serde(default)]
  pub mcp_servers: Vec<McpServer>,
  #[serde(default)]
  pub prompts: Vec<PromptPreset>,
  #[serde(default)]
  pub skills: Vec<SkillPackage>,
  #[serde(default)]
  pub projects: Vec<ProjectSnapshot>,
  pub active_project_id: Option<String>,
  #[serde(default)]
  pub proxy_settings: ProxySettings,
  #[serde(default)]
  pub model_prices: Vec<ModelPrice>,
}

impl Default for WorkspaceData {
  fn default() -> Self {
    Self {
      schema_version: WORKSPACE_SCHEMA_VERSION,
      mcp_servers: Vec::new(),
      prompts: Vec::new(),
      skills: Vec::new(),
      projects: Vec::new(),
      active_project_id: None,
      proxy_settings: ProxySettings::default(),
      model_prices: default_model_prices(),
    }
  }
}

fn workspace_schema_version() -> u32 {
  WORKSPACE_SCHEMA_VERSION
}

fn default_model_prices() -> Vec<ModelPrice> {
  vec![
    ModelPrice {
      model: "gpt-5.6-sol".into(),
      input_per_million: 5.0,
      output_per_million: 30.0,
      cached_input_per_million: 0.5,
    },
    ModelPrice {
      model: "gpt-5.6-terra".into(),
      input_per_million: 2.5,
      output_per_million: 15.0,
      cached_input_per_million: 0.25,
    },
    ModelPrice {
      model: "gpt-5.6-luna".into(),
      input_per_million: 1.0,
      output_per_million: 6.0,
      cached_input_per_million: 0.1,
    },
  ]
}

fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or(0)
}

fn normalize_id(value: &str) -> Result<String, AppError> {
  let id = value.trim().to_ascii_lowercase().replace(' ', "-");
  if id.is_empty() || id.len() > 80 || !id.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')) {
    return Err(AppError::Message("ID must contain only letters, numbers, dash, underscore, or dot".into()));
  }
  Ok(id)
}

fn normalize_apps(apps: Vec<String>) -> Result<Vec<String>, AppError> {
  let mut result = Vec::new();
  for app in apps {
    let app = app.trim().to_ascii_lowercase();
    if !SUPPORTED_APPS.contains(&app.as_str()) {
      return Err(AppError::Message(format!("unsupported application: {app}")));
    }
    if !result.contains(&app) {
      result.push(app);
    }
  }
  Ok(result)
}

pub fn load_workspace() -> Result<WorkspaceData, AppError> {
  let path = workspace_path();
  if !path.exists() {
    return Ok(WorkspaceData::default());
  }
  let mut data: WorkspaceData = read_json(&path)?;
  if data.schema_version > WORKSPACE_SCHEMA_VERSION {
    return Err(AppError::Message(format!("workspace schema {} is newer than supported schema {WORKSPACE_SCHEMA_VERSION}", data.schema_version)));
  }
  data.schema_version = WORKSPACE_SCHEMA_VERSION;
  if data.model_prices.is_empty() {
    data.model_prices = default_model_prices();
  }
  Ok(data)
}

pub fn validate_workspace_data(data: &WorkspaceData) -> Result<(), AppError> {
  if data.schema_version > WORKSPACE_SCHEMA_VERSION {
    return Err(AppError::Message(format!("workspace schema {} is newer than supported schema {WORKSPACE_SCHEMA_VERSION}", data.schema_version)));
  }
  if data.mcp_servers.len() > MAX_ITEMS || data.prompts.len() > MAX_ITEMS || data.skills.len() > MAX_ITEMS || data.projects.len() > MAX_ITEMS {
    return Err(AppError::Message("workspace item limit exceeded".into()));
  }
  for server in &data.mcp_servers {
    if normalize_id(&server.id)? != server.id || normalize_apps(server.enabled_apps.clone())? != server.enabled_apps {
      return Err(AppError::Message("workspace contains an invalid MCP identifier or application list".into()));
    }
  }
  for prompt in &data.prompts {
    if normalize_id(&prompt.id)? != prompt.id || normalize_apps(prompt.enabled_apps.clone())? != prompt.enabled_apps {
      return Err(AppError::Message("workspace contains an invalid prompt identifier or application list".into()));
    }
  }
  for skill in &data.skills {
    if normalize_id(&skill.id)? != skill.id || normalize_apps(skill.enabled_apps.clone())? != skill.enabled_apps {
      return Err(AppError::Message("workspace contains an invalid skill identifier or application list".into()));
    }
  }
  for project in &data.projects {
    if normalize_id(&project.id)? != project.id {
      return Err(AppError::Message("workspace contains an invalid project identifier".into()));
    }
  }
  Ok(())
}

pub fn save_workspace(data: &WorkspaceData) -> Result<(), AppError> {
  validate_workspace_data(data)?;
  write_json(&workspace_path(), data)
}

pub fn load_proxy_settings_checked() -> Result<ProxySettings, AppError> {
  let mut data = load_workspace()?;
  data.proxy_settings.local_proxy_token = load_or_create_local_proxy_token()?;
  Ok(data.proxy_settings)
}

pub fn load_proxy_settings() -> ProxySettings {
  load_proxy_settings_checked().unwrap_or_default()
}

pub fn load_model_prices() -> Vec<ModelPrice> {
  load_workspace().map(|data| data.model_prices).unwrap_or_else(|_| default_model_prices())
}

#[tauri::command]
pub fn app_get_workspace() -> Result<WorkspaceData, String> {
  load_workspace().map_err(|error| error.to_string())
}

fn validate_mcp(mut server: McpServer) -> Result<McpServer, AppError> {
  server.id = normalize_id(&server.id)?;
  server.name = server.name.trim().to_string();
  server.transport = server.transport.trim().to_ascii_lowercase();
  server.enabled_apps = normalize_apps(server.enabled_apps)?;
  server.updated_at = now_ms();
  if server.name.is_empty() || server.name.len() > 120 {
    return Err(AppError::Message("MCP name is required".into()));
  }
  match server.transport.as_str() {
    "stdio" if !server.command.trim().is_empty() => {
      server.command = server.command.trim().to_string();
      server.url.clear();
    }
    "http" | "sse" if server.url.starts_with("http://") || server.url.starts_with("https://") => {
      server.command.clear();
      server.args.clear();
    }
    _ => return Err(AppError::Message("MCP requires a command for stdio or an HTTP(S) URL".into())),
  }
  Ok(server)
}

#[tauri::command]
pub fn app_save_mcp_server(server: McpServer) -> Result<WorkspaceData, String> {
  let server = validate_mcp(server).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  if let Some(existing) = data.mcp_servers.iter_mut().find(|item| item.id == server.id) {
    *existing = server;
  } else {
    data.mcp_servers.push(server);
  }
  sync_all_mcp(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_delete_mcp_server(id: String) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  for app in SUPPORTED_APPS {
    remove_mcp_from_app(app, &id).map_err(|error| error.to_string())?;
  }
  data.mcp_servers.retain(|item| item.id != id);
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_sync_mcp_servers() -> Result<WorkspaceData, String> {
  let data = load_workspace().map_err(|error| error.to_string())?;
  sync_all_mcp(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_import_mcp_from_app(app: String) -> Result<WorkspaceData, String> {
  let app = normalize_apps(vec![app]).map_err(|error| error.to_string())?.remove(0);
  let imported = read_mcp_from_app(&app).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  for mut server in imported {
    server.enabled_apps = vec![app.clone()];
    if let Some(existing) = data.mcp_servers.iter_mut().find(|item| item.id == server.id) {
      if !existing.enabled_apps.contains(&app) {
        existing.enabled_apps.push(app.clone());
      }
    } else {
      data.mcp_servers.push(server);
    }
  }
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

fn sync_all_mcp(data: &WorkspaceData) -> Result<(), AppError> {
  for app in SUPPORTED_APPS {
    sync_mcp_app(app, data)?;
  }
  Ok(())
}

pub fn sync_workspace_extensions(data: &WorkspaceData) -> Result<(), AppError> {
  sync_all_mcp(data)?;
  sync_all_prompts(data)?;
  sync_all_skills(data)
}

fn mcp_json_value(server: &McpServer) -> Value {
  if server.transport == "stdio" {
    json!({ "command": server.command, "args": server.args, "env": server.env })
  } else {
    json!({ "type": server.transport, "url": server.url })
  }
}

fn json_config(path: &Path) -> Result<Value, AppError> {
  if !path.exists() {
    return Ok(json!({}));
  }
  let content = fs::read_to_string(path).map_err(|error| AppError::io(path, error))?;
  serde_json::from_str(&content).map_err(|error| AppError::json(path, error))
}

fn ensure_object<'a>(value: &'a mut Value, key: &str) -> &'a mut Map<String, Value> {
  if !value.get(key).map(Value::is_object).unwrap_or(false) {
    value[key] = json!({});
  }
  value.get_mut(key).and_then(Value::as_object_mut).expect("object inserted above")
}

fn json_mcp_path_and_key(app: &str) -> Option<(PathBuf, &'static str)> {
  match app {
    "claude" => Some((path_for("claude", "settings.json"), "mcpServers")),
    "gemini" => Some((path_for("gemini", "settings.json"), "mcpServers")),
    "opencode" => Some((path_for("opencode", "opencode.json"), "mcp")),
    "openclaw" => Some((path_for("openclaw", "openclaw.json"), "mcpServers")),
    _ => None,
  }
}

fn set_toml_value_in_place(table: &mut dyn toml_edit::TableLike, key: &str, mut replacement: toml_edit::Value) {
  if let Some(existing) = table.get(key).and_then(Item::as_value) {
    *replacement.decor_mut() = existing.decor().clone();
  }
  if let Some(existing) = table.get_mut(key) {
    *existing = Item::Value(replacement);
  } else {
    table.insert(key, Item::Value(replacement));
  }
}

fn set_toml_array_in_place(table: &mut dyn toml_edit::TableLike, key: &str, values: &[String]) {
  if let Some(array) = table.get_mut(key).and_then(Item::as_value_mut).and_then(toml_edit::Value::as_array_mut) {
    let common_len = array.len().min(values.len());
    for (index, value_text) in values.iter().take(common_len).enumerate() {
      array.replace(index, value_text.as_str());
    }
    while array.len() > values.len() {
      array.remove(array.len() - 1);
    }
    for value_text in values.iter().skip(common_len) {
      array.push(value_text.as_str());
    }
    return;
  }

  let mut array = Array::new();
  for value_text in values {
    array.push(value_text.as_str());
  }
  set_toml_value_in_place(table, key, toml_edit::Value::Array(array));
}

fn sync_toml_env_in_place(entry: &mut dyn toml_edit::TableLike, desired: &HashMap<String, String>) {
  if desired.is_empty() {
    entry.remove("env");
    return;
  }

  if entry.get("env").and_then(Item::as_table_like).is_none() {
    let mut env = Table::new();
    let mut keys = desired.keys().collect::<Vec<_>>();
    keys.sort_unstable();
    for key in keys {
      env.insert(key, value(desired[key].clone()));
    }
    if let Some(existing) = entry.get_mut("env") {
      *existing = Item::Table(env);
    } else {
      entry.insert("env", Item::Table(env));
    }
    return;
  }

  let env = entry.get_mut("env").and_then(Item::as_table_like_mut).expect("env table checked above");
  let existing_keys = env.iter().map(|(key, _)| key.to_string()).collect::<Vec<_>>();
  for key in &existing_keys {
    if !desired.contains_key(key) {
      env.remove(key);
    }
  }
  for key in existing_keys {
    if let Some(value_text) = desired.get(&key) {
      set_toml_value_in_place(env, &key, toml_edit::Value::from(value_text.clone()));
    }
  }
  let mut new_keys = desired.keys().filter(|key| !env.contains_key(key.as_str())).collect::<Vec<_>>();
  new_keys.sort_unstable();
  for key in new_keys {
    env.insert(key, value(desired[key].clone()));
  }
}

fn sync_mcp_app(app: &str, data: &WorkspaceData) -> Result<(), AppError> {
  if app == "codex" {
    let path = path_for("codex", "config.toml");
    let content = if path.exists() { fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))? } else { String::new() };
    let mut document = content.parse::<DocumentMut>().map_err(|error| AppError::Message(format!("{} parse failed: {error}", path.display())))?;
    if !document.as_table().contains_key("mcp_servers") {
      document["mcp_servers"] = Item::Table(Table::new());
    }
    let table = document["mcp_servers"].as_table_like_mut().ok_or_else(|| AppError::Message("Codex mcp_servers must be a table".into()))?;
    for server in &data.mcp_servers {
      if !server.enabled_apps.iter().any(|item| item == app) {
        table.remove(&server.id);
        continue;
      }

      if table.get(&server.id).and_then(Item::as_table_like).is_none() {
        if let Some(existing) = table.get_mut(&server.id) {
          *existing = Item::Table(Table::new());
        } else {
          table.insert(&server.id, Item::Table(Table::new()));
        }
      }
      let entry = table.get_mut(&server.id).and_then(Item::as_table_like_mut).expect("MCP table inserted above");
      if server.transport == "stdio" {
        entry.remove("url");
        entry.remove("type");
        set_toml_value_in_place(entry, "command", toml_edit::Value::from(server.command.clone()));
        set_toml_array_in_place(entry, "args", &server.args);
        sync_toml_env_in_place(entry, &server.env);
      } else {
        for key in ["command", "args", "env", "type"] {
          entry.remove(key);
        }
        set_toml_value_in_place(entry, "url", toml_edit::Value::from(server.url.clone()));
      }
    }
    return write_text(&path, &document.to_string());
  }

  if app == "hermes" {
    let path = path_for("hermes", "config.yaml");
    let mut root: serde_yaml::Value = if path.exists() {
      serde_yaml::from_str(&fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?).map_err(|error| AppError::Message(format!("{} parse failed: {error}", path.display())))?
    } else {
      serde_yaml::Value::Mapping(Default::default())
    };
    let root_map = root.as_mapping_mut().ok_or_else(|| AppError::Message("Hermes config root must be a mapping".into()))?;
    let key = serde_yaml::Value::String("mcp_servers".into());
    let mut servers = root_map.remove(&key).and_then(|value| value.as_mapping().cloned()).unwrap_or_default();
    for server in &data.mcp_servers {
      let id = serde_yaml::Value::String(server.id.clone());
      servers.remove(&id);
      if server.enabled_apps.iter().any(|item| item == app) {
        servers.insert(id, serde_yaml::to_value(mcp_json_value(server)).map_err(|error| AppError::Message(error.to_string()))?);
      }
    }
    root_map.insert(key, serde_yaml::Value::Mapping(servers));
    return write_text(&path, &serde_yaml::to_string(&root).map_err(|error| AppError::Message(error.to_string()))?);
  }

  if let Some((path, key)) = json_mcp_path_and_key(app) {
    let mut root = json_config(&path)?;
    let table = ensure_object(&mut root, key);
    for server in &data.mcp_servers {
      table.remove(&server.id);
      if server.enabled_apps.iter().any(|item| item == app) {
        table.insert(server.id.clone(), mcp_json_value(server));
      }
    }
    return write_json(&path, &root);
  }
  Ok(())
}

fn remove_mcp_from_app(app: &str, id: &str) -> Result<(), AppError> {
  if app == "codex" {
    let path = path_for("codex", "config.toml");
    if !path.exists() {
      return Ok(());
    }
    let content = fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?;
    let mut document = content.parse::<DocumentMut>().map_err(|error| AppError::Message(error.to_string()))?;
    if let Some(table) = document.get_mut("mcp_servers").and_then(Item::as_table_mut) {
      table.remove(id);
    }
    return write_text(&path, &document.to_string());
  }
  if app == "hermes" {
    let path = path_for("hermes", "config.yaml");
    if !path.exists() {
      return Ok(());
    }
    let mut root: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?).map_err(|error| AppError::Message(error.to_string()))?;
    if let Some(table) = root.get_mut("mcp_servers").and_then(serde_yaml::Value::as_mapping_mut) {
      table.remove(serde_yaml::Value::String(id.into()));
    }
    return write_text(&path, &serde_yaml::to_string(&root).map_err(|error| AppError::Message(error.to_string()))?);
  }
  if let Some((path, key)) = json_mcp_path_and_key(app) {
    if !path.exists() {
      return Ok(());
    }
    let mut root = json_config(&path)?;
    if let Some(table) = root.get_mut(key).and_then(Value::as_object_mut) {
      table.remove(id);
    }
    return write_json(&path, &root);
  }
  Ok(())
}

fn server_from_value(id: &str, value: &Value, app: &str) -> Option<McpServer> {
  let command = value.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
  let url = value.get("url").and_then(Value::as_str).unwrap_or_default().to_string();
  if command.is_empty() && url.is_empty() {
    return None;
  }
  Some(McpServer {
    id: normalize_id(id).ok()?,
    name: id.into(),
    transport: if command.is_empty() { value.get("type").and_then(Value::as_str).unwrap_or("http").into() } else { "stdio".into() },
    command,
    args: value
      .get("args")
      .and_then(Value::as_array)
      .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
      .unwrap_or_default(),
    env: value
      .get("env")
      .and_then(Value::as_object)
      .map(|map| map.iter().filter_map(|(key, value)| value.as_str().map(|text| (key.clone(), text.to_string()))).collect())
      .unwrap_or_default(),
    url,
    enabled_apps: vec![app.into()],
    updated_at: now_ms(),
  })
}

fn read_mcp_from_app(app: &str) -> Result<Vec<McpServer>, AppError> {
  let mut result = Vec::new();
  if app == "codex" {
    let path = path_for("codex", "config.toml");
    if !path.exists() {
      return Ok(result);
    }
    let value: toml::Value = toml::from_str(&fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?).map_err(|error| AppError::Message(error.to_string()))?;
    if let Some(table) = value.get("mcp_servers").and_then(toml::Value::as_table) {
      for (id, entry) in table {
        let json = serde_json::to_value(entry).map_err(|error| AppError::Message(error.to_string()))?;
        if let Some(server) = server_from_value(id, &json, app) {
          result.push(server);
        }
      }
    }
    return Ok(result);
  }
  if app == "hermes" {
    let path = path_for("hermes", "config.yaml");
    if !path.exists() {
      return Ok(result);
    }
    let root: serde_yaml::Value = serde_yaml::from_str(&fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))?).map_err(|error| AppError::Message(error.to_string()))?;
    if let Some(table) = root.get("mcp_servers").and_then(serde_yaml::Value::as_mapping) {
      for (id, entry) in table {
        let Some(id) = id.as_str() else {
          continue;
        };
        let json = serde_json::to_value(entry).map_err(|error| AppError::Message(error.to_string()))?;
        if let Some(server) = server_from_value(id, &json, app) {
          result.push(server);
        }
      }
    }
    return Ok(result);
  }
  if let Some((path, key)) = json_mcp_path_and_key(app) {
    if !path.exists() {
      return Ok(result);
    }
    if let Some(table) = json_config(&path)?.get(key).and_then(Value::as_object) {
      for (id, entry) in table {
        if let Some(server) = server_from_value(id, entry, app) {
          result.push(server);
        }
      }
    }
  }
  Ok(result)
}

fn validate_prompt(mut prompt: PromptPreset) -> Result<PromptPreset, AppError> {
  prompt.id = normalize_id(&prompt.id)?;
  prompt.name = prompt.name.trim().to_string();
  prompt.enabled_apps = normalize_apps(prompt.enabled_apps)?;
  prompt.updated_at = now_ms();
  if prompt.name.is_empty() || prompt.content.trim().is_empty() || prompt.content.len() > 1_000_000 {
    return Err(AppError::Message("Prompt name and content are required".into()));
  }
  Ok(prompt)
}

#[tauri::command]
pub fn app_save_prompt(prompt: PromptPreset) -> Result<WorkspaceData, String> {
  let prompt = validate_prompt(prompt).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  if let Some(existing) = data.prompts.iter_mut().find(|item| item.id == prompt.id) {
    *existing = prompt;
  } else {
    data.prompts.push(prompt);
  }
  sync_all_prompts(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_delete_prompt(id: String) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  data.prompts.retain(|item| item.id != id);
  sync_all_prompts(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

fn prompt_path(app: &str) -> PathBuf {
  match app {
    "codex" => path_for("codex", "AGENTS.md"),
    "claude" => path_for("claude", "CLAUDE.md"),
    "gemini" => path_for("gemini", "GEMINI.md"),
    "opencode" => path_for("opencode", "AGENTS.md"),
    "openclaw" => path_for("openclaw", "AGENTS.md"),
    "hermes" => path_for("hermes", "AGENTS.md"),
    _ => app_dir().join("unsupported-prompt.md"),
  }
}

fn strip_managed_prompt_blocks(content: &str) -> String {
  let mut output = Vec::new();
  let mut skipping = false;
  for line in content.lines() {
    if line.starts_with("<!-- AI8888-PROMPT:") && line.ends_with(":START -->") {
      skipping = true;
      continue;
    }
    if skipping && line.starts_with("<!-- AI8888-PROMPT:") && line.ends_with(":END -->") {
      skipping = false;
      continue;
    }
    if !skipping {
      output.push(line);
    }
  }
  output.join("\n").trim_end().to_string()
}

fn sync_all_prompts(data: &WorkspaceData) -> Result<(), AppError> {
  for app in SUPPORTED_APPS {
    let path = prompt_path(app);
    let existing = if path.exists() { fs::read_to_string(&path).map_err(|error| AppError::io(&path, error))? } else { String::new() };
    let mut content = strip_managed_prompt_blocks(&existing);
    for prompt in data.prompts.iter().filter(|item| item.enabled_apps.iter().any(|enabled| enabled == app)) {
      if !content.is_empty() {
        content.push_str("\n\n");
      }
      content.push_str(&format!("<!-- AI8888-PROMPT:{}:START -->\n{}\n<!-- AI8888-PROMPT:{}:END -->", prompt.id, prompt.content.trim(), prompt.id));
    }
    if !content.is_empty() || path.exists() {
      write_text(&path, &(content + "\n"))?;
    }
  }
  Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), AppError> {
  if fs::symlink_metadata(source).map(|metadata| metadata.file_type().is_symlink()).unwrap_or(false) {
    return Err(AppError::Message(format!("skill source contains a symlink: {}", source.display())));
  }
  fs::create_dir_all(destination).map_err(|error| AppError::io(destination, error))?;
  for entry in fs::read_dir(source).map_err(|error| AppError::io(source, error))? {
    let entry = entry.map_err(|error| AppError::io(source, error))?;
    let target = destination.join(entry.file_name());
    let file_type = entry.file_type().map_err(|error| AppError::io(&entry.path(), error))?;
    if file_type.is_symlink() {
      return Err(AppError::Message(format!("skill source contains a symlink: {}", entry.path().display())));
    }
    if file_type.is_dir() {
      copy_directory(&entry.path(), &target)?;
    } else if file_type.is_file() {
      fs::copy(entry.path(), &target).map_err(|error| AppError::io(&target, error))?;
    }
  }
  Ok(())
}

fn extract_zip(bytes: &[u8], destination: &Path) -> Result<(), AppError> {
  if bytes.len() > MAX_SKILL_ARCHIVE_BYTES {
    return Err(AppError::Message("skill archive exceeds 32 MB".into()));
  }
  let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| AppError::Message(format!("invalid skill zip: {error}")))?;
  if archive.len() > MAX_SKILL_ARCHIVE_ENTRIES {
    return Err(AppError::Message("skill archive contains too many entries".into()));
  }
  let mut extracted_bytes = 0u64;
  let mut paths = HashSet::new();
  for index in 0..archive.len() {
    let entry = archive.by_index(index).map_err(|error| AppError::Message(error.to_string()))?;
    if entry.name().contains('\\') || entry.unix_mode().map(|mode| mode & 0o170000 == 0o120000).unwrap_or(false) {
      return Err(AppError::Message("skill zip contains an unsafe entry".into()));
    }
    let Some(path) = entry.enclosed_name() else {
      return Err(AppError::Message("skill zip contains an unsafe path".into()));
    };
    let key = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if !paths.insert(key) {
      return Err(AppError::Message("skill zip contains duplicate paths".into()));
    }
    if !entry.is_dir() {
      if entry.size() > MAX_SKILL_FILE_BYTES {
        return Err(AppError::Message("skill zip contains a file larger than 16 MB".into()));
      }
      extracted_bytes = extracted_bytes.checked_add(entry.size()).ok_or_else(|| AppError::Message("skill archive size overflow".into()))?;
      if extracted_bytes > MAX_SKILL_EXTRACTED_BYTES {
        return Err(AppError::Message("skill archive expands beyond 128 MB".into()));
      }
    }
  }
  for index in 0..archive.len() {
    let mut entry = archive.by_index(index).map_err(|error| AppError::Message(error.to_string()))?;
    let Some(path) = entry.enclosed_name() else {
      return Err(AppError::Message("skill zip contains an unsafe path".into()));
    };
    let output = destination.join(path);
    if entry.is_dir() {
      fs::create_dir_all(&output).map_err(|error| AppError::io(&output, error))?;
    } else {
      if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?;
      }
      let expected = entry.size();
      let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&output).map_err(|error| AppError::io(&output, error))?;
      let copied = std::io::copy(&mut entry.take(MAX_SKILL_FILE_BYTES + 1), &mut file).map_err(|error| AppError::io(&output, error))?;
      if copied != expected {
        return Err(AppError::Message("skill zip entry size does not match its metadata".into()));
      }
    }
  }
  Ok(())
}

async fn download_skill_archive(response: reqwest::Response) -> Result<Vec<u8>, AppError> {
  if response.content_length().map(|length| length > MAX_SKILL_ARCHIVE_BYTES as u64).unwrap_or(false) {
    return Err(AppError::Message("skill archive exceeds 32 MB".into()));
  }
  let capacity = response.content_length().unwrap_or(0).min(MAX_SKILL_ARCHIVE_BYTES as u64) as usize;
  let mut bytes = Vec::with_capacity(capacity);
  let mut stream = response.bytes_stream();
  while let Some(chunk) = stream.next().await {
    let chunk = chunk.map_err(|error| AppError::Message(error.to_string()))?;
    if bytes.len().saturating_add(chunk.len()) > MAX_SKILL_ARCHIVE_BYTES {
      return Err(AppError::Message("skill archive exceeds 32 MB".into()));
    }
    bytes.extend_from_slice(&chunk);
  }
  Ok(bytes)
}

fn find_skill_root(root: &Path) -> Option<PathBuf> {
  if root.join("SKILL.md").is_file() {
    return Some(root.to_path_buf());
  }
  let entries = fs::read_dir(root).ok()?;
  for entry in entries.flatten() {
    if entry.file_type().ok()?.is_dir() {
      if let Some(found) = find_skill_root(&entry.path()) {
        return Some(found);
      }
    }
  }
  None
}

fn skill_target_root(app: &str) -> PathBuf {
  match app {
    "codex" => path_for("codex", "skills"),
    "claude" => path_for("claude", "skills"),
    "gemini" => path_for("gemini", "skills"),
    "opencode" => path_for("opencode", "skills"),
    "openclaw" => path_for("openclaw", "skills"),
    "hermes" => path_for("hermes", "skills"),
    _ => app_dir().join("unsupported-skills"),
  }
}

fn remove_managed_skill(path: &Path) -> Result<(), AppError> {
  if path.join(".ai8888-managed").exists() {
    fs::remove_dir_all(path).map_err(|error| AppError::io(path, error))?;
  }
  Ok(())
}

fn sync_all_skills(data: &WorkspaceData) -> Result<(), AppError> {
  for skill in &data.skills {
    let source = managed_skills_dir().join(&skill.id);
    for app in SUPPORTED_APPS {
      let destination = skill_target_root(app).join(&skill.id);
      if skill.enabled_apps.iter().any(|enabled| enabled == app) {
        if destination.exists() && !destination.join(".ai8888-managed").exists() {
          return Err(AppError::Message(format!("refusing to overwrite unmanaged skill: {}", destination.display())));
        }
        if destination.exists() {
          fs::remove_dir_all(&destination).map_err(|error| AppError::io(&destination, error))?;
        }
        copy_directory(&source, &destination)?;
        fs::write(destination.join(".ai8888-managed"), skill.id.as_bytes()).map_err(|error| AppError::io(&destination, error))?;
      } else {
        remove_managed_skill(&destination)?;
      }
    }
  }
  Ok(())
}

fn github_archive_urls(source: &str) -> Option<Vec<String>> {
  let rest = source.strip_prefix("https://github.com/")?;
  let mut parts = rest.trim_end_matches('/').split('/');
  let owner = parts.next()?;
  let repo = parts.next()?.trim_end_matches(".git");
  let tail = parts.collect::<Vec<_>>();
  let branches = if tail.first() == Some(&"tree") { vec![tail.get(1).copied().unwrap_or("main")] } else { vec!["main", "master"] };
  Some(branches.into_iter().map(|branch| format!("https://codeload.github.com/{owner}/{repo}/zip/refs/heads/{branch}")).collect())
}

#[tauri::command]
pub async fn app_install_skill(id: String, name: String, source: String, description: String, enabled_apps: Vec<String>) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let enabled_apps = normalize_apps(enabled_apps).map_err(|error| error.to_string())?;
  if name.trim().is_empty() || source.trim().is_empty() {
    return Err("Skill name and source are required".into());
  }
  let root = managed_skills_dir();
  fs::create_dir_all(&root).map_err(|error| error.to_string())?;
  let staging = root.join(format!(".{id}.{}.tmp", now_ms()));
  if staging.exists() {
    fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
  }
  fs::create_dir_all(&staging).map_err(|error| error.to_string())?;

  let source_path = PathBuf::from(source.trim());
  let install_result = if source_path.is_dir() {
    copy_directory(&source_path, &staging)
  } else if source_path.is_file() {
    let bytes = fs::metadata(&source_path).map_err(|error| AppError::io(&source_path, error)).and_then(|metadata| {
      if metadata.len() > MAX_SKILL_ARCHIVE_BYTES as u64 {
        Err(AppError::Message("skill archive exceeds 32 MB".into()))
      } else {
        fs::read(&source_path).map_err(|error| AppError::io(&source_path, error))
      }
    });
    bytes.and_then(|bytes| extract_zip(&bytes, &staging))
  } else if source.starts_with("https://") {
    let urls = if source.to_ascii_lowercase().ends_with(".zip") {
      vec![source.clone()]
    } else {
      github_archive_urls(&source).ok_or_else(|| "Only GitHub repositories or ZIP URLs are supported".to_string())?
    };
    let client = reqwest::Client::new();
    let mut downloaded = None;
    let mut last_error = String::new();
    for url in urls {
      match client.get(&url).send().await {
        Ok(response) if response.status().is_success() => match download_skill_archive(response).await {
          Ok(bytes) => {
            downloaded = Some(bytes);
            break;
          }
          Err(error) => last_error = error.to_string(),
        },
        Ok(response) => last_error = format!("{url} returned {}", response.status()),
        Err(error) => last_error = error.to_string(),
      }
    }
    let bytes = downloaded.ok_or_else(|| format!("Skill download failed: {last_error}"))?;
    extract_zip(&bytes, &staging)
  } else {
    Err(AppError::Message("Skill source path does not exist".into()))
  };
  if let Err(error) = install_result {
    let _ = fs::remove_dir_all(&staging);
    return Err(error.to_string());
  }
  let skill_root = match find_skill_root(&staging) {
    Some(path) => path,
    None => {
      let _ = fs::remove_dir_all(&staging);
      return Err("Skill package does not contain SKILL.md".into());
    }
  };
  let destination = root.join(&id);
  if destination.exists() {
    fs::remove_dir_all(&destination).map_err(|error| error.to_string())?;
  }
  if skill_root == staging {
    fs::rename(&staging, &destination).map_err(|error| error.to_string())?;
  } else {
    copy_directory(&skill_root, &destination).map_err(|error| error.to_string())?;
    fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
  }

  let mut data = load_workspace().map_err(|error| error.to_string())?;
  let skill = SkillPackage {
    id: id.clone(),
    name: name.trim().into(),
    description: description.trim().into(),
    source,
    enabled_apps,
    updated_at: now_ms(),
  };
  if let Some(existing) = data.skills.iter_mut().find(|item| item.id == id) {
    *existing = skill;
  } else {
    data.skills.push(skill);
  }
  sync_all_skills(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_update_skill_apps(id: String, enabled_apps: Vec<String>) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let enabled_apps = normalize_apps(enabled_apps).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  let skill = data.skills.iter_mut().find(|item| item.id == id).ok_or_else(|| "Skill not found".to_string())?;
  skill.enabled_apps = enabled_apps;
  skill.updated_at = now_ms();
  sync_all_skills(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_delete_skill(id: String) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  let source = managed_skills_dir().join(&id);
  if source.exists() {
    let backup_root = skill_backups_dir().join(format!("{}-{id}", now_ms()));
    copy_directory(&source, &backup_root).map_err(|error| error.to_string())?;
    fs::remove_dir_all(&source).map_err(|error| error.to_string())?;
  }
  for app in SUPPORTED_APPS {
    remove_managed_skill(&skill_target_root(app).join(&id)).map_err(|error| error.to_string())?;
  }
  data.skills.retain(|item| item.id != id);
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

fn capture_project_state(data: &WorkspaceData, id: String, name: String, profile_id: Option<String>) -> ProjectSnapshot {
  ProjectSnapshot {
    id,
    name,
    profile_id,
    mcp_apps: data.mcp_servers.iter().map(|item| (item.id.clone(), item.enabled_apps.clone())).collect(),
    prompt_apps: data.prompts.iter().map(|item| (item.id.clone(), item.enabled_apps.clone())).collect(),
    skill_apps: data.skills.iter().map(|item| (item.id.clone(), item.enabled_apps.clone())).collect(),
    updated_at: now_ms(),
  }
}

#[tauri::command]
pub fn app_save_project(id: Option<String>, name: String, profile_id: Option<String>) -> Result<WorkspaceData, String> {
  if name.trim().is_empty() {
    return Err("Project name is required".into());
  }
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  let id = match id {
    Some(value) => normalize_id(&value).map_err(|error| error.to_string())?,
    None => normalize_id(&format!("project-{}", now_ms())).map_err(|error| error.to_string())?,
  };
  let project = capture_project_state(&data, id.clone(), name.trim().into(), profile_id);
  if let Some(existing) = data.projects.iter_mut().find(|item| item.id == id) {
    *existing = project;
  } else {
    data.projects.push(project);
  }
  data.active_project_id = Some(id);
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_apply_project(id: String) -> Result<ProjectSnapshot, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  if let Some(active_id) = data.active_project_id.clone().filter(|active| active != &id) {
    if let Some(index) = data.projects.iter().position(|item| item.id == active_id) {
      let old = data.projects[index].clone();
      data.projects[index] = capture_project_state(&data, old.id, old.name, old.profile_id);
    }
  }
  let project = data.projects.iter().find(|item| item.id == id).cloned().ok_or_else(|| "Project not found".to_string())?;
  for item in &mut data.mcp_servers {
    item.enabled_apps = project.mcp_apps.get(&item.id).cloned().unwrap_or_default();
  }
  for item in &mut data.prompts {
    item.enabled_apps = project.prompt_apps.get(&item.id).cloned().unwrap_or_default();
  }
  for item in &mut data.skills {
    item.enabled_apps = project.skill_apps.get(&item.id).cloned().unwrap_or_default();
  }
  data.active_project_id = Some(id);
  sync_all_mcp(&data).map_err(|error| error.to_string())?;
  sync_all_prompts(&data).map_err(|error| error.to_string())?;
  sync_all_skills(&data).map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(project)
}

#[tauri::command]
pub fn app_delete_project(id: String) -> Result<WorkspaceData, String> {
  let id = normalize_id(&id).map_err(|error| error.to_string())?;
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  data.projects.retain(|item| item.id != id);
  if data.active_project_id.as_deref() == Some(&id) {
    data.active_project_id = None;
  }
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_save_proxy_settings(mut settings: ProxySettings) -> Result<WorkspaceData, String> {
  settings.request_timeout_ms = settings.request_timeout_ms.clamp(5_000, 600_000);
  settings.connect_timeout_ms = settings.connect_timeout_ms.clamp(1_000, 60_000);
  settings.max_retries = settings.max_retries.min(10);
  settings.circuit_failure_threshold = settings.circuit_failure_threshold.clamp(1, 20);
  settings.circuit_open_seconds = settings.circuit_open_seconds.clamp(5, 3_600);
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  settings.local_proxy_token = load_or_create_local_proxy_token().map_err(|error| error.to_string())?;
  let mut seen = HashSet::new();
  for endpoint in &mut settings.endpoints {
    endpoint.id = normalize_id(&endpoint.id).map_err(|error| error.to_string())?;
    endpoint.base_url = endpoint.base_url.trim().trim_end_matches('/').to_string();
    if !endpoint.base_url.starts_with("http://") && !endpoint.base_url.starts_with("https://") {
      return Err(format!("Invalid endpoint URL: {}", endpoint.base_url));
    }
    if !seen.insert(endpoint.id.clone()) {
      return Err(format!("Duplicate endpoint ID: {}", endpoint.id));
    }
  }
  settings.endpoints.sort_by_key(|item| item.priority);
  data.proxy_settings = settings;
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

#[tauri::command]
pub fn app_save_model_price(mut price: ModelPrice) -> Result<WorkspaceData, String> {
  price.model = price.model.trim().to_ascii_lowercase();
  if price.model.is_empty()
    || !price.input_per_million.is_finite()
    || !price.output_per_million.is_finite()
    || !price.cached_input_per_million.is_finite()
    || price.input_per_million < 0.0
    || price.output_per_million < 0.0
    || price.cached_input_per_million < 0.0
  {
    return Err("Invalid model price".into());
  }
  let mut data = load_workspace().map_err(|error| error.to_string())?;
  if let Some(existing) = data.model_prices.iter_mut().find(|item| item.model.eq_ignore_ascii_case(&price.model)) {
    *existing = price;
  } else {
    data.model_prices.push(price);
  }
  save_workspace(&data).map_err(|error| error.to_string())?;
  Ok(data)
}

pub fn workspace_files() -> Vec<PathBuf> {
  vec![workspace_path(), managed_skills_dir()]
}

#[cfg(test)]
mod tests {
  use super::*;

  fn with_workspace_test_home<T>(name: &str, test: impl FnOnce() -> T) -> T {
    let _guard = crate::config::test_home_guard();
    let root = std::env::temp_dir().join(format!("ai8888-workspace-{name}-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&root).expect("create test home");
    let old_home = std::env::var_os("AI8888_SWITCH_TEST_HOME");
    std::env::set_var("AI8888_SWITCH_TEST_HOME", &root);
    fs::create_dir_all(root.join(".codex")).expect("create test Codex home");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
    match old_home {
      Some(value) => std::env::set_var("AI8888_SWITCH_TEST_HOME", value),
      None => std::env::remove_var("AI8888_SWITCH_TEST_HOME"),
    }
    let _ = fs::remove_dir_all(root);
    match result {
      Ok(value) => value,
      Err(payload) => std::panic::resume_unwind(payload),
    }
  }

  fn test_mcp_server(transport: &str) -> McpServer {
    McpServer {
      id: "managed".into(),
      name: "Managed".into(),
      transport: transport.into(),
      command: "new-command".into(),
      args: vec!["new-arg".into()],
      env: HashMap::from([("KEEP".into(), "new-value".into()), ("ADD".into(), "added".into())]),
      url: "https://new.example/mcp".into(),
      enabled_apps: vec!["codex".into()],
      updated_at: 0,
    }
  }

  #[test]
  fn proxy_token_is_not_serialized_to_workspace_or_renderer() {
    let settings = ProxySettings::default();
    assert!(!settings.local_proxy_token.is_empty());
    let serialized = serde_json::to_value(settings).expect("serialize proxy settings");
    assert!(serialized.get("localProxyToken").is_none());
  }

  #[test]
  fn managed_prompt_blocks_preserve_user_content() {
    let input = "user\n<!-- AI8888-PROMPT:a:START -->\nmanaged\n<!-- AI8888-PROMPT:a:END -->\ntail\n";
    assert_eq!(strip_managed_prompt_blocks(input), "user\ntail");
  }

  #[test]
  fn ids_reject_path_traversal() {
    assert!(normalize_id("../../secret").is_err());
    assert_eq!(normalize_id("My Server").expect("valid id"), "my-server");
  }

  #[test]
  fn codex_mcp_updates_in_place_and_preserves_unknown_fields_comments_and_order() {
    with_workspace_test_home("mcp-preserve", || {
      let path = path_for("codex", "config.toml");
      let original = r#"# root comment
[mcp_servers.before]
url = "https://before.example/mcp"

# managed entry comment
[mcp_servers.managed]
unknown_before = "keep-before" # unknown before comment
command = "old-command" # command comment
args = ["old-arg"] # args comment
url = "https://old.example/mcp" # conflicting URL comment
type = "sse"
env = { KEEP = "old-value", STALE = "remove-me" } # env comment
unknown_after = 42 # unknown after comment

[mcp_servers.after]
command = "after"
"#;
      fs::write(&path, original).expect("seed Codex config");

      let mut data = WorkspaceData::default();
      data.mcp_servers.push(test_mcp_server("stdio"));
      sync_mcp_app("codex", &data).expect("sync stdio MCP");
      let first = fs::read_to_string(&path).expect("read synced Codex config");
      let first_doc = first.parse::<DocumentMut>().expect("parse synced Codex config");
      let first_entry = first_doc["mcp_servers"]["managed"].as_table().expect("managed table");
      assert_eq!(first_entry["unknown_before"].as_str(), Some("keep-before"));
      assert_eq!(first_entry["unknown_after"].as_integer(), Some(42));
      assert_eq!(first_entry["command"].as_str(), Some("new-command"));
      assert_eq!(first_entry["args"].as_array().and_then(|args| args.get(0)).and_then(toml_edit::Value::as_str), Some("new-arg"));
      assert_eq!(first_entry["env"]["KEEP"].as_str(), Some("new-value"));
      assert_eq!(first_entry["env"]["ADD"].as_str(), Some("added"));
      assert!(first_entry["env"].get("STALE").is_none());
      assert!(first_entry.get("url").is_none());
      assert!(first_entry.get("type").is_none());
      assert!(first.contains("# root comment"));
      assert!(first.contains("# managed entry comment"));
      assert!(first.contains("# command comment"));
      assert!(first.contains("# args comment"));
      assert!(first.contains("# env comment"));
      assert!(first.contains("# unknown before comment"));
      assert!(first.contains("# unknown after comment"));
      assert!(first.find("[mcp_servers.before]").unwrap() < first.find("[mcp_servers.managed]").unwrap());
      assert!(first.find("[mcp_servers.managed]").unwrap() < first.find("[mcp_servers.after]").unwrap());
      assert!(first.find("unknown_before").unwrap() < first.find("command").unwrap());
      assert!(first.find("command").unwrap() < first.find("unknown_after").unwrap());

      let mut http = test_mcp_server("http");
      http.command.clear();
      http.args.clear();
      http.env.clear();
      sync_mcp_app(
        "codex",
        &WorkspaceData {
          mcp_servers: vec![http.clone()],
          ..data.clone()
        },
      )
      .expect("switch MCP to HTTP");
      let second = fs::read_to_string(&path).expect("read HTTP Codex config");
      let second_doc = second.parse::<DocumentMut>().expect("parse HTTP Codex config");
      let second_entry = second_doc["mcp_servers"]["managed"].as_table().expect("managed HTTP table");
      assert_eq!(second_entry["url"].as_str(), Some("https://new.example/mcp"));
      assert!(second_entry.get("command").is_none());
      assert!(second_entry.get("args").is_none());
      assert!(second_entry.get("env").is_none());
      assert!(second_entry.get("type").is_none());
      assert_eq!(second_entry["unknown_after"].as_integer(), Some(42));
      assert!(second.contains("# unknown after comment"));
      assert!(second.find("[mcp_servers.before]").unwrap() < second.find("[mcp_servers.managed]").unwrap());
      assert!(second.find("[mcp_servers.managed]").unwrap() < second.find("[mcp_servers.after]").unwrap());

      let stdio_again = test_mcp_server("stdio");
      sync_mcp_app("codex", &WorkspaceData { mcp_servers: vec![stdio_again.clone()], ..data.clone() }).expect("switch MCP back to stdio");
      let third = fs::read_to_string(&path).expect("read second stdio Codex config");
      let third_doc = third.parse::<DocumentMut>().expect("parse second stdio Codex config");
      assert_eq!(third_doc["mcp_servers"]["managed"]["env"]["KEEP"].as_str(), Some("new-value"));
      assert!(third.find("[mcp_servers.before]").unwrap() < third.find("[mcp_servers.managed]").unwrap());
      assert!(third.find("[mcp_servers.managed]").unwrap() < third.find("[mcp_servers.after]").unwrap());
      assert!(third.find("[mcp_servers.managed.env]").unwrap() < third.find("[mcp_servers.after]").unwrap());

      let mut disabled_server = stdio_again;
      disabled_server.enabled_apps.clear();
      sync_mcp_app("codex", &WorkspaceData { mcp_servers: vec![disabled_server], ..data }).expect("disable MCP");
      let disabled = fs::read_to_string(&path).expect("read disabled Codex config");
      assert!(!disabled.contains("[mcp_servers.managed]"));
      assert!(disabled.contains("[mcp_servers.before]"));
      assert!(disabled.contains("[mcp_servers.after]"));
    });
  }

  #[test]
  fn malformed_codex_mcp_toml_is_rejected_without_overwriting_bytes() {
    with_workspace_test_home("mcp-malformed", || {
      let path = path_for("codex", "config.toml");
      let original = b"# preserve exactly\n[mcp_servers.broken\ncommand = \"old\"\n";
      fs::write(&path, original).expect("seed malformed Codex config");
      let mut data = WorkspaceData::default();
      data.mcp_servers.push(test_mcp_server("stdio"));
      let error = sync_mcp_app("codex", &data).expect_err("malformed TOML must fail");
      assert!(error.to_string().contains("parse failed"));
      assert_eq!(fs::read(&path).expect("read malformed Codex config"), original);
    });
  }
}
