use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::AppError;
use serde::{de::DeserializeOwned, Serialize};

pub const APP_NAME: &str = "ai8888-switch";
pub const API_BASE_URL: &str = "https://sub.ai8888.shop/api/v1";
pub const OPENAI_BASE_URL: &str = "https://sub.ai8888.shop/v1";
pub const SITE_BASE_URL: &str = "https://sub.ai8888.shop";
pub const PURCHASE_URL: &str = "https://sub.ai8888.shop/purchase";
pub const REST_URL: &str = "https://rest.ai8888.shop";
pub const RADAR_URL: &str = "https://codexradar.com/assets/radar-high-readout-comic.png";
pub const MODEL_STATUS_URL: &str = "https://status.ai8888.shop/";
pub const LOCAL_PROXY_BASE_URL: &str = "http://127.0.0.1:15888";
pub const LOCAL_PROXY_OPENAI_BASE_URL: &str = "http://127.0.0.1:15888/v1";
pub const LOCAL_PROXY_PROFILE_NAME: &str = "ai8888-local-route";
static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

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

pub fn config_history_dir() -> PathBuf {
  app_dir().join("config-history")
}

pub fn profiles_path() -> PathBuf {
  app_dir().join("profiles.json")
}

pub fn ensure_app_dir() -> Result<PathBuf, AppError> {
  let dir = app_dir();
  fs::create_dir_all(&dir).map_err(|err| AppError::io(&dir, err))?;
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|err| AppError::io(&dir, err))?;
  }
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
  let path = if path.exists() && fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_symlink()).unwrap_or(false) {
    path.canonicalize().map_err(|err| AppError::io(path, err))?
  } else {
    path.to_path_buf()
  };
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
  }

  let temp_path = path.with_file_name(format!(
    ".{}.{}.{}.tmp",
    path.file_name()
      .and_then(|name| name.to_str())
      .unwrap_or("ai8888-switch"),
    std::process::id(),
    TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed),
  ));

  {
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp_path).map_err(|err| AppError::io(&temp_path, err))?;
    file.write_all(content).map_err(|err| AppError::io(&temp_path, err))?;
    file.flush().map_err(|err| AppError::io(&temp_path, err))?;
    file.sync_all().map_err(|err| AppError::io(&temp_path, err))?;
  }

  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600)).map_err(|err| AppError::io(&temp_path, err))?;
  }

  let existed = path.exists();
  if path.exists() {
    let backup = backup_path(&path);
    fs::copy(&path, &backup).map_err(|err| AppError::io(&backup, err))?;
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      fs::set_permissions(&backup, fs::Permissions::from_mode(0o600)).map_err(|err| AppError::io(&backup, err))?;
    }
  }

  if let Err(error) = replace_file(&temp_path, &path, existed) {
    let _ = fs::remove_file(&temp_path);
    return Err(error);
  }
  #[cfg(unix)]
  if let Some(parent) = path.parent() {
    let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
  }
  Ok(())
}

#[cfg(windows)]
fn replace_file(temp_path: &Path, path: &Path, existed: bool) -> Result<(), AppError> {
  use std::iter::once;
  use std::os::windows::ffi::OsStrExt;
  use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACEFILE_WRITE_THROUGH};

  let wide = |value: &Path| value.as_os_str().encode_wide().chain(once(0)).collect::<Vec<_>>();
  let temp = wide(temp_path);
  let target = wide(path);
  let success = unsafe {
    if existed {
      ReplaceFileW(target.as_ptr(), temp.as_ptr(), std::ptr::null(), REPLACEFILE_WRITE_THROUGH, std::ptr::null(), std::ptr::null())
    } else {
      MoveFileExW(temp.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH)
    }
  };
  if success == 0 {
    Err(AppError::io(path, std::io::Error::last_os_error()))
  } else {
    Ok(())
  }
}

#[cfg(not(windows))]
fn replace_file(temp_path: &Path, path: &Path, _existed: bool) -> Result<(), AppError> {
  fs::rename(temp_path, path).map_err(|err| AppError::io(path, err))
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

#[cfg(test)]
pub fn test_home_guard() -> std::sync::MutexGuard<'static, ()> {
  use std::sync::{Mutex, OnceLock};
  static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
  LOCK.get_or_init(|| Mutex::new(())).lock().expect("test home lock")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn atomic_write_replaces_an_existing_file() {
    let root = std::env::temp_dir().join(format!("ai8888-atomic-write-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temp root");
    let path = root.join("config.json");
    fs::write(&path, b"before").expect("seed file");
    atomic_write(&path, b"after").expect("atomic replace");
    assert_eq!(fs::read(&path).expect("read replaced file"), b"after");
    assert_eq!(fs::read(backup_path(&path)).expect("read backup"), b"before");
    let _ = fs::remove_dir_all(root);
  }

  #[cfg(unix)]
  #[test]
  fn atomic_write_preserves_symlinks_and_restricts_file_permissions() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = std::env::temp_dir().join(format!("ai8888-atomic-symlink-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create temp root");
    let target = root.join("target.json");
    let link = root.join("link.json");
    fs::write(&target, b"before").expect("seed target");
    symlink(&target, &link).expect("create symlink");
    atomic_write(&link, b"after").expect("write through symlink");
    assert!(fs::symlink_metadata(&link).expect("link metadata").file_type().is_symlink());
    assert_eq!(fs::read(&target).expect("read target"), b"after");
    assert_eq!(fs::metadata(&target).expect("target metadata").permissions().mode() & 0o777, 0o600);
    let _ = fs::remove_dir_all(root);
  }
}
