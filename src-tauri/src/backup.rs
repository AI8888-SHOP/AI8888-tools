use std::collections::HashSet;
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
  app_dir, atomic_write, ensure_app_dir, exports_dir, local_route_manifest_path, managed_skills_dir, path_for, preferences_path, profiles_path,
  read_json, workspace_path,
};
use crate::error::AppError;
use crate::workspace::{app_sync_mcp_servers, load_workspace, save_workspace, sync_workspace_extensions, validate_workspace_data, WorkspaceData};

const EXPORT_SCHEMA_VERSION: u32 = 1;
const PBKDF2_ROUNDS: u32 = 210_000;
const EXPORT_SALT_BYTES: usize = 16;
const EXPORT_NONCE_BYTES: usize = 12;
const MAX_SKILL_EXPORT_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPORT_FILE_BYTES: usize = 128 * 1024 * 1024;
const MAX_SKILL_FILES: usize = 4_096;

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
        if lowered.contains("token") || lowered.contains("api_key") || lowered.contains("apikey") || lowered == "authorization" || lowered == "password" || lowered == "secret" || lowered == "url" || lowered == "baseurl" {
          *item = Value::String("<redacted>".into());
        } else if matches!(lowered.as_str(), "env" | "headers" | "http_headers" | "request_headers") {
          if let Some(values) = item.as_object_mut() {
            for value in values.values_mut() { *value = Value::String("<redacted>".into()); }
          } else { *item = Value::String("<redacted>".into()); }
        } else {
          redact_value(item);
        }
      }
    }
    Value::Array(items) => items.iter_mut().for_each(redact_value),
    _ => {}
  }
}

fn sanitize_workspace(workspace: &mut WorkspaceData) {
  for server in &mut workspace.mcp_servers {
    server.command = "<redacted>".into();
    server.args.clear();
    for value in server.env.values_mut() { *value = "<redacted>".into(); }
    if !server.url.is_empty() { server.url = "<redacted>".into(); }
  }
  for prompt in &mut workspace.prompts { prompt.content = "<redacted>".into(); }
  for skill in &mut workspace.skills { skill.source = "<redacted>".into(); }
  for endpoint in &mut workspace.proxy_settings.endpoints { endpoint.base_url = "<redacted>".into(); }
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
      if result.len() >= MAX_SKILL_FILES { return Err(AppError::Message("skill export contains too many files".into())); }
      let size = entry.metadata().map_err(|error| AppError::io(&entry.path(), error))?.len() as usize;
      if (*total).saturating_add(size) > MAX_SKILL_EXPORT_BYTES { return Err(AppError::Message("skill export exceeds 64 MB".into())); }
      let bytes = fs::read(entry.path()).map_err(|error| AppError::io(&entry.path(), error))?;
      *total = total.saturating_add(bytes.len());
      if *total > MAX_SKILL_EXPORT_BYTES { return Err(AppError::Message("skill export exceeds 64 MB".into())); }
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
  if passphrase.chars().count() < 8 { return Err(AppError::Message("encrypted export requires a passphrase of at least 8 characters".into())); }
  if envelope.rounds != PBKDF2_ROUNDS || envelope.salt.len() != 24 || envelope.nonce.len() != 16 {
    return Err(AppError::Message("unsupported encrypted export parameters".into()));
  }
  let salt = BASE64.decode(envelope.salt).map_err(|_| AppError::Message("invalid export salt".into()))?;
  let nonce = BASE64.decode(envelope.nonce).map_err(|_| AppError::Message("invalid export nonce".into()))?;
  let ciphertext = BASE64.decode(envelope.ciphertext).map_err(|_| AppError::Message("invalid export ciphertext".into()))?;
  if salt.len() != EXPORT_SALT_BYTES || nonce.len() != EXPORT_NONCE_BYTES { return Err(AppError::Message("invalid export nonce or salt length".into())); }
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
  let mut preferences = json_or_default(&preferences_path(), json!({}));
  let mut skill_files = Vec::new();
  let mut skill_bytes = 0usize;
  if include_secrets {
    collect_skill_files(&managed_skills_dir(), &managed_skills_dir(), &mut skill_files, &mut skill_bytes).map_err(|error| error.to_string())?;
  }
  if !include_secrets {
    redact_value(&mut profiles);
    let mut workspace_value = serde_json::to_value(&workspace).map_err(|error| error.to_string())?;
    redact_value(&mut workspace_value);
    workspace = serde_json::from_value(workspace_value).map_err(|error| error.to_string())?;
    sanitize_workspace(&mut workspace);
    redact_value(&mut preferences);
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
  if bytes.len() > MAX_EXPORT_FILE_BYTES { return Err("config export exceeds 128 MB".into()); }
  let directory = exports_dir();
  fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
  let suffix = if encrypted { "encrypted.json" } else { "sanitized.json" };
  let path = directory.join(format!("ai8888-config-{}-{suffix}", now_ms()));
  fs::write(&path, &bytes).map_err(|error| error.to_string())?;
  Ok(ExportResult { path: path.display().to_string(), encrypted, sanitized: !include_secrets, size_bytes: bytes.len() as u64 })
}

fn safe_relative_path(value: &str) -> Result<PathBuf, AppError> {
  if value.is_empty() || value.contains('\\') || value.contains('\0') || value.len() > 1_024 {
    return Err(AppError::Message("export contains an unsafe skill path".into()));
  }
  let path = PathBuf::from(value);
  if path.is_absolute() || path.components().any(|component| !matches!(component, Component::Normal(_))) {
    return Err(AppError::Message("export contains an unsafe skill path".into()));
  }
  Ok(path)
}

struct PreparedSkillFile {
  relative: PathBuf,
  content: Vec<u8>,
}

fn prepare_skill_files(files: Vec<ExportFile>) -> Result<Vec<PreparedSkillFile>, AppError> {
  if files.len() > MAX_SKILL_FILES { return Err(AppError::Message("config export contains too many skill files".into())); }
  let mut prepared = Vec::with_capacity(files.len());
  let mut total = 0usize;
  let mut paths = HashSet::new();
  for file in files {
    let relative = safe_relative_path(&file.path)?;
    let key = relative.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
    if !paths.insert(key) { return Err(AppError::Message("config export contains duplicate skill paths".into())); }
    let content = BASE64.decode(file.content).map_err(|_| AppError::Message("Invalid base64 skill content".into()))?;
    total = total.checked_add(content.len()).ok_or_else(|| AppError::Message("skill export size overflow".into()))?;
    if total > MAX_SKILL_EXPORT_BYTES { return Err(AppError::Message("skill export exceeds 64 MB".into())); }
    prepared.push(PreparedSkillFile { relative, content });
  }
  for file in &prepared {
    let mut parent = file.relative.parent();
    while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
      let key = path.to_string_lossy().replace('\\', "/").to_ascii_lowercase();
      if paths.contains(&key) { return Err(AppError::Message("config export contains a file/directory path conflict".into())); }
      parent = path.parent();
    }
  }
  Ok(prepared)
}

fn random_sibling_path(prefix: &str) -> PathBuf {
  let suffix = rand::random::<u64>();
  app_dir().join(format!(".{prefix}-{suffix:016x}"))
}

fn remove_path(path: &Path) -> Result<(), AppError> {
  let metadata = match fs::symlink_metadata(path) {
    Ok(metadata) => metadata,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
    Err(error) => return Err(AppError::io(path, error)),
  };
  if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
    fs::remove_dir_all(path).map_err(|error| AppError::io(path, error))
  } else {
    fs::remove_file(path).map_err(|error| AppError::io(path, error))
  }
}

fn stage_skill_files(files: &[PreparedSkillFile]) -> Result<PathBuf, AppError> {
  let staging = random_sibling_path("skills-import");
  let result = (|| {
    fs::create_dir(&staging).map_err(|error| AppError::io(&staging, error))?;
    for file in files {
      let destination = staging.join(&file.relative);
      if let Some(parent) = destination.parent() { fs::create_dir_all(parent).map_err(|error| AppError::io(parent, error))?; }
      fs::write(&destination, &file.content).map_err(|error| AppError::io(&destination, error))?;
    }
    Ok::<(), AppError>(())
  })();
  if let Err(error) = result {
    let _ = remove_path(&staging);
    return Err(error);
  }
  Ok(staging)
}

struct FileSnapshot {
  path: PathBuf,
  content: Option<Vec<u8>>,
}

fn snapshot_files(paths: &[PathBuf]) -> Result<Vec<FileSnapshot>, AppError> {
  paths.iter().map(|path| {
    let content = match fs::read(path) {
      Ok(content) => Some(content),
      Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
      Err(error) => return Err(AppError::io(path, error)),
    };
    Ok(FileSnapshot { path: path.clone(), content })
  }).collect()
}

fn restore_files(snapshots: &[FileSnapshot]) -> Result<(), AppError> {
  for snapshot in snapshots {
    match &snapshot.content {
      Some(content) => atomic_write(&snapshot.path, content)?,
      None => remove_path(&snapshot.path)?,
    }
  }
  Ok(())
}

fn activate_skill_tree(staging: &Path, root: &Path, backup: &Path) -> Result<bool, AppError> {
  let had_previous = fs::symlink_metadata(root).is_ok();
  if had_previous { fs::rename(root, backup).map_err(|error| AppError::io(root, error))?; }
  if let Err(error) = fs::rename(staging, root) {
    if had_previous { let _ = fs::rename(backup, root); }
    return Err(AppError::io(root, error));
  }
  Ok(had_previous)
}

fn restore_skill_tree(root: &Path, backup: &Path, had_previous: bool) -> Result<(), AppError> {
  remove_path(root)?;
  if had_previous { fs::rename(backup, root).map_err(|error| AppError::io(root, error))?; }
  Ok(())
}

#[tauri::command]
pub fn app_import_config(path: String, passphrase: String) -> Result<WorkspaceData, String> {
  ensure_app_dir().map_err(|error| error.to_string())?;
  let path = PathBuf::from(path.trim());
  if !path.is_file() { return Err("Import file does not exist".into()); }
  let bytes = fs::read(&path).map_err(|error| error.to_string())?;
  if bytes.len() > MAX_EXPORT_FILE_BYTES { return Err("Import file exceeds 128 MB".into()); }
  let plain = match serde_json::from_slice::<EncryptedEnvelope>(&bytes) {
    Ok(envelope) if envelope.format == "ai8888-config-aes256gcm-v1" => decrypt_bundle(&bytes, &passphrase).map_err(|error| error.to_string())?,
    _ => bytes,
  };
  let bundle: ConfigBundle = serde_json::from_slice(&plain).map_err(|error| format!("Invalid config export: {error}"))?;
  if bundle.schema_version > EXPORT_SCHEMA_VERSION { return Err("Config export is newer than this application".into()); }
  if bundle.sanitized { return Err("Sanitized exports are for sharing and cannot restore redacted secrets".into()); }
  validate_workspace_data(&bundle.workspace).map_err(|error| error.to_string())?;
  let prepared_skills = prepare_skill_files(bundle.skill_files).map_err(|error| error.to_string())?;
  let config_values = [
    (profiles_path(), serde_json::to_vec_pretty(&bundle.profiles).map_err(|error| error.to_string())?),
    (workspace_path(), serde_json::to_vec_pretty(&bundle.workspace).map_err(|error| error.to_string())?),
    (preferences_path(), serde_json::to_vec_pretty(&bundle.preferences).map_err(|error| error.to_string())?),
    (local_route_manifest_path(), serde_json::to_vec_pretty(&bundle.local_routing).map_err(|error| error.to_string())?),
  ];
  let snapshots = snapshot_files(&config_values.iter().map(|(path, _)| path.clone()).collect::<Vec<_>>()).map_err(|error| error.to_string())?;
  let staging = stage_skill_files(&prepared_skills).map_err(|error| error.to_string())?;
  let skills_root = managed_skills_dir();
  let skill_backup = random_sibling_path("skills-before-import");
  for (path, content) in &config_values {
    if let Err(error) = atomic_write(path, content) {
      let _ = restore_files(&snapshots);
      let _ = remove_path(&staging);
      return Err(error.to_string());
    }
  }
  let had_previous = match activate_skill_tree(&staging, &skills_root, &skill_backup) {
    Ok(value) => value,
    Err(error) => {
      let _ = restore_files(&snapshots);
      let _ = remove_path(&staging);
      return Err(error.to_string());
    }
  };
  if let Err(error) = sync_workspace_extensions(&bundle.workspace) {
    let mut rollback_errors = Vec::new();
    if let Err(rollback) = restore_files(&snapshots) { rollback_errors.push(rollback.to_string()); }
    if let Err(rollback) = restore_skill_tree(&skills_root, &skill_backup, had_previous) { rollback_errors.push(rollback.to_string()); }
    if rollback_errors.is_empty() {
      if let Ok(previous) = load_workspace() {
        if let Err(rollback) = sync_workspace_extensions(&previous) { rollback_errors.push(rollback.to_string()); }
      }
    }
    if !rollback_errors.is_empty() { return Err(format!("{error}; import rollback incomplete: {}", rollback_errors.join("; "))); }
    return Err(error.to_string());
  }
  if let Err(error) = remove_path(&skill_backup) {
    return Err(format!("configuration imported, but the previous Skills backup could not be removed: {error}"));
  }
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn encrypted_export_round_trips_and_rejects_wrong_password() {
    let plain = br#"{"profile":"secret"}"#;
    let encrypted = encrypt_bundle(plain, "correct-password").expect("encrypt bundle");
    assert_ne!(encrypted, plain);
    assert_eq!(decrypt_bundle(&encrypted, "correct-password").expect("decrypt bundle"), plain);
    assert!(decrypt_bundle(&encrypted, "wrong-password").is_err());
  }

  #[test]
  fn encrypted_import_rejects_untrusted_kdf_parameters() {
    let envelope = EncryptedEnvelope {
      format: "ai8888-config-aes256gcm-v1".into(),
      rounds: PBKDF2_ROUNDS + 1,
      salt: BASE64.encode([0u8; EXPORT_SALT_BYTES]),
      nonce: BASE64.encode([0u8; EXPORT_NONCE_BYTES]),
      ciphertext: String::new(),
    };
    let bytes = serde_json::to_vec(&envelope).expect("serialize envelope");
    let error = decrypt_bundle(&bytes, "correct-password").expect_err("invalid KDF parameters must be rejected");
    assert!(error.to_string().contains("unsupported encrypted export parameters"));
  }

  #[test]
  fn sanitized_exports_remove_nested_credentials() {
    let mut value = json!({
      "apiKey": "sk-secret",
      "nested": { "refresh_token": "refresh", "env": { "TOKEN": "value" } },
      "safe": "keep"
    });
    redact_value(&mut value);
    assert_eq!(value["apiKey"], "<redacted>");
    assert_eq!(value["nested"]["refresh_token"], "<redacted>");
    assert_eq!(value["nested"]["env"]["TOKEN"], "<redacted>");
    assert_eq!(value["safe"], "keep");
  }

  #[test]
  fn imported_skill_paths_cannot_escape_the_managed_root() {
    assert!(safe_relative_path("skill/SKILL.md").is_ok());
    assert!(safe_relative_path("../secret").is_err());
    assert!(safe_relative_path("/absolute").is_err());
  }

  #[test]
  fn imported_skill_files_are_validated_before_writes() {
    let files = vec![
      ExportFile { path: "skill/SKILL.md".into(), content: BASE64.encode(b"skill") },
      ExportFile { path: "skill/../outside".into(), content: BASE64.encode(b"outside") },
    ];
    assert!(prepare_skill_files(files).is_err());
    let duplicate = vec![
      ExportFile { path: "skill/SKILL.md".into(), content: BASE64.encode(b"one") },
      ExportFile { path: "SKILL/skill.md".into(), content: BASE64.encode(b"two") },
    ];
    assert!(prepare_skill_files(duplicate).is_err());
  }
}
