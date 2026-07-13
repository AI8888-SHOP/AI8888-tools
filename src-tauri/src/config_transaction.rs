use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{atomic_write, config_history_dir};
use crate::error::AppError;
use crate::models::{ConfigSnapshotFile, ConfigSnapshotSummary};

const MAX_CONFIG_SNAPSHOTS: usize = 20;
static SNAPSHOT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotEntry {
  path: String,
  label: String,
  existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotManifest {
  id: String,
  created_at: u64,
  label: String,
  entries: Vec<SnapshotEntry>,
}

impl SnapshotManifest {
  fn summary(&self) -> ConfigSnapshotSummary {
    ConfigSnapshotSummary {
      id: self.id.clone(),
      created_at: self.created_at,
      label: self.label.clone(),
      files: self.entries.iter().map(|entry| ConfigSnapshotFile {
        path: entry.path.clone(),
        label: entry.label.clone(),
        existed: entry.existed,
      }).collect(),
    }
  }
}

fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or(0)
}

fn valid_snapshot_id(id: &str) -> bool {
  !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit() || byte == b'-')
}

fn snapshot_dir(id: &str) -> Result<PathBuf, AppError> {
  if !valid_snapshot_id(id) {
    return Err(AppError::Message("invalid configuration snapshot id".into()));
  }
  Ok(config_history_dir().join(id))
}

fn stored_file(root: &Path, index: usize) -> PathBuf {
  root.join("files").join(format!("{index:04}.bin"))
}

#[cfg(unix)]
fn restrict_permissions(path: &Path, mode: u32) -> Result<(), AppError> {
  use std::os::unix::fs::PermissionsExt;
  fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|err| AppError::io(path, err))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path, _mode: u32) -> Result<(), AppError> {
  Ok(())
}

fn load_manifest(id: &str) -> Result<SnapshotManifest, AppError> {
  let root = snapshot_dir(id)?;
  let path = root.join("manifest.json");
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  let manifest: SnapshotManifest = serde_json::from_str(&content).map_err(|err| AppError::json(&path, err))?;
  if manifest.id != id {
    return Err(AppError::Message("configuration snapshot manifest id mismatch".into()));
  }
  Ok(manifest)
}

fn cleanup_stale_snapshot_dirs(history: &Path) {
  let Ok(entries) = fs::read_dir(history) else { return; };
  for entry in entries.flatten() {
    let name = entry.file_name().to_string_lossy().to_string();
    if name.starts_with('.') && name.ends_with(".tmp") && entry.path().is_dir() {
      let _ = fs::remove_dir_all(entry.path());
    }
  }
}

pub fn create_snapshot(paths: &[(PathBuf, String)], label: &str) -> Result<ConfigSnapshotSummary, AppError> {
  let history = config_history_dir();
  fs::create_dir_all(&history).map_err(|err| AppError::io(&history, err))?;
  restrict_permissions(&history, 0o700)?;
  cleanup_stale_snapshot_dirs(&history);
  let created_at = now_ms();
  let id = format!("{created_at}-{}-{}", std::process::id(), SNAPSHOT_SEQUENCE.fetch_add(1, Ordering::Relaxed));
  let root = history.join(&id);
  let temp = history.join(format!(".{id}.tmp"));
  if temp.exists() {
    fs::remove_dir_all(&temp).map_err(|err| AppError::io(&temp, err))?;
  }
  fs::create_dir_all(temp.join("files")).map_err(|err| AppError::io(&temp, err))?;
  restrict_permissions(&temp, 0o700)?;
  restrict_permissions(&temp.join("files"), 0o700)?;

  let mut seen = HashSet::new();
  let mut entries = Vec::new();
  for (path, file_label) in paths {
    let path_text = path.display().to_string();
    if !seen.insert(path_text.clone()) {
      continue;
    }
    let existed = path.exists();
    if existed {
      if !path.is_file() {
        let _ = fs::remove_dir_all(&temp);
        return Err(AppError::Message(format!("managed configuration path is not a file: {}", path.display())));
      }
      let destination = stored_file(&temp, entries.len());
      fs::copy(path, &destination).map_err(|err| AppError::io(path, err))?;
      restrict_permissions(&destination, 0o600)?;
    }
    entries.push(SnapshotEntry { path: path_text, label: file_label.clone(), existed });
  }

  let manifest = SnapshotManifest {
    id: id.clone(),
    created_at,
    label: label.to_string(),
    entries,
  };
  let manifest_path = temp.join("manifest.json");
  let bytes = serde_json::to_vec_pretty(&manifest).map_err(|err| AppError::Message(err.to_string()))?;
  fs::write(&manifest_path, bytes).map_err(|err| AppError::io(&manifest_path, err))?;
  restrict_permissions(&manifest_path, 0o600)?;
  fs::rename(&temp, &root).map_err(|err| AppError::io(&root, err))?;
  Ok(manifest.summary())
}

fn validate_manifest_paths(manifest: &SnapshotManifest, allowed_paths: &[PathBuf]) -> Result<(), AppError> {
  for entry in &manifest.entries {
    let path = PathBuf::from(&entry.path);
    if !allowed_paths.iter().any(|allowed| allowed == &path) {
      return Err(AppError::Message(format!("configuration snapshot contains an unmanaged path: {}", path.display())));
    }
  }
  Ok(())
}

fn apply_manifest(manifest: &SnapshotManifest, allowed_paths: &[PathBuf]) -> Result<(), AppError> {
  validate_manifest_paths(manifest, allowed_paths)?;
  let root = snapshot_dir(&manifest.id)?;
  for (index, entry) in manifest.entries.iter().enumerate() {
    let path = PathBuf::from(&entry.path);
    if entry.existed {
      let stored = stored_file(&root, index);
      let bytes = fs::read(&stored).map_err(|err| AppError::io(&stored, err))?;
      atomic_write(&path, &bytes)?;
    } else if path.exists() {
      fs::remove_file(&path).map_err(|err| AppError::io(&path, err))?;
    }
  }
  Ok(())
}

pub fn rollback_failed_transaction(snapshot_id: &str, allowed_paths: &[PathBuf]) -> Result<(), AppError> {
  let manifest = load_manifest(snapshot_id)?;
  apply_manifest(&manifest, allowed_paths)
}

pub fn restore_snapshot(snapshot_id: &str, allowed_paths: &[PathBuf]) -> Result<(ConfigSnapshotSummary, ConfigSnapshotSummary), AppError> {
  let target = load_manifest(snapshot_id)?;
  validate_manifest_paths(&target, allowed_paths)?;
  let current_paths = target.entries.iter().map(|entry| (PathBuf::from(&entry.path), entry.label.clone())).collect::<Vec<_>>();
  let recovery = create_snapshot(&current_paths, &format!("回滚操作前（目标：{}）", target.label))?;

  if let Err(error) = apply_manifest(&target, allowed_paths) {
    let recovery_manifest = load_manifest(&recovery.id)?;
    return match apply_manifest(&recovery_manifest, allowed_paths) {
      Ok(()) => Err(AppError::Message(format!("configuration rollback failed and the pre-rollback state was restored: {error}"))),
      Err(recovery_error) => Err(AppError::Message(format!("configuration rollback failed: {error}; recovery also failed: {recovery_error}; recovery snapshot {} was retained", recovery.id))),
    };
  }
  prune_snapshots(&[target.id.as_str(), recovery.id.as_str()])?;
  Ok((target.summary(), recovery))
}

pub fn remove_snapshot(snapshot_id: &str) -> Result<(), AppError> {
  let root = snapshot_dir(snapshot_id)?;
  if root.exists() {
    fs::remove_dir_all(&root).map_err(|err| AppError::io(&root, err))?;
  }
  Ok(())
}

pub fn list_snapshots() -> Result<Vec<ConfigSnapshotSummary>, AppError> {
  let history = config_history_dir();
  if !history.exists() {
    return Ok(Vec::new());
  }
  let entries = fs::read_dir(&history).map_err(|err| AppError::io(&history, err))?;
  let mut snapshots = entries
    .flatten()
    .filter_map(|entry| {
      let id = entry.file_name().to_string_lossy().to_string();
      if !valid_snapshot_id(&id) || !entry.path().is_dir() {
        return None;
      }
      load_manifest(&id).ok().map(|manifest| manifest.summary())
    })
    .collect::<Vec<_>>();
  snapshots.sort_by(|left, right| right.created_at.cmp(&left.created_at));
  Ok(snapshots)
}

pub fn prune_snapshots(protected_ids: &[&str]) -> Result<(), AppError> {
  let snapshots = list_snapshots()?;
  let protected = protected_ids.iter().copied().collect::<HashSet<_>>();
  let mut remaining = snapshots.len();
  for snapshot in snapshots.into_iter().rev() {
    if remaining <= MAX_CONFIG_SNAPSHOTS {
      break;
    }
    if protected.contains(snapshot.id.as_str()) {
      continue;
    }
    remove_snapshot(&snapshot.id)?;
    remaining -= 1;
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn snapshots_and_restores_existing_and_new_files() {
    let _guard = crate::config::test_home_guard();
    let root = std::env::temp_dir().join(format!("ai8888-config-transaction-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&root).expect("create temp root");
    let old_home = std::env::var_os("AI8888_SWITCH_TEST_HOME");
    std::env::set_var("AI8888_SWITCH_TEST_HOME", &root);
    let existing = root.join("existing.json");
    let created = root.join("created.json");
    fs::write(&existing, b"before").expect("seed existing");
    let paths = vec![(existing.clone(), "existing".into()), (created.clone(), "created".into())];
    let snapshot = create_snapshot(&paths, "before write").expect("snapshot");
    fs::write(&existing, b"after").expect("change existing");
    fs::write(&created, b"new").expect("create file");
    rollback_failed_transaction(&snapshot.id, &[existing.clone(), created.clone()]).expect("restore");
    assert_eq!(fs::read(&existing).expect("read existing"), b"before");
    assert!(!created.exists());
    let _ = fs::remove_dir_all(&root);
    match old_home {
      Some(value) => std::env::set_var("AI8888_SWITCH_TEST_HOME", value),
      None => std::env::remove_var("AI8888_SWITCH_TEST_HOME"),
    }
  }
}
