mod api;
mod codex_auth;
mod codex_sessions;
mod config;
mod config_profiles;
mod config_transaction;
mod error;
mod local_proxy;
mod models;
mod tools;

use std::collections::HashMap;

use api::{ApiClient, CreateKeyPayload, LoginPayload, ModelsQuery, RefreshPayload, UpdateKeyPayload};
use codex_auth::{open_device_auth_page, CodexAuthManager, CodexAuthStatus};
use codex_sessions::{CodexSessionMessage, CodexSessionMeta, CodexSessionSearchHit, CodexSessionSearchRequest, CodexSessionVisibilityRepairOutcome, CodexSessionVisibilityRepairRequest};
use config::{ensure_app_dir, local_route_manifest_path, preferences_path, read_json, state_path, updates_dir, write_json, MODEL_STATUS_URL, PURCHASE_URL, RADAR_URL, REST_URL};
use config_profiles::{delete_profile, list_profiles, resolve_profile_target, save_profile};
use config_transaction::{create_snapshot, list_snapshots, prune_snapshots, remove_snapshot, restore_snapshot, rollback_failed_transaction};
use error::AppError;
use local_proxy::ensure_local_proxy;
use models::{AccountSummary, ApiKeySummary, AppPreferences, AppStateData, ConfigProfile, ConfigProfileInput, ConfigSnapshotSummary, ConfigTransactionResult, EndpointProbeSummary, GroupSummary, LocalRouteManifest, LocalRouteStatus, LoginResult, ModelSummary, Pagination, StoredSession, SubscriptionSummary, SwitchTarget, ToolProfile, UpdateCheckResult, UpdateDownloadProgress, UpdateInstallResult};
use sha2::{Digest, Sha256};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State};
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Mutex, RwLock};
use tools::{activate_codex_official, all_managed_config_paths, build_tool_preview, cleanup_local_route_takeover, default_switch_target, detect_local_route_statuses, managed_paths_for_route_cleanup, managed_paths_for_target, restore_local_route_backups, supported_tools, write_local_routed_targets, ToolKind};

const CURRENT_APP_VERSION: &str = "v0.0.7";
const GITHUB_UPDATE_REPOSITORY: &str = "AI8888-SHOP/AI8888-tools";
const TRAY_ID: &str = "main-tray";
const TRAY_SHOW_ID: &str = "tray-show";
const TRAY_QUIT_ID: &str = "tray-quit";

struct UpdateDownloadControl {
  task_id: String,
  cancel: watch::Sender<bool>,
}

pub struct SharedState {
  pub api: ApiClient,
  pub data: RwLock<AppStateData>,
  codex_auth: CodexAuthManager,
  update_download: std::sync::Mutex<Option<UpdateDownloadControl>>,
  config_transaction: Mutex<()>,
}

impl SharedState {
  pub fn new() -> Result<Self, AppError> {
    Ok(Self {
      api: ApiClient::new()?,
      data: RwLock::new(AppStateData {
        selected_tool: ToolKind::Codex.as_str().to_string(),
        ..Default::default()
      }),
      codex_auth: CodexAuthManager::new(),
      update_download: std::sync::Mutex::new(None),
      config_transaction: Mutex::new(()),
    })
  }
}

fn persist_state(data: &AppStateData) -> Result<(), AppError> {
  ensure_app_dir()?;
  write_json(&state_path(), data)
}

fn load_state() -> Option<AppStateData> {
  read_json(&state_path()).ok()
}

#[tauri::command]
async fn app_get_state(state: State<'_, SharedState>) -> Result<AppStateData, String> {
  Ok(state.data.read().await.clone())
}

#[tauri::command]
async fn app_get_tools() -> Result<Vec<ToolProfile>, String> {
  Ok(supported_tools())
}


#[tauri::command]
fn app_get_codex_auth_status(state: State<'_, SharedState>) -> CodexAuthStatus {
  state.codex_auth.status()
}

#[tauri::command]
fn app_start_codex_login(state: State<'_, SharedState>, mode: String) -> Result<CodexAuthStatus, String> {
  state.codex_auth.start_login(&mode)
}

#[tauri::command]
fn app_cancel_codex_login(state: State<'_, SharedState>) -> Result<CodexAuthStatus, String> {
  state.codex_auth.cancel_login()
}

#[tauri::command]
fn app_logout_codex(state: State<'_, SharedState>) -> Result<CodexAuthStatus, String> {
  state.codex_auth.logout()
}

#[tauri::command]
fn app_open_codex_device_auth_page() -> Result<(), String> {
  open_device_auth_page()
}

#[tauri::command]
async fn app_activate_codex_official(state: State<'_, SharedState>) -> Result<ConfigTransactionResult, String> {
  if !state.codex_auth.status().authenticated {
    return Err("请先登录 OpenAI/ChatGPT 官方账户".into());
  }
  let _transaction_guard = state.config_transaction.lock().await;
  run_config_file_transaction("切换到 OpenAI 官方账户前", activate_codex_official)
}

#[tauri::command]
async fn app_check_update() -> Result<UpdateCheckResult, String> {
  let mainland_china = detect_mainland_china_exit_ip().await;
  let url = format!("https://api.github.com/repos/{GITHUB_UPDATE_REPOSITORY}/releases/latest");
  let client = reqwest::Client::builder()
    .user_agent("AI8888-tools-update-check")
    .build()
    .map_err(|err| err.to_string())?;

  let response = match client.get(&url).send().await {
    Ok(response) => response,
    Err(err) => {
      return Ok(UpdateCheckResult {
        current_version: CURRENT_APP_VERSION.to_string(),
        latest_version: None,
        update_available: false,
        release_url: Some(format!("https://github.com/{GITHUB_UPDATE_REPOSITORY}/releases")),
        download_url: None,
        download_accelerated: false,
        mainland_china,
        repository: GITHUB_UPDATE_REPOSITORY.to_string(),
        error: Some(err.to_string()),
      });
    }
  };

  let status = response.status();
  let text = response.text().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Ok(UpdateCheckResult {
      current_version: CURRENT_APP_VERSION.to_string(),
      latest_version: None,
      update_available: false,
      release_url: Some(format!("https://github.com/{GITHUB_UPDATE_REPOSITORY}/releases")),
      download_url: None,
      download_accelerated: false,
      mainland_china,
      repository: GITHUB_UPDATE_REPOSITORY.to_string(),
      error: Some(format!("GitHub releases returned {status}: {}", text.chars().take(200).collect::<String>())),
    });
  }

  let value: serde_json::Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
  let latest_version = value.get("tag_name").and_then(serde_json::Value::as_str).map(str::to_string);
  let release_url = value
    .get("html_url")
    .and_then(serde_json::Value::as_str)
    .map(str::to_string)
    .or_else(|| Some(format!("https://github.com/{GITHUB_UPDATE_REPOSITORY}/releases")));
  let update_available = latest_version
    .as_deref()
    .map(|version| compare_versions(version, CURRENT_APP_VERSION).is_gt())
    .unwrap_or(false);

  let original_download = select_release_download_url(&value);
  let (download_url, download_accelerated) = if update_available {
    match original_download {
      Some(url) if mainland_china => (Some(accelerate_github_download_url(&url)), true),
      Some(url) => (Some(url), false),
      None => (None, false),
    }
  } else {
    (None, false)
  };

  Ok(UpdateCheckResult {
    current_version: CURRENT_APP_VERSION.to_string(),
    latest_version,
    update_available,
    release_url,
    download_url,
    download_accelerated,
    mainland_china,
    repository: GITHUB_UPDATE_REPOSITORY.to_string(),
    error: None,
  })
}


fn load_preferences() -> AppPreferences {
  read_json(&preferences_path()).unwrap_or_default()
}

fn save_preferences(prefs: &AppPreferences) -> Result<(), AppError> {
  ensure_app_dir()?;
  write_json(&preferences_path(), prefs)
}

#[tauri::command]
async fn app_get_preferences() -> Result<AppPreferences, String> {
  Ok(load_preferences())
}

#[tauri::command]
async fn app_set_preferences(preferences: AppPreferences) -> Result<AppPreferences, String> {
  save_preferences(&preferences).map_err(|err| err.to_string())?;
  Ok(preferences)
}

#[tauri::command]
async fn app_complete_onboarding() -> Result<AppPreferences, String> {
  let mut prefs = load_preferences();
  prefs.onboarding_completed = true;
  prefs.onboarding_step = 0;
  save_preferences(&prefs).map_err(|err| err.to_string())?;
  Ok(prefs)
}

#[tauri::command]
async fn app_dismiss_alert(alert_id: String) -> Result<AppPreferences, String> {
  let mut prefs = load_preferences();
  if !alert_id.trim().is_empty() && !prefs.dismissed_alert_ids.iter().any(|item| item == &alert_id) {
    prefs.dismissed_alert_ids.push(alert_id);
    // Keep list bounded.
    if prefs.dismissed_alert_ids.len() > 100 {
      let overflow = prefs.dismissed_alert_ids.len() - 100;
      prefs.dismissed_alert_ids.drain(0..overflow);
    }
    save_preferences(&prefs).map_err(|err| err.to_string())?;
  }
  Ok(prefs)
}

#[tauri::command]
async fn app_install_update(
  app: tauri::AppHandle,
  state: State<'_, SharedState>,
  version: String,
  prefer_accelerated: bool,
) -> Result<UpdateInstallResult, String> {
  let version = version.trim();
  if version.is_empty() || version.chars().any(|ch| ch.is_control() || ch == '/' || ch == '\\') {
    return Err("invalid update version".into());
  }

  let task_id = format!("{}-{}", now_epoch_ms(), std::process::id());
  let (cancel_sender, cancel_receiver) = watch::channel(false);
  {
    let mut active = state.update_download.lock().map_err(|_| "update download state is unavailable".to_string())?;
    if active.is_some() {
      return Err("an update download is already running".into());
    }
    *active = Some(UpdateDownloadControl { task_id: task_id.clone(), cancel: cancel_sender });
  }
  emit_update_progress(&app, &task_id, "preparing", 0, 0, "正在准备更新");

  let outcome = run_update_install(&app, &task_id, version, prefer_accelerated, cancel_receiver).await;
  {
    let mut active = state.update_download.lock().map_err(|_| "update download state is unavailable".to_string())?;
    if active.as_ref().map(|item| item.task_id.as_str()) == Some(task_id.as_str()) {
      *active = None;
    }
  }

  match &outcome {
    Ok(result) => emit_update_progress(&app, &task_id, "completed", 1, 1, &result.message),
    Err(error) if error == UPDATE_CANCELED_ERROR => emit_update_progress(&app, &task_id, "canceled", 0, 0, "更新下载已取消"),
    Err(error) => emit_update_progress(&app, &task_id, "failed", 0, 0, error),
  }
  if outcome.as_ref().map(|result| result.launched).unwrap_or(false) {
    app.exit(0);
  }
  outcome
}

#[tauri::command]
fn app_cancel_update(state: State<'_, SharedState>) -> Result<bool, String> {
  let active = state.update_download.lock().map_err(|_| "update download state is unavailable".to_string())?;
  if let Some(control) = active.as_ref() {
    control.cancel.send(true).map_err(|_| "update download has already stopped".to_string())?;
    Ok(true)
  } else {
    Ok(false)
  }
}

const UPDATE_CANCELED_ERROR: &str = "update download canceled";

fn now_epoch_ms() -> u64 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|value| value.as_millis() as u64)
    .unwrap_or(0)
}

fn emit_update_progress(app: &tauri::AppHandle, task_id: &str, status: &str, downloaded_bytes: u64, total_bytes: u64, message: &str) {
  let percent = if total_bytes == 0 { 0.0 } else { (downloaded_bytes as f64 / total_bytes as f64 * 100.0).clamp(0.0, 100.0) };
  let _ = app.emit("update-download-progress", UpdateDownloadProgress {
    task_id: task_id.to_string(),
    status: status.to_string(),
    downloaded_bytes,
    total_bytes,
    percent,
    message: message.to_string(),
  });
}

async fn run_update_install(
  app: &tauri::AppHandle,
  task_id: &str,
  version: &str,
  prefer_accelerated: bool,
  mut cancel: watch::Receiver<bool>,
) -> Result<UpdateInstallResult, String> {
  let client = reqwest::Client::builder()
    .user_agent("AI8888-tools-update-install")
    .connect_timeout(std::time::Duration::from_secs(15))
    .timeout(std::time::Duration::from_secs(30 * 60))
    .build()
    .map_err(|err| err.to_string())?;
  let release_request = fetch_latest_update_release(&client);
  tokio::pin!(release_request);
  let release = tokio::select! {
    result = &mut release_request => result?,
    _ = cancel.changed() => return Err(UPDATE_CANCELED_ERROR.into()),
  };
  let latest_version = release
    .get("tag_name")
    .and_then(serde_json::Value::as_str)
    .ok_or_else(|| "latest GitHub release has no tag name".to_string())?;
  if latest_version != version {
    return Err(format!("requested update {version} is no longer the latest release ({latest_version})"));
  }

  let asset = select_release_asset(&release)
    .ok_or_else(|| format!("no installer asset is available for {}", current_os_family()))?;
  validate_release_asset(&asset, version)?;

  ensure_app_dir().map_err(|err| err.to_string())?;
  let updates = updates_dir();
  tokio::fs::create_dir_all(&updates).await.map_err(|err| err.to_string())?;
  cleanup_stale_update_parts(&updates).await;
  let target = updates.join(&asset.name);
  let partial = updates.join(format!(".{}.{}.part", asset.name, task_id));
  let accelerated_url = accelerate_github_download_url(&asset.download_url);
  let download = if prefer_accelerated {
    match download_update_asset(app, task_id, &client, &accelerated_url, asset.size, &partial, &mut cancel, "正在通过加速线路下载").await {
      Ok(digest) => Ok(digest),
      Err(error) if error == UPDATE_CANCELED_ERROR => Err(error),
      Err(accelerated_error) => {
        emit_update_progress(app, task_id, "fallback", 0, asset.size, "加速线路失败，正在切换直接下载");
        match download_update_asset(app, task_id, &client, &asset.download_url, asset.size, &partial, &mut cancel, "正在通过 GitHub 直接下载").await {
          Err(error) if error == UPDATE_CANCELED_ERROR => Err(error),
          Err(direct_error) => Err(format!("accelerated download failed ({accelerated_error}); direct download failed ({direct_error})")),
          Ok(digest) => Ok(digest),
        }
      }
    }
  } else {
    download_update_asset(app, task_id, &client, &asset.download_url, asset.size, &partial, &mut cancel, "正在通过 GitHub 直接下载").await
  };
  let actual_digest = match download {
    Ok(digest) => digest,
    Err(error) => {
      let _ = tokio::fs::remove_file(&partial).await;
      return Err(error);
    }
  };
  emit_update_progress(app, task_id, "verifying", asset.size, asset.size, "正在校验安装包");
  if let Err(error) = verify_update_digest_hex(&actual_digest, asset.digest.as_deref()) {
    let _ = tokio::fs::remove_file(&partial).await;
    return Err(error);
  }
  if *cancel.borrow() {
    let _ = tokio::fs::remove_file(&partial).await;
    return Err(UPDATE_CANCELED_ERROR.into());
  }
  if target.exists() {
    if let Err(error) = tokio::fs::remove_file(&target).await {
      let _ = tokio::fs::remove_file(&partial).await;
      return Err(error.to_string());
    }
  }
  if let Err(error) = tokio::fs::rename(&partial, &target).await {
    let _ = tokio::fs::remove_file(&partial).await;
    return Err(error.to_string());
  }

  emit_update_progress(app, task_id, "launching", asset.size, asset.size, "安装包校验完成，正在启动");
  let launched = launch_installer(&target)?;
  Ok(UpdateInstallResult {
    success: true,
    installer_path: Some(target.display().to_string()),
    launched,
    message: if launched {
      "update verified; installer launched".into()
    } else {
      format!("update verified and downloaded to {}", target.display())
    },
  })
}

async fn cleanup_stale_update_parts(updates: &std::path::Path) {
  let Ok(mut entries) = tokio::fs::read_dir(updates).await else { return; };
  while let Ok(Some(entry)) = entries.next_entry().await {
    let file_name = entry.file_name();
    let name = file_name.to_string_lossy();
    let is_partial = name.starts_with('.') && name.ends_with(".part");
    if is_partial {
      let _ = tokio::fs::remove_file(entry.path()).await;
    }
  }
}

fn launch_installer(path: &std::path::Path) -> Result<bool, String> {
  #[cfg(target_os = "windows")]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;
    let lower = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut command = if lower == "msi" {
      let mut cmd = std::process::Command::new("msiexec");
      cmd.arg("/i").arg(path);
      cmd
    } else {
      let cmd = std::process::Command::new(path);
      cmd
    };
    command.creation_flags(CREATE_NEW_CONSOLE);
    match command.spawn() {
      Ok(_) => Ok(true),
      Err(err) => Err(format!("安装包启动失败: {err}")),
    }
  }

  #[cfg(target_os = "macos")]
  {
    match std::process::Command::new("open").arg(path).status() {
      Ok(status) if status.success() => Ok(true),
      Ok(status) => Err(format!("安装包打开失败，open 退出状态: {status}")),
      Err(err) => Err(format!("安装包打开失败: {err}")),
    }
  }

  #[cfg(target_os = "linux")]
  {
    use std::os::unix::fs::PermissionsExt;
    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    if name.ends_with(".appimage") {
      let metadata = std::fs::metadata(path).map_err(|err| format!("无法读取 AppImage 权限: {err}"))?;
      let mut permissions = metadata.permissions();
      permissions.set_mode(permissions.mode() | 0o111);
      std::fs::set_permissions(path, permissions).map_err(|err| format!("无法设置 AppImage 执行权限: {err}"))?;
      return std::process::Command::new(path)
        .spawn()
        .map(|_| true)
        .map_err(|err| format!("AppImage 启动失败: {err}"));
    }
    if name.ends_with(".deb") || name.ends_with(".rpm") {
      return std::process::Command::new("xdg-open")
        .arg(path)
        .status()
        .map_err(|err| format!("系统安装器打开失败: {err}"))
        .and_then(|status| status.success().then_some(true).ok_or_else(|| format!("系统安装器打开失败，xdg-open 退出状态: {status}")));
    }
    Err("不支持当前 Linux 更新包格式".into())
  }

  #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
  {
    let _ = path;
    Ok(false)
  }
}

#[tauri::command]
async fn app_search_codex_sessions(request: CodexSessionSearchRequest) -> Result<Vec<CodexSessionSearchHit>, String> {
  tauri::async_runtime::spawn_blocking(move || codex_sessions::search_sessions(&request))
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())
}

const GITHUB_DOWNLOAD_ACCELERATOR_PREFIX: &str = "https://gh.jasonzeng.dev/";

fn accelerate_github_download_url(url: &str) -> String {
  let trimmed = url.trim();
  if trimmed.is_empty() {
    return String::new();
  }
  if trimmed.starts_with(GITHUB_DOWNLOAD_ACCELERATOR_PREFIX) {
    return trimmed.to_string();
  }
  format!("{GITHUB_DOWNLOAD_ACCELERATOR_PREFIX}{trimmed}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReleaseAsset {
  name: String,
  download_url: String,
  size: u64,
  digest: Option<String>,
}

async fn fetch_latest_update_release(client: &reqwest::Client) -> Result<serde_json::Value, String> {
  let url = format!("https://api.github.com/repos/{GITHUB_UPDATE_REPOSITORY}/releases/latest");
  let response = client.get(url).send().await.map_err(|err| err.to_string())?;
  let status = response.status();
  let text = response.text().await.map_err(|err| err.to_string())?;
  if !status.is_success() {
    return Err(format!("GitHub releases returned {status}: {}", text.chars().take(200).collect::<String>()));
  }
  serde_json::from_str(&text).map_err(|err| err.to_string())
}

fn select_release_asset(release: &serde_json::Value) -> Option<ReleaseAsset> {
  let assets = release.get("assets").and_then(serde_json::Value::as_array)?;
  let mut candidates = Vec::new();
  for asset in assets {
    let name = asset.get("name").and_then(serde_json::Value::as_str).unwrap_or("");
    let download_url = asset.get("browser_download_url").and_then(serde_json::Value::as_str).unwrap_or("");
    let score = score_release_asset(&name.to_ascii_lowercase());
    if download_url.is_empty() || score <= 0 {
      continue;
    }
    candidates.push((score, ReleaseAsset {
      name: name.to_string(),
      download_url: download_url.to_string(),
      size: asset.get("size").and_then(serde_json::Value::as_u64).unwrap_or(0),
      digest: asset.get("digest").and_then(serde_json::Value::as_str).map(str::to_string),
    }));
  }
  candidates.sort_by(|left, right| right.0.cmp(&left.0));
  candidates.into_iter().map(|(_, asset)| asset).next()
}

fn select_release_download_url(release: &serde_json::Value) -> Option<String> {
  select_release_asset(release).map(|asset| asset.download_url)
}

async fn download_update_asset(
  app: &tauri::AppHandle,
  task_id: &str,
  client: &reqwest::Client,
  url: &str,
  expected_size: u64,
  partial: &std::path::Path,
  cancel: &mut watch::Receiver<bool>,
  message: &str,
) -> Result<String, String> {
  if *cancel.borrow() {
    return Err(UPDATE_CANCELED_ERROR.into());
  }
  let request = client.get(url).send();
  tokio::pin!(request);
  let mut response = tokio::select! {
    result = &mut request => result.map_err(|err| err.to_string())?,
    _ = cancel.changed() => return Err(UPDATE_CANCELED_ERROR.into()),
  };
  if !response.status().is_success() {
    return Err(format!("HTTP {}", response.status()));
  }
  if let Some(content_length) = response.content_length() {
    if content_length != expected_size {
      return Err(format!("installer content length mismatch: expected {expected_size}, received {content_length}"));
    }
  }

  let mut file = tokio::fs::File::create(partial).await.map_err(|err| err.to_string())?;
  let mut digest = Sha256::new();
  let mut downloaded = 0_u64;
  let mut last_emit = std::time::Instant::now() - std::time::Duration::from_secs(1);
  emit_update_progress(app, task_id, "downloading", 0, expected_size, message);
  loop {
    let chunk = tokio::select! {
      result = response.chunk() => result.map_err(|err| err.to_string())?,
      _ = cancel.changed() => return Err(UPDATE_CANCELED_ERROR.into()),
    };
    let Some(chunk) = chunk else { break; };
    downloaded = downloaded.saturating_add(chunk.len() as u64);
    if downloaded > expected_size {
      return Err(format!("installer exceeded expected size: expected {expected_size}, received at least {downloaded}"));
    }
    file.write_all(&chunk).await.map_err(|err| err.to_string())?;
    digest.update(&chunk);
    if last_emit.elapsed() >= std::time::Duration::from_millis(150) || downloaded == expected_size {
      emit_update_progress(app, task_id, "downloading", downloaded, expected_size, message);
      last_emit = std::time::Instant::now();
    }
  }
  file.flush().await.map_err(|err| err.to_string())?;
  file.sync_all().await.map_err(|err| err.to_string())?;
  if downloaded != expected_size {
    return Err(format!("installer size mismatch: expected {expected_size}, received {downloaded}"));
  }
  Ok(format!("{:x}", digest.finalize()))
}

fn validate_release_asset(asset: &ReleaseAsset, version: &str) -> Result<(), String> {
  let file_name = std::path::Path::new(&asset.name).file_name().and_then(|value| value.to_str());
  if asset.name.is_empty() || asset.name.contains(['/', '\\']) || file_name != Some(asset.name.as_str()) {
    return Err("invalid update asset name".into());
  }
  let expected_prefix = format!("https://github.com/{GITHUB_UPDATE_REPOSITORY}/releases/download/{version}/");
  if !asset.download_url.starts_with(&expected_prefix) {
    return Err("update asset URL is outside the configured GitHub repository or release".into());
  }
  if asset.size < 1024 {
    return Err("GitHub reported an invalid installer size".into());
  }
  expected_sha256(asset.digest.as_deref())?;
  Ok(())
}

fn expected_sha256(digest: Option<&str>) -> Result<&str, String> {
  let expected = digest
    .and_then(|value| value.strip_prefix("sha256:"))
    .ok_or_else(|| "GitHub did not provide a SHA-256 digest for the update asset".to_string())?;
  if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
    return Err("GitHub provided an invalid SHA-256 digest for the update asset".into());
  }
  Ok(expected)
}

fn verify_update_digest(bytes: &[u8], digest: Option<&str>) -> Result<(), String> {
  let actual = format!("{:x}", Sha256::digest(bytes));
  verify_update_digest_hex(&actual, digest)
}

fn verify_update_digest_hex(actual: &str, digest: Option<&str>) -> Result<(), String> {
  let expected = expected_sha256(digest)?;
  if actual.eq_ignore_ascii_case(expected) {
    Ok(())
  } else {
    Err("downloaded installer SHA-256 digest does not match GitHub metadata".into())
  }
}

fn current_os_family() -> &'static str {
  if cfg!(target_os = "windows") {
    "windows"
  } else if cfg!(target_os = "macos") {
    "macos"
  } else if cfg!(target_os = "linux") {
    "linux"
  } else {
    "other"
  }
}

fn score_release_asset(name: &str) -> i32 {
  score_release_asset_for(current_os_family(), std::env::consts::ARCH, name)
}

fn score_release_asset_for(os: &str, host_arch: &str, name: &str) -> i32 {
  let mut score = 0;

  let is_windows = name.ends_with(".msi")
    || name.ends_with("-setup.exe")
    || name.ends_with("setup.exe")
    || name.ends_with(".exe");
  let is_macos = name.ends_with(".dmg") || name.ends_with(".pkg");
  let is_linux = name.ends_with(".appimage")
    || name.ends_with(".deb")
    || name.ends_with(".rpm");

  match os {
    "windows" => {
      if name.ends_with("-setup.exe") || name.ends_with("setup.exe") {
        score += 120;
      } else if name.ends_with(".msi") {
        score += 110;
      } else if name.ends_with(".exe") {
        score += 100;
      } else {
        return 0;
      }
    }
    "macos" => {
      if name.ends_with(".dmg") {
        score += 120;
      } else if name.ends_with(".pkg") {
        score += 110;
      } else {
        return 0;
      }
    }
    "linux" => {
      let running_appimage = std::env::var_os("APPIMAGE").is_some();
      let debian_family = std::path::Path::new("/usr/bin/dpkg").exists();
      let rpm_family = std::path::Path::new("/usr/bin/rpm").exists();
      if name.ends_with(".appimage") {
        score += if running_appimage || (!debian_family && !rpm_family) { 120 } else { 100 };
      } else if name.ends_with(".deb") {
        score += if debian_family && !running_appimage { 120 } else { 90 };
      } else if name.ends_with(".rpm") {
        score += if rpm_family && !running_appimage { 120 } else { 90 };
      } else {
        return 0;
      }
    }
    _ => {
      if is_windows || is_macos || is_linux {
        score += 20;
      } else {
        return 0;
      }
    }
  }

  // Architecture preference.
  if host_arch == "x86_64" && (name.contains("x64") || name.contains("amd64") || name.contains("x86_64") || name.contains("win64")) {
    score += 20;
  }
  if host_arch == "aarch64" && (name.contains("arm64") || name.contains("aarch64")) {
    score += 20;
  }
  // Never install a package that explicitly targets another architecture.
  if host_arch == "x86_64" && (name.contains("arm64") || name.contains("aarch64")) && !name.contains("universal") {
    return 0;
  }
  if host_arch == "aarch64" && (name.contains("x64") || name.contains("amd64") || name.contains("x86_64")) && !name.contains("universal") {
    return 0;
  }
  if name.contains("universal") {
    score += 15;
  }
  if name.contains("ai8888") || name.contains("switch") {
    score += 5;
  }
  score
}

#[cfg(test)]
mod update_download_tests {
  use sha2::{Digest, Sha256};
  use super::{accelerate_github_download_url, score_release_asset, score_release_asset_for, validate_release_asset, verify_update_digest, ReleaseAsset, GITHUB_DOWNLOAD_ACCELERATOR_PREFIX};

  #[test]
  fn accelerates_github_download_url() {
    let original = "https://github.com/AI8888-SHOP/AI8888-tools/releases/download/v0.0.6/AI8888.Switch_0.0.6_x64-setup.exe";
    let accelerated = accelerate_github_download_url(original);
    assert_eq!(accelerated, format!("{GITHUB_DOWNLOAD_ACCELERATOR_PREFIX}{original}"));
    assert_eq!(accelerate_github_download_url(&accelerated), accelerated);
  }

  #[test]
  fn scores_current_platform_installers_positive() {
    let samples = [
      "ai8888-switch_0.0.6_x64-setup.exe",
      "AI8888 Switch_0.0.6_x64_en-US.msi",
      "AI8888.Switch_0.0.6_x64.dmg",
      "ai8888-switch_0.0.6_amd64.AppImage",
      "ai8888-switch_0.0.6_amd64.deb",
      "ai8888-switch-0.0.6-1.x86_64.rpm",
    ];
    assert!(samples.iter().any(|name| score_release_asset(&name.to_ascii_lowercase()) > 50));
  }

  #[test]
  fn rejects_installers_for_another_operating_system_or_architecture() {
    assert!(score_release_asset_for("windows", "x86_64", "ai8888.switch_0.0.6_x64-setup.exe") > 0);
    assert!(score_release_asset_for("macos", "aarch64", "ai8888.switch_0.0.6_universal.dmg") > 0);
    assert!(score_release_asset_for("linux", "x86_64", "ai8888.switch_0.0.6_amd64.appimage") > 0);
    assert_eq!(score_release_asset_for("windows", "x86_64", "ai8888.switch_0.0.6_universal.dmg"), 0);
    assert_eq!(score_release_asset_for("macos", "aarch64", "ai8888.switch_0.0.6_x64-setup.exe"), 0);
    assert_eq!(score_release_asset_for("linux", "x86_64", "ai8888.switch_0.0.6_universal.dmg"), 0);
    assert_eq!(score_release_asset_for("windows", "aarch64", "ai8888.switch_0.0.6_x64-setup.exe"), 0);
    assert_eq!(score_release_asset_for("linux", "x86_64", "ai8888.switch_0.0.6_aarch64.appimage"), 0);
  }

  #[test]
  fn validates_repository_release_size_and_digest() {
    let digest = format!("sha256:{:x}", Sha256::digest(b"installer"));
    let asset = ReleaseAsset {
      name: "AI8888.Switch_0.0.6_x64-setup.exe".into(),
      download_url: "https://github.com/AI8888-SHOP/AI8888-tools/releases/download/v0.0.6/AI8888.Switch_0.0.6_x64-setup.exe".into(),
      size: 4096,
      digest: Some(digest),
    };
    assert!(validate_release_asset(&asset, "v0.0.6").is_ok());
    let mut wrong_repository = asset.clone();
    wrong_repository.download_url = wrong_repository.download_url.replace("AI8888-SHOP", "untrusted");
    assert!(validate_release_asset(&wrong_repository, "v0.0.6").is_err());
    let mut missing_digest = asset;
    missing_digest.digest = None;
    assert!(validate_release_asset(&missing_digest, "v0.0.6").is_err());
  }

  #[test]
  fn verifies_sha256_digest() {
    let digest = format!("sha256:{:x}", Sha256::digest(b"installer"));
    assert!(verify_update_digest(b"installer", Some(&digest)).is_ok());
    assert!(verify_update_digest(b"tampered", Some(&digest)).is_err());
  }
}

async fn detect_mainland_china_exit_ip() -> bool {
  let client = match reqwest::Client::builder()
    .user_agent("AI8888-tools-update-check")
    .timeout(std::time::Duration::from_secs(4))
    .build()
  {
    Ok(client) => client,
    Err(_) => return false,
  };

  // Prefer lightweight public endpoints; treat country code CN as mainland China.
  let endpoints = [
    "https://ipapi.co/country_code/",
    "https://ipinfo.io/country",
    "https://api.country.is/",
  ];

  for endpoint in endpoints {
    if let Ok(response) = client.get(endpoint).send().await {
      if !response.status().is_success() {
        continue;
      }
      if let Ok(body) = response.text().await {
        if country_code_is_mainland_china(&body) {
          return true;
        }
        // api.country.is returns JSON: {"ip":"...","country":"CN"}
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
          if let Some(code) = value.get("country").and_then(serde_json::Value::as_str) {
            if country_code_is_mainland_china(code) {
              return true;
            }
          }
        }
      }
    }
  }
  false
}

fn country_code_is_mainland_china(value: &str) -> bool {
  value
    .chars()
    .filter(|ch| ch.is_ascii_alphabetic())
    .collect::<String>()
    .eq_ignore_ascii_case("CN")
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
  let parse = |value: &str| {
    value
      .trim()
      .trim_start_matches('v')
      .split('.')
      .map(|part| part.parse::<u64>().unwrap_or(0))
      .collect::<Vec<_>>()
  };
  let left = parse(left);
  let right = parse(right);
  let len = left.len().max(right.len());
  for index in 0..len {
    let l = *left.get(index).unwrap_or(&0);
    let r = *right.get(index).unwrap_or(&0);
    match l.cmp(&r) {
      std::cmp::Ordering::Equal => continue,
      ordering => return ordering,
    }
  }
  std::cmp::Ordering::Equal
}
#[tauri::command]
async fn app_open_login_window(app: tauri::AppHandle, state: State<'_, SharedState>) -> Result<(), String> {
  let _ = state.api.ensure_best_endpoint().await;
  let login_url = state.api.login_url();
  {
    let mut guard = state.data.write().await;
    guard.login_window_open = true;
    persist_state(&guard).map_err(|err| err.to_string())?;
  }

  if app.get_webview_window("login").is_none() {
    tauri::WebviewWindowBuilder::new(
      &app,
      "login",
      tauri::WebviewUrl::External(login_url.parse::<tauri::Url>().map_err(|err| err.to_string())?),
    )
    .title("AI8888 Login")
    .inner_size(1100.0, 820.0)
    .visible(true)
    .build()
    .map_err(|err| err.to_string())?;
  } else if let Some(window) = app.get_webview_window("login") {
    let _ = window.show();
    let _ = window.set_focus();
  }
  Ok(())
}


fn open_external_window(app: tauri::AppHandle, label: &str, title: &str, url: &str, width: f64, height: f64) -> Result<(), String> {
  let parsed_url = url.parse::<tauri::Url>().map_err(|err| err.to_string())?;
  if app.get_webview_window(label).is_none() {
    tauri::WebviewWindowBuilder::new(
      &app,
      label,
      tauri::WebviewUrl::External(parsed_url),
    )
    .title(title)
    .inner_size(width, height)
    .visible(true)
    .build()
    .map_err(|err| err.to_string())?;
  } else if let Some(window) = app.get_webview_window(label) {
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.navigate(parsed_url);
  }
  Ok(())
}

#[tauri::command]
async fn app_open_purchase_window(app: tauri::AppHandle) -> Result<(), String> {
  open_external_window(app, "purchase", "AI8888 Purchase", PURCHASE_URL, 1180.0, 860.0)
}

#[tauri::command]
async fn app_open_daily_reset_window(app: tauri::AppHandle) -> Result<(), String> {
  open_external_window(app, "daily-reset", "AI8888 Daily Reset", REST_URL, 1180.0, 860.0)
}

#[tauri::command]
async fn app_open_radar_window(app: tauri::AppHandle) -> Result<(), String> {
  open_external_window(app, "radar", "智商雷达", RADAR_URL, 980.0, 900.0)
}

#[tauri::command]
async fn app_open_model_status_window(app: tauri::AppHandle) -> Result<(), String> {
  open_external_window(app, "model-status", "模型监控", MODEL_STATUS_URL, 1180.0, 860.0)
}
#[tauri::command]
async fn app_open_codex_sessions_window(app: tauri::AppHandle) -> Result<(), String> {
  if app.get_webview_window("codex-sessions").is_none() {
    tauri::WebviewWindowBuilder::new(
      &app,
      "codex-sessions",
      tauri::WebviewUrl::App("index.html?view=sessions".into()),
    )
    .title("Codex \u{4f1a}\u{8bdd}\u{7ba1}\u{7406}")
    .inner_size(1180.0, 780.0)
    .min_inner_size(980.0, 640.0)
    .resizable(true)
    .visible(true)
    .build()
    .map_err(|err| err.to_string())?;
  } else if let Some(window) = app.get_webview_window("codex-sessions") {
    let _ = window.show();
    let _ = window.set_focus();
  }
  Ok(())
}

#[tauri::command]
async fn app_list_codex_sessions() -> Result<Vec<CodexSessionMeta>, String> {
  tauri::async_runtime::spawn_blocking(codex_sessions::scan_sessions)
    .await
    .map_err(|err| format!("failed to scan Codex sessions: {err}"))
}

#[tauri::command]
async fn app_get_codex_session_messages(source_path: String) -> Result<Vec<CodexSessionMessage>, String> {
  tauri::async_runtime::spawn_blocking(move || codex_sessions::load_messages(&source_path))
    .await
    .map_err(|err| format!("failed to load Codex session: {err}"))?
    .map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_launch_codex_session(session_id: String, cwd: Option<String>, model_provider_key: Option<String>) -> Result<(), String> {
  tauri::async_runtime::spawn_blocking(move || codex_sessions::launch_resume(&session_id, cwd.as_deref(), model_provider_key.as_deref()))
    .await
    .map_err(|err| format!("failed to launch Codex session: {err}"))?
    .map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_repair_codex_session_visibility(requests: Vec<CodexSessionVisibilityRepairRequest>) -> Result<Vec<CodexSessionVisibilityRepairOutcome>, String> {
  tauri::async_runtime::spawn_blocking(move || codex_sessions::repair_visibility(&requests))
    .await
    .map_err(|err| format!("failed to repair Codex session visibility: {err}"))
}

#[tauri::command]
async fn app_login_with_password(
  state: State<'_, SharedState>,
  email: String,
  password: String,
) -> Result<AppStateData, String> {
  let result = state.api.login(&LoginPayload { email, password }).await.map_err(|err| err.to_string())?;
  apply_login_result(&state, result).await.map_err(|err| err.to_string())
}


fn clear_auth_state(data: &mut AppStateData) {
  let selected_tool = data.selected_tool.clone();
  *data = AppStateData {
    selected_tool,
    ..Default::default()
  };
}

fn requires_relogin(error: &str) -> bool {
  let lower = error.to_ascii_lowercase();
  error.contains("无法获取账号信息")
    || lower.contains("not logged in")
    || lower.contains("missing refresh token")
    || lower.contains("please login again")
    || lower.contains("please re-login")
    || lower.contains("unauthorized")
    || lower.contains("unauthenticated")
    || lower.contains("invalid token")
    || lower.contains("token expired")
    || lower.contains("jwt")
    || lower.contains("401")
    || lower.contains("403")
}

fn relogin_error(error: impl ToString) -> String {
  let text = error.to_string();
  if requires_relogin(&text) {
    "无法获取账号信息，请重新登录".into()
  } else {
    text
  }
}

async fn clear_auth_and_persist(state: &State<'_, SharedState>, message: &str) -> Result<AppStateData, String> {
  let mut guard = state.data.write().await;
  clear_auth_state(&mut guard);
  guard.last_error = Some(message.to_string());
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(guard.clone())
}

#[tauri::command]
async fn app_refresh_session(state: State<'_, SharedState>) -> Result<AppStateData, String> {
  let session = {
    let guard = state.data.read().await;
    guard.session.clone().ok_or_else(|| "not logged in".to_string())?
  };
  if session.refresh_token.is_empty() {
    let message = "无法获取账号信息，请重新登录";
    let _ = clear_auth_and_persist(&state, message).await?;
    return Err(message.into());
  }

  let refreshed = match state
    .api
    .refresh(&RefreshPayload { refresh_token: session.refresh_token })
    .await
  {
    Ok(value) => value,
    Err(err) => {
      let message = relogin_error(err);
      if requires_relogin(&message) {
        let _ = clear_auth_and_persist(&state, &message).await?;
      }
      return Err(message);
    }
  };

  let account = match state.api.get_account(&refreshed.access_token).await {
    Ok(value) => value,
    Err(err) => {
      let message = relogin_error(err);
      if requires_relogin(&message) {
        let _ = clear_auth_and_persist(&state, &message).await?;
      }
      return Err(message);
    }
  };

  apply_login_result(&state, LoginResult { session: refreshed, account })
    .await
    .map_err(|err| err.to_string())
}

async fn apply_login_result(state: &State<'_, SharedState>, result: LoginResult) -> Result<AppStateData, AppError> {
  let mut account = result.account;
  if let Ok(profile) = state.api.get_profile(&result.session.access_token).await {
    account = merge_account(account, profile);
  }
  let subscriptions = state.api.get_subscriptions(&result.session.access_token).await.unwrap_or_default();
  let subscription_progress = state.api.get_subscription_progress(&result.session.access_token).await.unwrap_or_default();
  let api_groups = state.api.get_groups(&result.session.access_token).await.unwrap_or_default();
  let keys = state.api.get_keys(&result.session.access_token).await.unwrap_or_default();
  let groups = merge_groups(api_groups, &subscriptions, &keys);

  let mut guard = state.data.write().await;
  guard.session = Some(StoredSession {
    access_token: result.session.access_token,
    refresh_token: result.session.refresh_token,
    expires_in: result.session.expires_in,
    account: Some(account.clone()),
  });
  guard.account = Some(account);
  guard.subscriptions = subscriptions;
  guard.subscription_progress = subscription_progress;
  guard.groups = groups;
  guard.keys = keys;
  guard.login_window_open = false;
  guard.last_error = None;
  persist_state(&guard)?;
  Ok(guard.clone())
}

fn groups_from_subscriptions(subscriptions: &[SubscriptionSummary]) -> Vec<GroupSummary> {
  let mut groups = Vec::new();
  for subscription in subscriptions {
    if let Some(group) = &subscription.group {
      if !groups.iter().any(|item: &GroupSummary| item.id == group.id) {
        groups.push(group.clone());
      }
    }
  }
  groups
}

fn groups_from_keys(keys: &Pagination<ApiKeySummary>) -> Vec<GroupSummary> {
  let mut groups = Vec::new();
  for key in &keys.items {
    if let Some(group) = &key.group {
      if !groups.iter().any(|item: &GroupSummary| item.id == group.id) {
        groups.push(group.clone());
      }
    }
  }
  groups
}

fn merge_groups(api_groups: Vec<GroupSummary>, subscriptions: &[SubscriptionSummary], keys: &Pagination<ApiKeySummary>) -> Vec<GroupSummary> {
  let mut groups = Vec::new();
  for source in [api_groups, groups_from_subscriptions(subscriptions), groups_from_keys(keys)] {
    for group in source {
      if !groups.iter().any(|item: &GroupSummary| item.id == group.id) {
        groups.push(group);
      }
    }
  }
  groups.sort_by(|left, right| left.id.cmp(&right.id));
  groups
}

fn merge_account(mut base: AccountSummary, profile: AccountSummary) -> AccountSummary {
  if profile.id != 0 {
    base.id = profile.id;
  }
  if !profile.email.is_empty() {
    base.email = profile.email;
  }
  if profile.username.is_some() {
    base.username = profile.username;
  }
  if profile.role.is_some() {
    base.role = profile.role;
  }
  if profile.balance != 0.0 {
    base.balance = profile.balance;
  }
  if profile.concurrency != 0 {
    base.concurrency = profile.concurrency;
  }
  if !profile.status.is_empty() {
    base.status = profile.status;
  }
  if profile.run_mode.is_some() {
    base.run_mode = profile.run_mode;
  }
  base
}

#[tauri::command]
async fn app_load_remote_state(state: State<'_, SharedState>) -> Result<AppStateData, String> {
  let token = {
    let guard = state.data.read().await;
    guard.session.as_ref().map(|session| session.access_token.clone()).ok_or_else(|| "not logged in".to_string())?
  };

  // Account info is required for a valid session. Subscription expiry is informational only
  // and must not force re-login.
  let account = match state.api.get_account(&token).await {
    Ok(value) => value,
    Err(err) => {
      let message = relogin_error(err);
      if requires_relogin(&message) {
        let _ = clear_auth_and_persist(&state, &message).await?;
      }
      return Err(message);
    }
  };

  let profile = state.api.get_profile(&token).await.unwrap_or_else(|_| account.clone());
  let subscriptions = state.api.get_subscriptions(&token).await.unwrap_or_default();
  let subscription_progress = state.api.get_subscription_progress(&token).await.unwrap_or_default();
  let api_groups = state.api.get_groups(&token).await.unwrap_or_default();
  let keys = state.api.get_keys(&token).await.unwrap_or_default();
  let groups = merge_groups(api_groups, &subscriptions, &keys);

  let mut guard = state.data.write().await;
  guard.account = Some(merge_account(account, profile));
  guard.subscriptions = subscriptions;
  guard.subscription_progress = subscription_progress;
  guard.groups = groups;
  guard.keys = keys;
  guard.last_error = None;
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(guard.clone())
}

#[tauri::command]
async fn app_get_remote_account(state: State<'_, SharedState>) -> Result<AccountSummary, String> {
  state.data.read().await.account.clone().ok_or_else(|| "not logged in".to_string())
}

#[tauri::command]
async fn app_get_remote_subscriptions(state: State<'_, SharedState>) -> Result<Vec<SubscriptionSummary>, String> {
  Ok(state.data.read().await.subscriptions.clone())
}

#[tauri::command]
async fn app_get_remote_groups(state: State<'_, SharedState>) -> Result<Vec<GroupSummary>, String> {
  Ok(state.data.read().await.groups.clone())
}

#[tauri::command]
async fn app_get_remote_keys(state: State<'_, SharedState>) -> Result<Pagination<ApiKeySummary>, String> {
  Ok(state.data.read().await.keys.clone())
}

#[tauri::command]
async fn app_create_key(state: State<'_, SharedState>, payload: CreateKeyPayload) -> Result<ApiKeySummary, String> {
  let token = state.data.read().await.session.as_ref().map(|session| session.access_token.clone()).ok_or_else(|| "not logged in".to_string())?;
  let created = state.api.create_key(&token, &payload).await.map_err(|err| err.to_string())?;
  let mut guard = state.data.write().await;
  guard.keys.items.insert(0, created.clone());
  guard.keys.total = guard.keys.items.len() as u64;
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(created)
}

#[tauri::command]
async fn app_update_key(state: State<'_, SharedState>, key_id: u64, payload: UpdateKeyPayload) -> Result<ApiKeySummary, String> {
  let token = state.data.read().await.session.as_ref().map(|session| session.access_token.clone()).ok_or_else(|| "not logged in".to_string())?;
  let updated = state.api.update_key(&token, key_id, &payload).await.map_err(|err| err.to_string())?;
  let mut guard = state.data.write().await;
  if let Some(item) = guard.keys.items.iter_mut().find(|item| item.id == key_id) {
    *item = updated.clone();
  }
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(updated)
}

#[tauri::command]
async fn app_update_key_group(state: State<'_, SharedState>, key_id: u64, group_id: Option<u64>) -> Result<ApiKeySummary, String> {
  app_update_key(state, key_id, UpdateKeyPayload { name: None, group_id, status: None }).await
}

#[tauri::command]
async fn app_delete_key(state: State<'_, SharedState>, key_id: u64) -> Result<(), String> {
  let token = state.data.read().await.session.as_ref().map(|session| session.access_token.clone()).ok_or_else(|| "not logged in".to_string())?;
  state.api.delete_key(&token, key_id).await.map_err(|err| err.to_string())?;
  let mut guard = state.data.write().await;
  guard.keys.items.retain(|item| item.id != key_id);
  guard.keys.total = guard.keys.items.len() as u64;
  if guard.selected_key_id == Some(key_id) {
    guard.selected_key_id = None;
  }
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(())
}

#[tauri::command]
async fn app_set_selected_tool(state: State<'_, SharedState>, tool: String) -> Result<AppStateData, String> {
  let mut guard = state.data.write().await;
  guard.selected_tool = tool;
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(guard.clone())
}

#[tauri::command]
async fn app_set_selected_key(state: State<'_, SharedState>, key_id: Option<u64>) -> Result<AppStateData, String> {
  let mut guard = state.data.write().await;
  guard.selected_key_id = key_id;
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(guard.clone())
}

#[tauri::command]
async fn app_logout(state: State<'_, SharedState>) -> Result<AppStateData, String> {
  let mut guard = state.data.write().await;
  clear_auth_state(&mut guard);
  persist_state(&guard).map_err(|err| err.to_string())?;
  Ok(guard.clone())
}

#[tauri::command]
async fn app_prepare_switch(state: State<'_, SharedState>, tool: String, base_url: Option<String>, api_key: String, model: Option<String>, review_model: Option<String>, local_routing_enabled: Option<bool>, local_route_apps: Option<Vec<String>>, local_route_model_map: Option<HashMap<String, String>>, local_route_preserve_claude_auth: Option<bool>, local_route_only: Option<bool>) -> Result<SwitchTarget, String> {
  let tool = match tool.as_str() {
    "codex" => ToolKind::Codex,
    "claude" => ToolKind::Claude,
    "opencode" => ToolKind::OpenCode,
    "openclaw" => ToolKind::OpenClaw,
    "hermes" => ToolKind::Hermes,
    _ => return Err("unsupported tool".into()),
  };
  let _ = state.api.ensure_best_endpoint().await;
  let base_url = base_url.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| state.api.site_base_url());
  let mut target = default_switch_target(tool, &base_url, &api_key);
  target.local_routing_enabled = local_routing_enabled.unwrap_or(false);
  target.local_route_apps = local_route_apps.unwrap_or_default();
  target.local_route_model_map = local_route_model_map.unwrap_or_default();
  target.local_route_preserve_claude_auth = local_route_preserve_claude_auth.unwrap_or(false);
  target.local_route_only = local_route_only.unwrap_or(false);
  if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
    target.model = Some(model);
  }
  if let Some(review_model) = review_model.filter(|value| !value.trim().is_empty()) {
    target.review_model = Some(review_model);
  } else {
    target.review_model = target.model.clone();
  }
  Ok(target)
}

#[tauri::command]
async fn app_write_switch(state: State<'_, SharedState>, target: SwitchTarget) -> Result<ConfigTransactionResult, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  write_switch_transaction(&target).await
}

async fn write_switch_transaction(target: &SwitchTarget) -> Result<ConfigTransactionResult, String> {
  if target.api_key.trim().is_empty() {
    return Err("API Key cannot be empty".into());
  }

  let managed = managed_paths_for_target(&target);
  let allowed = all_managed_config_paths();
  let snapshot = create_snapshot(&managed, &format!("写入 {} 配置前", target.profile_name)).map_err(|err| err.to_string())?;
  let transaction = match write_local_routed_targets(&target) {
    Ok(()) if target.local_routing_enabled => ensure_local_proxy(&target).await.map_err(|err| err.to_string()),
    Ok(()) => Ok(()),
    Err(error) => Err(error.to_string()),
  };

  if let Err(error) = transaction {
    match rollback_failed_transaction(&snapshot.id, &allowed) {
      Ok(()) => {
        let _ = remove_snapshot(&snapshot.id);
        return Err(format!("配置事务失败，已自动回滚：{error}"));
      }
      Err(rollback_error) => {
        return Err(format!("配置事务失败且自动回滚失败：{error}；回滚错误：{rollback_error}；快照 {} 已保留", snapshot.id));
      }
    }
  }

  let _ = prune_snapshots(&[snapshot.id.as_str()]);
  let artifacts = build_tool_preview(&target);
  Ok(ConfigTransactionResult {
    snapshot,
    artifacts,
    message: "配置事务已提交，可从历史版本回滚".into(),
  })
}

#[tauri::command]
async fn app_list_config_profiles(state: State<'_, SharedState>) -> Result<Vec<ConfigProfile>, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  list_profiles().map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_save_config_profile(
  state: State<'_, SharedState>,
  profile_id: Option<String>,
  expected_updated_at: Option<u64>,
  profile: ConfigProfileInput,
) -> Result<ConfigProfile, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  save_profile(profile_id.as_deref(), expected_updated_at, profile).map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_delete_config_profile(
  state: State<'_, SharedState>,
  profile_id: String,
  expected_updated_at: u64,
) -> Result<(), String> {
  let _transaction_guard = state.config_transaction.lock().await;
  delete_profile(&profile_id, expected_updated_at).map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_apply_config_profile(
  state: State<'_, SharedState>,
  profile_id: String,
  api_key_override: Option<String>,
) -> Result<ConfigTransactionResult, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  let keys = state.data.read().await.keys.items.clone();
  let (profile, target) = resolve_profile_target(&profile_id, &keys, api_key_override).map_err(|err| err.to_string())?;
  let mut result = write_switch_transaction(&target).await?;

  let persist_warning = {
    let mut guard = state.data.write().await;
    guard.selected_tool = profile.tool;
    guard.selected_key_id = profile.key_id;
    persist_state(&guard).err().map(|error| error.to_string())
  };
  result.message = match persist_warning {
    Some(warning) => format!("Profile 已应用，但保存当前选择失败：{warning}"),
    None => format!("Profile「{}」已应用，可从配置历史回滚", profile.name),
  };
  Ok(result)
}

#[tauri::command]
fn app_list_config_snapshots() -> Result<Vec<ConfigSnapshotSummary>, String> {
  list_snapshots().map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_restore_config_snapshot(state: State<'_, SharedState>, snapshot_id: String) -> Result<ConfigTransactionResult, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  let allowed = all_managed_config_paths();
  let (restored, recovery) = restore_snapshot(&snapshot_id, &allowed).map_err(|err| err.to_string())?;
  let artifacts = restored.files.iter().map(|file| {
    (file.path.clone(), format!("已恢复 {}", file.label))
  }).collect();
  Ok(ConfigTransactionResult {
    snapshot: recovery,
    artifacts,
    message: format!("已恢复版本：{}", restored.label),
  })
}

#[tauri::command]
async fn app_copy_target_preview(target: SwitchTarget) -> Result<Vec<(String, String)>, String> {
  Ok(build_tool_preview(&target))
}

#[tauri::command]
async fn app_fetch_models(state: State<'_, SharedState>, query: ModelsQuery) -> Result<Vec<ModelSummary>, String> {
  state.api.fetch_models(&query).await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_probe_best_endpoint(state: State<'_, SharedState>) -> Result<EndpointProbeSummary, String> {
  state.api.select_best_endpoint().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_get_endpoint(state: State<'_, SharedState>) -> Result<EndpointProbeSummary, String> {
  state.api.ensure_best_endpoint().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_get_local_route_manifest() -> Result<LocalRouteManifest, String> {
  read_json(&local_route_manifest_path()).map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_get_local_route_statuses() -> Result<Vec<LocalRouteStatus>, String> {
  Ok(detect_local_route_statuses())
}

#[tauri::command]
async fn app_cleanup_local_route_takeover(state: State<'_, SharedState>) -> Result<ConfigTransactionResult, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  run_config_file_transaction("清理本地路由前", cleanup_local_route_takeover)
}

#[tauri::command]
async fn app_restore_local_route_backups(state: State<'_, SharedState>) -> Result<ConfigTransactionResult, String> {
  let _transaction_guard = state.config_transaction.lock().await;
  run_config_file_transaction("恢复旧版备份前", restore_local_route_backups)
}

fn run_config_file_transaction(
  label: &str,
  operation: impl FnOnce() -> Result<Vec<(String, String)>, AppError>,
) -> Result<ConfigTransactionResult, String> {
  let managed = managed_paths_for_route_cleanup();
  let allowed = all_managed_config_paths();
  let snapshot = create_snapshot(&managed, label).map_err(|err| err.to_string())?;
  let artifacts = match operation() {
    Ok(artifacts) => artifacts,
    Err(error) => {
      return match rollback_failed_transaction(&snapshot.id, &allowed) {
        Ok(()) => {
          let _ = remove_snapshot(&snapshot.id);
          Err(format!("配置事务失败，已自动回滚：{error}"))
        }
        Err(rollback_error) => Err(format!("配置事务失败且自动回滚失败：{error}；回滚错误：{rollback_error}；快照 {} 已保留", snapshot.id)),
      };
    }
  };
  let _ = prune_snapshots(&[snapshot.id.as_str()]);
  Ok(ConfigTransactionResult {
    snapshot,
    artifacts,
    message: "配置事务已提交，可从历史版本回滚".into(),
  })
}

fn show_main_window(app: &tauri::AppHandle) {
  if let Some(window) = app.get_webview_window("main") {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
  }
}

fn setup_system_tray(app: &tauri::App) -> tauri::Result<()> {
  let show_item = MenuItem::with_id(app, TRAY_SHOW_ID, "显示主窗口", true, None::<&str>)?;
  let quit_item = MenuItem::with_id(app, TRAY_QUIT_ID, "退出", true, None::<&str>)?;
  let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

  let mut tray = TrayIconBuilder::with_id(TRAY_ID)
    .menu(&menu)
    .show_menu_on_left_click(false)
    .tooltip("AI8888 Switch");
  if let Some(icon) = app.default_window_icon() {
    tray = tray.icon(icon.clone());
  }

  tray
    .on_menu_event(|app, event| match event.id().as_ref() {
      TRAY_SHOW_ID => show_main_window(app),
      TRAY_QUIT_ID => app.exit(0),
      _ => {}
    })
    .on_tray_icon_event(|tray, event| {
      if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
      } = event
      {
        show_main_window(tray.app_handle());
      }
    })
    .build(app)?;

  Ok(())
}

pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
      show_main_window(app);
    }))
    .setup(|app| {
      let shared = SharedState::new().map_err(|err| Box::new(err) as Box<dyn std::error::Error>)?;
      let startup_api = shared.api.clone();
      tauri::async_runtime::spawn(async move {
        let _ = startup_api.ensure_best_endpoint().await;
      });
      if let Some(stored) = load_state() {
        *shared.data.blocking_write() = stored;
      }
      app.manage(shared);
      setup_system_tray(app)?;
      Ok(())
    })
    .on_window_event(|window, event| {
      if window.label() == "main" {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
          api.prevent_close();
          let _ = window.hide();
        }
      }
    })
    .invoke_handler(tauri::generate_handler![
      app_get_state,
      app_get_tools,
      app_get_codex_auth_status,
      app_start_codex_login,
      app_cancel_codex_login,
      app_logout_codex,
      app_open_codex_device_auth_page,
      app_activate_codex_official,
      app_check_update,
      app_install_update,
      app_cancel_update,
      app_get_preferences,
      app_set_preferences,
      app_complete_onboarding,
      app_dismiss_alert,
      app_search_codex_sessions,
      app_open_login_window,
      app_open_purchase_window,
      app_open_daily_reset_window,
      app_open_radar_window,
      app_open_model_status_window,
      app_open_codex_sessions_window,
      app_list_codex_sessions,
      app_get_codex_session_messages,
      app_launch_codex_session,
      app_repair_codex_session_visibility,
      app_login_with_password,
      app_refresh_session,
      app_load_remote_state,
      app_get_remote_account,
      app_get_remote_subscriptions,
      app_get_remote_groups,
      app_get_remote_keys,
      app_create_key,
      app_update_key,
      app_update_key_group,
      app_delete_key,
      app_set_selected_tool,
      app_set_selected_key,
      app_logout,
      app_prepare_switch,
      app_write_switch,
      app_list_config_profiles,
      app_save_config_profile,
      app_delete_config_profile,
      app_apply_config_profile,
      app_list_config_snapshots,
      app_restore_config_snapshot,
      app_copy_target_preview,
      app_fetch_models,
      app_probe_best_endpoint,
      app_get_endpoint,
      app_get_local_route_manifest,
      app_get_local_route_statuses,
      app_cleanup_local_route_takeover,
      app_restore_local_route_backups,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}





