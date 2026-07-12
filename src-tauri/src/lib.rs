mod api;
mod codex_sessions;
mod config;
mod error;
mod local_proxy;
mod models;
mod tools;

use std::collections::HashMap;

use api::{ApiClient, CreateKeyPayload, LoginPayload, ModelsQuery, RefreshPayload, UpdateKeyPayload};
use codex_sessions::{CodexSessionMessage, CodexSessionMeta, CodexSessionVisibilityRepairOutcome, CodexSessionVisibilityRepairRequest};
use config::{ensure_app_dir, local_route_manifest_path, read_json, state_path, write_json, MODEL_STATUS_URL, PURCHASE_URL, RADAR_URL};
use error::AppError;
use local_proxy::ensure_local_proxy;
use models::{AccountSummary, ApiKeySummary, AppStateData, EndpointProbeSummary, GroupSummary, LocalRouteManifest, LocalRouteStatus, LoginResult, ModelSummary, Pagination, StoredSession, SubscriptionSummary, SwitchTarget, ToolProfile, UpdateCheckResult};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State};
use tokio::sync::RwLock;
use tools::{build_tool_preview, cleanup_local_route_takeover, default_switch_target, detect_local_route_statuses, restore_local_route_backups, supported_tools, write_local_routed_targets, ToolKind};

const CURRENT_APP_VERSION: &str = "v0.0.3";
const GITHUB_UPDATE_REPOSITORY: &str = "AI8888-SHOP/AI8888-tools";
const TRAY_ID: &str = "main-tray";
const TRAY_SHOW_ID: &str = "tray-show";
const TRAY_QUIT_ID: &str = "tray-quit";

pub struct SharedState {
  pub api: ApiClient,
  pub data: RwLock<AppStateData>,
}

impl SharedState {
  pub fn new() -> Result<Self, AppError> {
    Ok(Self {
      api: ApiClient::new()?,
      data: RwLock::new(AppStateData {
        selected_tool: ToolKind::Codex.as_str().to_string(),
        ..Default::default()
      }),
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
async fn app_check_update() -> Result<UpdateCheckResult, String> {
  let url = format!("https://api.github.com/repos/{GITHUB_UPDATE_REPOSITORY}/releases/latest");
  let client = reqwest::Client::builder().user_agent("AI8888-tools-update-check").build().map_err(|err| err.to_string())?;
  let response = match client.get(&url).send().await {
    Ok(response) => response,
    Err(err) => {
      return Ok(UpdateCheckResult {
        current_version: CURRENT_APP_VERSION.to_string(),
        latest_version: None,
        update_available: false,
        release_url: Some(format!("https://github.com/{GITHUB_UPDATE_REPOSITORY}/releases")),
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
  let update_available = latest_version.as_deref().map(|version| compare_versions(version, CURRENT_APP_VERSION).is_gt()).unwrap_or(false);

  Ok(UpdateCheckResult {
    current_version: CURRENT_APP_VERSION.to_string(),
    latest_version,
    update_available,
    release_url,
    repository: GITHUB_UPDATE_REPOSITORY.to_string(),
    error: None,
  })
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
async fn app_prepare_switch(state: State<'_, SharedState>, tool: String, base_url: Option<String>, api_key: String, model: Option<String>, local_routing_enabled: Option<bool>, local_route_apps: Option<Vec<String>>, local_route_model_map: Option<HashMap<String, String>>, local_route_preserve_claude_auth: Option<bool>, local_route_only: Option<bool>) -> Result<SwitchTarget, String> {
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
  Ok(target)
}

#[tauri::command]
async fn app_write_switch(target: SwitchTarget) -> Result<Vec<(String, String)>, String> {
  if target.api_key.trim().is_empty() {
    return Err("API Key cannot be empty".into());
  }
  write_local_routed_targets(&target).map_err(|err| err.to_string())?;
  if target.local_routing_enabled {
    ensure_local_proxy(&target).await.map_err(|err| err.to_string())?;
  }
  Ok(build_tool_preview(&target))
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
async fn app_cleanup_local_route_takeover() -> Result<Vec<(String, String)>, String> {
  cleanup_local_route_takeover().map_err(|err| err.to_string())
}

#[tauri::command]
async fn app_restore_local_route_backups() -> Result<Vec<(String, String)>, String> {
  restore_local_route_backups().map_err(|err| err.to_string())
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
      app_check_update,
      app_open_login_window,
      app_open_purchase_window,
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





