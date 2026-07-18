use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{normalize_base_url, profiles_path, write_json};
use crate::error::AppError;
use crate::models::{ApiKeySummary, ConfigProfile, ConfigProfileInput, SwitchTarget};

const PROFILE_SCHEMA_VERSION: u32 = 1;
const MAX_PROFILES: usize = 50;
static PROFILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredConfigProfile {
  id: String,
  created_at: u64,
  updated_at: u64,
  name: String,
  tool: String,
  base_url: String,
  key_id: Option<u64>,
  #[serde(default)]
  api_key: String,
  model: Option<String>,
  #[serde(default)]
  review_model: Option<String>,
  #[serde(default)]
  local_routing_enabled: bool,
  #[serde(default)]
  local_route_apps: Vec<String>,
  #[serde(default)]
  local_route_model_map: HashMap<String, String>,
  #[serde(default)]
  local_route_preserve_claude_auth: bool,
  #[serde(default)]
  local_route_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileStore {
  #[serde(default = "profile_schema_version")]
  schema_version: u32,
  #[serde(default)]
  profiles: Vec<StoredConfigProfile>,
}

impl Default for ProfileStore {
  fn default() -> Self {
    Self { schema_version: PROFILE_SCHEMA_VERSION, profiles: Vec::new() }
  }
}

fn profile_schema_version() -> u32 {
  PROFILE_SCHEMA_VERSION
}

fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_millis() as u64).unwrap_or(0)
}

fn next_profile_id(now: u64) -> String {
  format!("profile-{now}-{}-{}", std::process::id(), PROFILE_SEQUENCE.fetch_add(1, Ordering::Relaxed))
}

fn read_store() -> Result<ProfileStore, AppError> {
  let path = profiles_path();
  if !path.exists() {
    return Ok(ProfileStore::default());
  }
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  let value: serde_json::Value = serde_json::from_str(&content).map_err(|err| AppError::json(&path, err))?;
  let mut store = if value.is_array() {
    ProfileStore {
      schema_version: PROFILE_SCHEMA_VERSION,
      profiles: serde_json::from_value(value).map_err(|err| AppError::json(&path, err))?,
    }
  } else {
    serde_json::from_value(value).map_err(|err| AppError::json(&path, err))?
  };
  if store.schema_version > PROFILE_SCHEMA_VERSION {
    return Err(AppError::Message(format!("profile store schema {} is newer than supported schema {PROFILE_SCHEMA_VERSION}", store.schema_version)));
  }
  store.schema_version = PROFILE_SCHEMA_VERSION;
  Ok(store)
}

fn write_store(store: &ProfileStore) -> Result<(), AppError> {
  let path = profiles_path();
  write_json(&path, store)?;
  let backup = path.with_file_name("profiles.json.ai8888-switch.bak");
  if backup.exists() {
    fs::remove_file(&backup).map_err(|err| AppError::io(&backup, err))?;
  }
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|err| AppError::io(&path, err))?;
  }
  Ok(())
}

fn normalize_input(mut input: ConfigProfileInput) -> Result<ConfigProfileInput, AppError> {
  input.name = input.name.trim().to_string();
  if input.name.is_empty() || input.name.chars().count() > 80 || input.name.chars().any(char::is_control) {
    return Err(AppError::Message("Profile 名称必须为 1 到 80 个有效字符".into()));
  }
  if !matches!(input.tool.as_str(), "codex" | "claude" | "opencode" | "openclaw" | "hermes") {
    return Err(AppError::Message(format!("Profile 包含不支持的工具：{}", input.tool)));
  }
  input.base_url = normalize_base_url(&input.base_url);
  let parsed_url = reqwest::Url::parse(&input.base_url).map_err(|_| AppError::Message("Profile 的端点地址无效".into()))?;
  if !matches!(parsed_url.scheme(), "https" | "http")
    || parsed_url.host_str().is_none()
    || !parsed_url.username().is_empty()
    || parsed_url.password().is_some()
    || parsed_url.query().is_some()
    || parsed_url.fragment().is_some()
    || input.base_url.chars().any(char::is_control)
    || input.base_url.len() > 2048 {
    return Err(AppError::Message("Profile 的端点地址无效".into()));
  }
  if let Some(api_key) = input.api_key.take() {
    let api_key = api_key.trim().to_string();
    if api_key.len() > 8192 || api_key.chars().any(char::is_control) {
      return Err(AppError::Message("Profile 的 API Key 无效".into()));
    }
    input.api_key = (!api_key.is_empty()).then_some(api_key);
  }
  input.model = input.model.map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
  input.review_model = input.review_model.map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
  if input.model.as_ref().map(|value| value.len() > 200 || value.chars().any(char::is_control)).unwrap_or(false) {
    return Err(AppError::Message("Profile 的模型名称无效".into()));
  }

  if input.review_model.as_ref().map(|value| value.len() > 200 || value.chars().any(char::is_control)).unwrap_or(false) {
    return Err(AppError::Message("Profile 的自动审核模型名称无效".into()));
  }

  let mut seen = HashSet::new();
  let mut apps = Vec::new();
  for app in input.local_route_apps {
    if !matches!(app.as_str(), "codex" | "claude" | "opencode") {
      return Err(AppError::Message(format!("Profile 包含不支持的本地路由工具：{app}")));
    }
    if seen.insert(app.clone()) {
      apps.push(app);
    }
  }
  if input.local_routing_enabled && apps.is_empty() {
    return Err(AppError::Message("Profile 启用了本地路由但未选择路由工具".into()));
  }
  input.local_route_apps = apps;

  let mut model_map = HashMap::new();
  for (role, model) in input.local_route_model_map {
    if !matches!(role.as_str(), "sonnet" | "opus" | "haiku") {
      continue;
    }
    let model = model.trim().to_string();
    if model.len() > 200 || model.chars().any(char::is_control) {
      return Err(AppError::Message(format!("Profile 的 {role} 路由模型无效")));
    }
    if !model.is_empty() {
      model_map.insert(role, model);
    }
  }
  input.local_route_model_map = model_map;
  if !input.local_routing_enabled {
    input.local_route_apps.clear();
    input.local_route_model_map.clear();
    input.local_route_preserve_claude_auth = false;
    input.local_route_only = false;
  }
  Ok(input)
}

fn key_hint(api_key: &str) -> Option<String> {
  if api_key.is_empty() {
    return None;
  }
  let suffix = api_key.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<String>();
  Some(format!("****{suffix}"))
}

fn summary(profile: &StoredConfigProfile) -> ConfigProfile {
  ConfigProfile {
    id: profile.id.clone(),
    created_at: profile.created_at,
    updated_at: profile.updated_at,
    name: profile.name.clone(),
    tool: profile.tool.clone(),
    base_url: profile.base_url.clone(),
    key_id: profile.key_id,
    key_hint: key_hint(&profile.api_key),
    has_stored_key: !profile.api_key.is_empty(),
    model: profile.model.clone(),
    review_model: profile.review_model.clone(),
    local_routing_enabled: profile.local_routing_enabled,
    local_route_apps: profile.local_route_apps.clone(),
    local_route_model_map: profile.local_route_model_map.clone(),
    local_route_preserve_claude_auth: profile.local_route_preserve_claude_auth,
    local_route_only: profile.local_route_only,
  }
}

fn apply_input(profile: &mut StoredConfigProfile, input: ConfigProfileInput, updated_at: u64, is_new: bool) -> Result<(), AppError> {
  let next_api_key = if input.key_id.is_some() {
    String::new()
  } else if let Some(api_key) = input.api_key {
    api_key
  } else if !is_new && profile.key_id.is_none() && !profile.api_key.is_empty() {
    profile.api_key.clone()
  } else {
    return Err(AppError::Message("手动 Key Profile 必须提供 API Key".into()));
  };

  profile.updated_at = updated_at;
  profile.name = input.name;
  profile.tool = input.tool;
  profile.base_url = input.base_url;
  profile.key_id = input.key_id;
  profile.api_key = next_api_key;
  profile.model = input.model;
  profile.review_model = input.review_model;
  profile.local_routing_enabled = input.local_routing_enabled;
  profile.local_route_apps = input.local_route_apps;
  profile.local_route_model_map = input.local_route_model_map;
  profile.local_route_preserve_claude_auth = input.local_route_preserve_claude_auth;
  profile.local_route_only = input.local_route_only;
  Ok(())
}

pub fn list_profiles() -> Result<Vec<ConfigProfile>, AppError> {
  let mut profiles = read_store()?.profiles;
  profiles.sort_by(|left, right| right.updated_at.cmp(&left.updated_at).then_with(|| left.name.cmp(&right.name)));
  Ok(profiles.iter().map(summary).collect())
}

fn get_stored_profile(profile_id: &str) -> Result<StoredConfigProfile, AppError> {
  read_store()?
    .profiles
    .into_iter()
    .find(|profile| profile.id == profile_id)
    .ok_or_else(|| AppError::Message("Profile 不存在或已被删除".into()))
}

pub fn save_profile(profile_id: Option<&str>, expected_updated_at: Option<u64>, input: ConfigProfileInput) -> Result<ConfigProfile, AppError> {
  let input = normalize_input(input)?;
  let mut store = read_store()?;
  let duplicate_name = store.profiles.iter().any(|profile| {
    Some(profile.id.as_str()) != profile_id && profile.name.eq_ignore_ascii_case(&input.name)
  });
  if duplicate_name {
    return Err(AppError::Message(format!("已存在同名 Profile：{}", input.name)));
  }

  let now = now_ms();
  let saved = if let Some(profile_id) = profile_id {
    let profile = store.profiles.iter_mut().find(|profile| profile.id == profile_id)
      .ok_or_else(|| AppError::Message("Profile 不存在或已被删除".into()))?;
    if expected_updated_at != Some(profile.updated_at) {
      return Err(AppError::Message("Profile 已被修改，请刷新后重试".into()));
    }
    apply_input(profile, input, now, false)?;
    summary(profile)
  } else {
    if store.profiles.len() >= MAX_PROFILES {
      return Err(AppError::Message(format!("Profile 数量已达到上限 {MAX_PROFILES}")));
    }
    let mut profile = StoredConfigProfile {
      id: next_profile_id(now),
      created_at: now,
      updated_at: now,
      ..Default::default()
    };
    apply_input(&mut profile, input, now, true)?;
    let saved = summary(&profile);
    store.profiles.push(profile);
    saved
  };
  write_store(&store)?;
  Ok(saved)
}

pub fn delete_profile(profile_id: &str, expected_updated_at: u64) -> Result<(), AppError> {
  let mut store = read_store()?;
  let profile = store.profiles.iter().find(|profile| profile.id == profile_id)
    .ok_or_else(|| AppError::Message("Profile 不存在或已被删除".into()))?;
  if profile.updated_at != expected_updated_at {
    return Err(AppError::Message("Profile 已被修改，请刷新后重试".into()));
  }
  store.profiles.retain(|profile| profile.id != profile_id);
  write_store(&store)
}

pub fn resolve_profile_target(profile_id: &str, keys: &[ApiKeySummary], api_key_override: Option<String>) -> Result<(ConfigProfile, SwitchTarget), AppError> {
  let profile = get_stored_profile(profile_id)?;
  let override_key = api_key_override.map(|value| value.trim().to_string()).filter(|value| !value.is_empty());
  let referenced_key = profile.key_id.and_then(|key_id| {
    keys.iter().find(|key| key.id == key_id).and_then(|key| key.key.clone()).filter(|value| !value.trim().is_empty())
  });
  let api_key = override_key
    .or(referenced_key)
    .or_else(|| (!profile.api_key.is_empty()).then_some(profile.api_key.clone()))
    .ok_or_else(|| AppError::Message("Profile 引用的 Key 当前不可用，请重新选择 Key 后覆盖保存".into()))?;
  let public = summary(&profile);
  let target = SwitchTarget {
    tool: profile.tool,
    profile_name: profile.name,
    base_url: profile.base_url,
    api_key,
    model: profile.model.clone(),
    review_model: profile.review_model.or(profile.model),
    token_type: None,
    local_routing_enabled: profile.local_routing_enabled,
    local_route_apps: profile.local_route_apps,
    local_route_model_map: profile.local_route_model_map,
    local_route_preserve_claude_auth: profile.local_route_preserve_claude_auth,
    local_route_only: profile.local_route_only,
  };
  Ok((public, target))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn input(name: &str) -> ConfigProfileInput {
    ConfigProfileInput {
      name: name.into(),
      tool: "codex".into(),
      base_url: "https://sub.ai8888.shop".into(),
      key_id: Some(7),
      api_key: None,
      model: Some("gpt-test".into()),
      review_model: Some("gpt-review".into()),
      local_routing_enabled: false,
      local_route_apps: Vec::new(),
      local_route_model_map: HashMap::new(),
      local_route_preserve_claude_auth: false,
      local_route_only: false,
    }
  }

  #[test]
  fn creates_updates_and_deletes_profiles_without_exposing_keys() {
    let _guard = crate::config::test_home_guard();
    let root = std::env::temp_dir().join(format!("ai8888-profile-test-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&root).expect("create test home");
    let old_home = std::env::var_os("AI8888_SWITCH_TEST_HOME");
    std::env::set_var("AI8888_SWITCH_TEST_HOME", &root);

    let created = save_profile(None, None, input("Work")).expect("create profile");
    assert!(!created.has_stored_key);
    let mut updated = input("Work renamed");
    updated.model = Some("gpt-updated".into());
    let saved = save_profile(Some(&created.id), Some(created.updated_at), updated).expect("update profile");
    assert_eq!(saved.name, "Work renamed");
    assert_eq!(saved.model.as_deref(), Some("gpt-updated"));
    assert_eq!(saved.review_model.as_deref(), Some("gpt-review"));
    delete_profile(&created.id, saved.updated_at).expect("delete profile");
    assert!(list_profiles().expect("list profiles").is_empty());

    match old_home {
      Some(value) => std::env::set_var("AI8888_SWITCH_TEST_HOME", value),
      None => std::env::remove_var("AI8888_SWITCH_TEST_HOME"),
    }
    let _ = fs::remove_dir_all(root);
  }

  #[test]
  fn preserves_manual_key_without_returning_it_in_profile_summaries() {
    let _guard = crate::config::test_home_guard();
    let root = std::env::temp_dir().join(format!("ai8888-manual-profile-test-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&root).expect("create test home");
    let old_home = std::env::var_os("AI8888_SWITCH_TEST_HOME");
    std::env::set_var("AI8888_SWITCH_TEST_HOME", &root);

    let mut manual = input("Manual");
    manual.key_id = None;
    manual.api_key = Some("sk-manual-secret".into());
    let created = save_profile(None, None, manual).expect("create manual profile");
    assert!(created.has_stored_key);
    assert_eq!(created.key_hint.as_deref(), Some("****cret"));
    assert!(!serde_json::to_string(&created).expect("serialize summary").contains("sk-manual-secret"));

    let mut update = input("Manual renamed");
    update.key_id = None;
    update.api_key = None;
    let saved = save_profile(Some(&created.id), Some(created.updated_at), update).expect("preserve manual key");
    let (_, target) = resolve_profile_target(&saved.id, &[], None).expect("resolve saved manual key");
    assert_eq!(target.api_key, "sk-manual-secret");

    match old_home {
      Some(value) => std::env::set_var("AI8888_SWITCH_TEST_HOME", value),
      None => std::env::remove_var("AI8888_SWITCH_TEST_HOME"),
    }
    let _ = fs::remove_dir_all(root);
  }
}
