use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::error::AppError;
use serde::{de::DeserializeOwned, Serialize};

pub const APP_NAME: &str = "ai8888-switch";
pub const API_BASE_URL: &str = "https://sub.ai8888.shop/api/v1";
pub const OPENAI_BASE_URL: &str = "https://sub.ai8888.shop/v1";
pub const SITE_BASE_URL: &str = "https://sub.ai8888.shop";
pub const PURCHASE_URL: &str = "https://sub.ai8888.shop/purchase";
pub const RADAR_URL: &str = "https://codexradar.com/assets/radar-high-readout-comic.png";
pub const MODEL_STATUS_URL: &str = "https://status.ai8888.shop/";
pub const LOCAL_PROXY_BASE_URL: &str = "http://127.0.0.1:15888";
pub const LOCAL_PROXY_OPENAI_BASE_URL: &str = "http://127.0.0.1:15888/v1";
pub const LOCAL_PROXY_PROFILE_NAME: &str = "ai8888-local-route";

pub fn home_dir() -> PathBuf {
  #[cfg(test)]
  if let Some(path) = std::env::var_os("AI8888_SWITCH_TEST_HOME") {
    return PathBuf::from(path);
  }
  dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}


pub fn app_dir() -> PathBuf {
  home_dir().join(format!(".{APP_NAME}"))
}

pub fn state_path() -> PathBuf {
  app_dir().join("state.json")
}

pub fn local_route_manifest_path() -> PathBuf {
  app_dir().join("local-routing.json")
}

pub fn preferences_path() -> PathBuf {
  app_dir().join("preferences.json")
}

pub fn updates_dir() -> PathBuf {
  app_dir().join("updates")
}

pub fn ensure_app_dir() -> Result<PathBuf, AppError> {
  let dir = app_dir();
  fs::create_dir_all(&dir).map_err(|err| AppError::io(&dir, err))?;
  Ok(dir)
}

pub fn path_for(tool: &str, file: &str) -> PathBuf {
  let dir = match tool {
    "codex" => home_dir().join(".codex"),
    "claude" => home_dir().join(".claude"),
    "gemini" => home_dir().join(".gemini"),
    "opencode" => home_dir().join(".config").join("opencode"),
    "openclaw" => home_dir().join(".openclaw"),
    "hermes" => home_dir().join(".hermes"),
    _ => app_dir().join(tool),
  };

  if file.is_empty() {
    dir
  } else {
    dir.join(file)
  }
}

fn backup_path(path: &Path) -> PathBuf {
  let file_name = path
    .file_name()
    .and_then(|name| name.to_str())
    .unwrap_or("config");
  path.with_file_name(format!("{file_name}.ai8888-switch.bak"))
}

pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), AppError> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
  }

  let temp_path = path.with_file_name(format!(
    ".{}.tmp",
    path.file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("ai8888-switch")
  ));

  {
    let mut file = fs::File::create(&temp_path).map_err(|err| AppError::io(&temp_path, err))?;
    file.write_all(content).map_err(|err| AppError::io(&temp_path, err))?;
    file.flush().map_err(|err| AppError::io(&temp_path, err))?;
  }

  if path.exists() {
    let backup = backup_path(path);
    let _ = fs::copy(path, backup);
    fs::remove_file(path).map_err(|err| AppError::io(path, err))?;
  }

  fs::rename(&temp_path, path).map_err(|err| AppError::io(path, err))?;
  Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), AppError> {
  let bytes = serde_json::to_vec_pretty(value).map_err(|err| AppError::Message(err.to_string()))?;
  atomic_write(path, &bytes)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, AppError> {
  let content = fs::read_to_string(path).map_err(|err| AppError::io(path, err))?;
  serde_json::from_str(&content).map_err(|err| AppError::json(path, err))
}

pub fn write_text(path: &Path, content: &str) -> Result<(), AppError> {
  atomic_write(path, content.as_bytes())
}

pub fn normalize_base_url(url: &str) -> String {
  url.trim().trim_end_matches('/').to_string()
}

pub fn normalize_api_base_url(url: &str) -> String {
  let trimmed = normalize_base_url(url);
  if trimmed.ends_with("/v1") {
    trimmed
  } else {
    format!("{trimmed}/v1")
  }
}
