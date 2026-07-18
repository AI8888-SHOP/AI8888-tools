use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use pbkdf2::pbkdf2_hmac;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::config::{
  app_dir, ensure_app_dir, exports_dir, local_route_manifest_path, managed_skills_dir, path_for, preferences_path, profiles_path, read_json,
  workspace_path, write_json,
};
use crate::error::AppError;
use crate::workspace::{app_sync_mcp_servers, load_workspace, save_workspace, sync_workspace_extensions, WorkspaceData};

const EXPORT_SCHEMA_VERSION: u32 = 1;
const PBKDF2_ROUNDS: u32 = 210_000;
const MAX_EXPORT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportFile {
  path: String,
  content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConfigBundle {
  schema_version: u32,
  created_at: u64,
  sanitized: bool,
  profiles: Value,
  workspace: WorkspaceData,
  preferences: Value,
  local_routing: Value,
  #[serde(default)]
  skill_files: Vec<ExportFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedEnvelope {
  format: String,
  rounds: u32,
  salt: String,
  nonce: String,
  ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
  pub path: String,
  pub encrypted: bool,
  pub sanitized: bool,
  pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticItem {
  pub id: String,
  pub level: String,
  pub title: String,
  pub detail: String,
  pub path: Option<String>,
}

fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or(0)
}

fn json_or_default(path: &Path, default: Value) -> Value {
  read_json(path).unwrap_or(default)
}

fn redact_value(value: &mut Value) {
  match value {
    Value::Object(map) => {
      for (key, item) in map {
        let lowered = key.to_ascii_lowercase();
        if lowered.contains("token") || lowered.contains("api_key") || lowered.contains("apikey") || lowered == "authorization" || lowered == "password" || lowered == "secret" {
          *item = Value::String("<redacted>".into());
        } else if lowered == "env" {
          if let Some(env) = item.as_object_mut() {
            for value in env.values_mut() { *value = Value::String("<redacted>".into()); }
          }
        } else {
          redact_value(item);
        }
      }
    }
    Value::Array(items) => items.iter_mut().for_each(redact_value),
    _ => {}
  }
}

fn collect_skill_files(root: &Path, current: &Path, result: &mut Vec<ExportFile>, total: &mut usize) -> Result<(), AppError> {
  if !current.exists() { return Ok(()); }
  for entry in fs::read_dir(current).map_err(|error| AppError::io(current, error))? {
    let entry = entry.map_err(|error| AppError::io(current, error))?;
    let file_type = entry.file_type().map_err(|error| AppError::io(&entry.path(), error))?;
    if file_type.is_symlink() { continue; }
    if file_type.is_dir() {
      collect_skill_files(root, &entry.path(), result, total)?;
    } else {
      let bytes = fs::read(entry.path()).map_err(|error| AppError::io(&entry.path(), error))?;
      *total = total.saturating_add(bytes.len());
      if *total > MAX_EXPORT_BYTES { return Err(AppError::Message("skill export exceeds 64 MB".into())); }
      let entry_path = entry.path();
      let relative = entry_path.strip_prefix(root).map_err(|_| AppError::Message("invalid skill export path".into()))?;
      result.push(ExportFile { path: relative.to_string_lossy().replace('\\', "/"), content: BASE64.encode(bytes) });
    }
  }
  Ok(())
}

fn derive_key(passphrase: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
  let mut key = [0u8; 32];
  pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, rounds, &mut key);
  key
}

fn encrypt_bundle(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>, AppError> {
  if passphrase.chars().count() < 8 { return Err(AppError::Message("encrypted export requires a passphrase of at least 8 characters".into())); }
  let mut salt = [0u8; 16];
  let mut nonce = [0u8; 12];
  rand::rngs::OsRng.fill_bytes(&mut salt);
  rand::rngs::OsRng.fill_bytes(&mut nonce);
  let key = derive_key(passphrase, &salt, PBKDF2_ROUNDS);
  let cipher = Aes256Gcm::new_from_slice(&key).map_err(|error| AppError::Message(error.to_string()))?;
  let ciphertext = cipher.encrypt(Nonce::from_slice(&nonce), bytes).map_err(|_| AppError::Message("config encryption failed".into()))?;
  let envelope = EncryptedEnvelope {
    format: "ai8888-config-aes256gcm-v1".into(), rounds: PBKDF2_ROUNDS,
    salt: BASE64.encode(salt), nonce: BASE64.encode(nonce), ciphertext: BASE64.encode(ciphertext),
  };
  serde_json::to_vec_pretty(&envelope).map_err(|error| AppError::Message(error.to_string()))
}

fn decrypt_bundle(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>, AppError> {
  let envelope: EncryptedEnvelope = serde_json::from_slice(bytes).map_err(|error| AppError::Message(format!("invalid encrypted export: {error}")))?;
  if envelope.format != "ai8888-config-aes256gcm-v1" { return Err(AppError::Message("unsupported encrypted export format".into())); }
  let salt = BASE64.decode(envelope.salt).map_err(|_| AppError::Message("invalid export salt".into()))?;
  let nonce = BASE64.decode(envelope.nonce).map_err(|_| AppError::Message("invalid export nonce".into()))?;
  let ciphertext = BASE64.decode(envelope.ciphertext).map_err(|_| AppError::Message("invalid export ciphertext".into()))?;
  if nonce.len() != 12 { return Err(AppError::Message("invalid export nonce length".into())); }
  let key = derive_key(passphrase, &salt, envelope.rounds);
  let cipher = Aes256Gcm::new_from_slice(&key).map_err(|error| AppError::Message(error.to_string()))?;
  cipher.decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref()).map_err(|_| AppError::Message("wrong passphrase or damaged export".into()))
}

#[tauri::command]
pub fn app_export_config(include_secrets: bool, passphrase: String) -> Result<ExportResult, String> {
  ensure_app_dir().map_err(|error| error.to_string())?;
  let mut profiles = json_or_default(&profiles_path(), json!({ "schemaVersion": 1, "profiles": [] }));
  let mut workspace = load_workspace().map_err(|error| error.to_string())?;
  let mut local_routing = json_or_default(&local_route_manifest_path(), json!({}));
  let preferences = json_or_default(&preferences_path(), json!({}));
  let mut skill_files = Vec::new();
  let mut skill_bytes = 0usize;
  collect_skill_files(&managed_skills_dir(), &managed_skills_dir(), &mut skill_files, &mut skill_bytes).map_err(|error| error.to_string())?;
  if !include_secrets {
    redact_value(&mut profiles);
    let mut workspace_value = serde_json::to_value(&workspace).map_err(|error| error.to_string())?;
    redact_value(&mut workspace_value);
    workspace = serde_json::from_value(workspace_value).map_err(|error| error.to_string())?;
    redact_value(&mut local_routing);
  } else if passphrase.chars().count() < 8 {
    return Err("Including secrets requires a passphrase of at least 8 characters".into());
  }
  let bundle = ConfigBundle {
    schema_version: EXPORT_SCHEMA_VERSION, created_at: now_ms(), sanitized: !include_secrets,
    profiles, workspace, preferences, local_routing, skill_files,
  };
  let plain = serde_json::to_vec_pretty(&bundle).map_err(|error| error.to_string())?;
  let encrypted = include_secrets;
  let bytes = if encrypted { encrypt_bundle(&plain, &passphrase).map_err(|error| error.to_string())? } else { plain };
  let directory = exports_dir();
  fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
  let suffix = if encrypted { "encrypted.json" } else { "sanitized.json" };
  let path = directory.join(format!("ai8888-config-{}-{suffix}", now_ms()));
  fs::write(&path, &bytes).map_err(|error| error.to_string())?;
  Ok(ExportResult { path: path.display().to_string(), encrypted, sanitized: !include_secrets, size_bytes: bytes.len() as u64 })
}

fn safe_relative_path(value: &str) -> Result<PathBuf, AppError> {
  let path = PathBuf::from(value);
  if path.is_absolute() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
    return Err(AppError::Message("export contains an unsafe skill path".into()));
  }
  Ok(path)
}

#[tauri::command]
pub fn app_import_config(path: String, passphrase: String) -> Result<WorkspaceData, String> {
  let path = PathBuf::from(path.trim());
  if !path.is_file() { return Err("Import file does not exist".into()); }
  let bytes = fs::read(&path).map_err(|error| error.to_string())?;
  if bytes.len() > MAX_EXPORT_BYTES { return Err("Import file exceeds 64 MB".into()); }
  let plain = match serde_json::from_slice::<EncryptedEnvelope>(&bytes) {
    Ok(envelope) if envelope.format == "ai8888-config-aes256gcm-v1" => decrypt_bundle(&bytes, &passphrase).map_err(|error| error.to_string())?,
    _ => bytes,
  };
  let bundle: ConfigBundle = serde_json::from_slice(&plain).map_err(|error| format!("Invalid config export: {error}"))?;
  if bundle.schema_version > EXPORT_SCHEMA_VERSION { return Err("Config export is newer than this application".into()); }
  if bundle.sanitized { return Err("Sanitized exports are for sharing and cannot restore redacted secrets".into()); }

  write_json(&profiles_path(), &bundle.profiles).map_err(|error| error.to_string())?;
  save_workspace(&bundle.workspace).map_err(|error| error.to_string())?;
  write_json(&preferences_path(), &bundle.preferences).map_err(|error| error.to_string())?;
  write_json(&local_route_manifest_path(), &bundle.local_routing).map_err(|error| error.to_string())?;
  let skills_root = managed_skills_dir();
  fs::create_dir_all(&skills_root).map_err(|error| error.to_string())?;
  for file in bundle.skill_files {
    let relative = safe_relative_path(&file.path).map_err(|error| error.to_string())?;
    let destination = skills_root.join(relative);
    if let Some(parent) = destination.parent() { fs::create_dir_all(parent).map_err(|error| error.to_string())?; }
    let content = BASE64.decode(file.content).map_err(|_| "Invalid base64 skill content".to_string())?;
    fs::write(destination, content).map_err(|error| error.to_string())?;
  }
  sync_workspace_extensions(&bundle.workspace).map_err(|error| error.to_string())?;
  Ok(bundle.workspace)
}

fn diagnostic_file(id: &str, title: &str, path: PathBuf, format: &str) -> DiagnosticItem {
  if !path.exists() {
    return DiagnosticItem { id: id.into(), level: "info".into(), title: title.into(), detail: "Configuration file has not been created yet".into(), path: Some(path.display().to_string()) };
  }
  let content = match fs::read_to_string(&path) {
    Ok(content) => content,
    Err(error) => return DiagnosticItem { id: id.into(), level: "error".into(), title: title.into(), detail: error.to_string(), path: Some(path.display().to_string()) },
  };
  let valid = match format {
    "json" => serde_json::from_str::<Value>(&content).map(|_| ()).map_err(|error| error.to_string()),
    "toml" => toml::from_str::<toml::Value>(&content).map(|_| ()).map_err(|error| error.to_string()),
    "yaml" => serde_yaml::from_str::<serde_yaml::Value>(&content).map(|_| ()).map_err(|error| error.to_string()),
    _ => Ok(()),
  };
  match valid {
    Ok(()) => DiagnosticItem { id: id.into(), level: "ok".into(), title: title.into(), detail: "Readable and valid".into(), path: Some(path.display().to_string()) },
    Err(error) => DiagnosticItem { id: id.into(), level: "error".into(), title: title.into(), detail: format!("Parse failed: {error}"), path: Some(path.display().to_string()) },
  }
}

#[tauri::command]
pub fn app_run_diagnostics() -> Vec<DiagnosticItem> {
  let mut items = vec![
    diagnostic_file("codex", "Codex config.toml", path_for("codex", "config.toml"), "toml"),
    diagnostic_file("claude", "Claude settings.json", path_for("claude", "settings.json"), "json"),
    diagnostic_file("gemini", "Gemini settings.json", path_for("gemini", "settings.json"), "json"),
    diagnostic_file("opencode", "OpenCode opencode.json", path_for("opencode", "opencode.json"), "json"),
    diagnostic_file("openclaw", "OpenClaw openclaw.json", path_for("openclaw", "openclaw.json"), "json"),
    diagnostic_file("hermes", "Hermes config.yaml", path_for("hermes", "config.yaml"), "yaml"),
  ];
  let app_root = app_dir();
  items.push(DiagnosticItem {
    id: "app-data".into(), level: if app_root.exists() { "ok".into() } else { "info".into() }, title: "AI8888 data directory".into(),
    detail: if app_root.exists() { "Data directory is available".into() } else { "Data directory will be created on first write".into() }, path: Some(app_root.display().to_string()),
  });
  items.push(DiagnosticItem {
    id: "workspace".into(), level: match load_workspace() { Ok(_) => "ok".into(), Err(_) => "error".into() }, title: "Workspace database".into(),
    detail: match load_workspace() { Ok(data) => format!("{} MCP, {} prompts, {} skills, {} projects", data.mcp_servers.len(), data.prompts.len(), data.skills.len(), data.projects.len()), Err(error) => error.to_string() },
    path: Some(workspace_path().display().to_string()),
  });
  items
}

#[tauri::command]
pub fn app_repair_workspace() -> Result<WorkspaceData, String> {
  ensure_app_dir().map_err(|error| error.to_string())?;
  let data = load_workspace().map_err(|error| error.to_string())?;
  save_workspace(&data).map_err(|error| error.to_string())?;
  let _ = app_sync_mcp_servers()?;
  Ok(data)
}
