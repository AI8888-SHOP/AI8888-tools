use serde_json::json;
use toml::value::Table as TomlTable;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{local_route_manifest_path, normalize_api_base_url, normalize_base_url, path_for, read_json, write_json, write_text, LOCAL_PROXY_BASE_URL, LOCAL_PROXY_OPENAI_BASE_URL, LOCAL_PROXY_PROFILE_NAME, OPENAI_BASE_URL};
use crate::error::AppError;
use crate::models::{LocalRouteEntry, LocalRouteManifest, LocalRouteStatus, SwitchTarget, ToolProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolKind {
  Codex,
  Claude,
  OpenCode,
  OpenClaw,
  Hermes,
}

impl ToolKind {
  pub fn as_str(self) -> &'static str {
    match self {
      ToolKind::Codex => "codex",
      ToolKind::Claude => "claude",
      ToolKind::OpenCode => "opencode",
      ToolKind::OpenClaw => "openclaw",
      ToolKind::Hermes => "hermes",
    }
  }

  pub fn display_name(self) -> &'static str {
    match self {
      ToolKind::Codex => "Codex",
      ToolKind::Claude => "Claude Code",
      ToolKind::OpenCode => "OpenCode",
      ToolKind::OpenClaw => "OpenClaw",
      ToolKind::Hermes => "Hermes Agent",
    }
  }
}

pub fn supported_tools() -> Vec<ToolProfile> {
  [
    (ToolKind::Codex, "写入 Codex config.toml（保留 auth.json）"),
    (ToolKind::Claude, "写入 Claude Code settings.json"),
    (ToolKind::OpenCode, "写入 OpenCode opencode.json"),
    (ToolKind::OpenClaw, "写入 OpenClaw openclaw.json"),
    (ToolKind::Hermes, "写入 Hermes config.yaml"),
  ]
  .into_iter()
  .map(|(tool, description)| ToolProfile {
    tool: tool.as_str().into(),
    display_name: tool.display_name().into(),
    description: description.into(),
    directory: path_for(tool.as_str(), "").display().to_string(),
    config_path: primary_config_path(tool),
    notes: Some("写入前会创建可回滚的版本化快照".into()),
  })
  .collect()
}

fn primary_config_path(tool: ToolKind) -> String {
  match tool {
    ToolKind::Codex => path_for("codex", "config.toml"),
    ToolKind::Claude => path_for("claude", "settings.json"),
    ToolKind::OpenCode => path_for("opencode", "opencode.json"),
    ToolKind::OpenClaw => path_for("openclaw", "openclaw.json"),
    ToolKind::Hermes => path_for("hermes", "config.yaml"),
  }
  .display()
  .to_string()
}

pub fn default_switch_target(tool: ToolKind, base_url: &str, api_key: &str) -> SwitchTarget {
  SwitchTarget {
    tool: tool.as_str().to_string(),
    profile_name: "AI8888".into(),
    base_url: normalize_base_url(if base_url.trim().is_empty() { OPENAI_BASE_URL } else { base_url }),
    api_key: api_key.to_string(),
    model: Some(match tool {
      _ => "gpt-5.5",
    }
    .into()),
    review_model: Some("gpt-5.5".into()),
    token_type: Some(match tool {
      ToolKind::Claude => "ANTHROPIC_AUTH_TOKEN",
      _ => "OPENAI_API_KEY",
    }
    .into()),
    local_routing_enabled: false,
    local_route_apps: Vec::new(),
    local_route_model_map: HashMap::new(),
    local_route_preserve_claude_auth: false,
    local_route_only: false,
  }
}

pub fn write_switch_target(target: &SwitchTarget) -> Result<(), AppError> {
  match parse_tool(&target.tool)? {
    ToolKind::Codex => write_codex(target),
    ToolKind::Claude => write_claude(target),
    ToolKind::OpenCode => write_opencode(target),
    ToolKind::OpenClaw => write_openclaw(target),
    ToolKind::Hermes => write_hermes(target),
  }
}

fn parse_tool(tool: &str) -> Result<ToolKind, AppError> {
  match tool {
    "codex" => Ok(ToolKind::Codex),
    "claude" => Ok(ToolKind::Claude),
    "opencode" => Ok(ToolKind::OpenCode),
    "openclaw" => Ok(ToolKind::OpenClaw),
    "hermes" => Ok(ToolKind::Hermes),
    other => Err(AppError::Message(format!("unsupported tool: {other}"))),
  }
}


fn read_json_value(path: &Path) -> serde_json::Value {
  fs::read_to_string(path).ok().and_then(|content| serde_json::from_str(&content).ok()).unwrap_or_else(|| json!({}))
}

fn toml_table() -> toml::map::Map<String, toml::Value> {
  toml::map::Map::new()
}

fn ensure_toml_table<'a>(value: &'a mut toml::Value, key: &str) -> &'a mut toml::map::Map<String, toml::Value> {
  let needs_insert = !value.get(key).map(|item| item.is_table()).unwrap_or(false);
  if needs_insert {
    value.as_table_mut().expect("root toml table").insert(key.to_string(), toml::Value::Table(toml_table()));
  }
  value.get_mut(key).and_then(|item| item.as_table_mut()).expect("nested toml table")
}

fn build_codex_config_merged(existing: &str, model: &str, review_model: &str, base_url: &str, api_key: &str, route_only: bool) -> String {
  let mut root = existing
    .parse::<toml::Value>()
    .ok()
    .filter(|value| value.is_table())
    .unwrap_or_else(|| toml::Value::Table(toml_table()));
  let root_table = root.as_table_mut().expect("root toml table");
  root_table.insert("model_provider".into(), toml::Value::String("ai8888".into()));
  if !route_only {
    root_table.insert("model".into(), toml::Value::String(model.into()));
    root_table.insert("review_model".into(), toml::Value::String(review_model.into()));
    root_table.insert("model_reasoning_effort".into(), toml::Value::String("high".into()));
    root_table.insert("disable_response_storage".into(), toml::Value::Boolean(true));
    root_table.insert("network_access".into(), toml::Value::String("enabled".into()));
    root_table.insert("windows_wsl_setup_acknowledged".into(), toml::Value::Boolean(true));
  }

  let providers = ensure_toml_table(&mut root, "model_providers");
  let mut ai8888 = toml_table();
  ai8888.insert("name".into(), toml::Value::String("AI8888".into()));
  ai8888.insert("base_url".into(), toml::Value::String(base_url.into()));
  ai8888.insert("wire_api".into(), toml::Value::String("responses".into()));
  ai8888.insert("experimental_bearer_token".into(), toml::Value::String(api_key.into()));
  providers.insert("ai8888".into(), toml::Value::Table(ai8888));

  if !route_only {
    let features = ensure_toml_table(&mut root, "features");
    features.insert("goals".into(), toml::Value::Boolean(true));
    features.insert("responses_websockets_v2".into(), toml::Value::Boolean(true));
  }

  let mut output = toml::to_string_pretty(&root).unwrap_or_default();
  if !output.ends_with('\n') {
    output.push('\n');
  }
  output
}



fn write_codex(target: &SwitchTarget) -> Result<(), AppError> {
  let config_path = path_for("codex", "config.toml");
  let model = target.model.clone().unwrap_or_else(|| "gpt-5.5".into());
  let review_model = target.review_model.clone().unwrap_or_else(|| model.clone());
  let existing = fs::read_to_string(&config_path).unwrap_or_default();
  let config = build_codex_config_merged(&existing, &model, &review_model, &effective_base_url(target), effective_api_key(target), target.local_route_only);
  write_text(&config_path, &config)
}

pub fn activate_codex_official() -> Result<Vec<(String, String)>, AppError> {
  let config_path = path_for("codex", "config.toml");
  let existing = fs::read_to_string(&config_path).unwrap_or_default();
  let mut root = if existing.trim().is_empty() {
    toml::Value::Table(toml_table())
  } else {
    existing.parse::<toml::Value>().map_err(|error| AppError::Toml {
      path: config_path.display().to_string(),
      source: error,
    })?
  };
  if !root.is_table() {
    return Err(AppError::Message("Codex config.toml 根节点必须是 TOML 表".into()));
  }
  let root_table = root.as_table_mut().expect("Codex root should be a table");
  root_table.insert("model_provider".into(), toml::Value::String("openai".into()));
  for key in ["model", "review_model", "openai_base_url", "chatgpt_base_url"] {
    root_table.remove(key);
  }

  let mut output = toml::to_string_pretty(&root)?;
  if !output.ends_with('\n') {
    output.push('\n');
  }
  write_text(&config_path, &output)?;

  let mut artifacts = vec![(config_path.display().to_string(), "Codex 已切换到 OpenAI 官方账户".into())];
  let manifest_path = local_route_manifest_path();
  if manifest_path.exists() {
    let mut manifest: LocalRouteManifest = read_json(&manifest_path)?;
    let previous_len = manifest.entries.len();
    manifest.entries.retain(|entry| entry.app != "codex");
    if manifest.entries.len() != previous_len {
      manifest.updated_at = SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0);
      write_json(&manifest_path, &manifest)?;
      artifacts.push((manifest_path.display().to_string(), "Codex 本地路由已停用".into()));
    }
  }
  Ok(artifacts)
}

fn write_claude(target: &SwitchTarget) -> Result<(), AppError> {
  let path = path_for("claude", "settings.json");
  let mut value = read_json_value(&path);
  if !value.is_object() {
    value = json!({});
  }
  let root = value.as_object_mut().expect("Claude settings should be an object");
  let env = root.entry("env".to_string()).or_insert_with(|| json!({}));
  if !env.is_object() {
    *env = json!({});
  }
  let env = env.as_object_mut().expect("Claude env should be an object");
  let mut next_env = serde_json::Map::new();
  next_env.insert("ANTHROPIC_BASE_URL".into(), json!(effective_claude_base_url(target)));
  if target.local_routing_enabled && target.local_route_preserve_claude_auth {
    if let Some((key, value)) = existing_claude_auth_value() {
      next_env.insert(key, json!(value));
    }
  } else {
    next_env.insert("ANTHROPIC_API_KEY".into(), json!(effective_api_key(target)));
    next_env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(effective_api_key(target)));
  }
  next_env.insert("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(), json!("1"));
  next_env.insert("CLAUDE_CODE_ATTRIBUTION_HEADER".into(), json!("0"));
  if target.local_routing_enabled && !target.local_route_only {
    let model = target.model.clone().unwrap_or_else(|| "gpt-5.5".into());
    next_env.insert("ANTHROPIC_MODEL".into(), json!(model));
    next_env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL".into(), json!(route_model(target, "haiku")));
    next_env.insert("ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME".into(), json!(route_model(target, "haiku")));
    next_env.insert("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), json!(route_model(target, "sonnet")));
    next_env.insert("ANTHROPIC_DEFAULT_SONNET_MODEL_NAME".into(), json!(route_model(target, "sonnet")));
    next_env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL".into(), json!(route_model(target, "opus")));
    next_env.insert("ANTHROPIC_DEFAULT_OPUS_MODEL_NAME".into(), json!(route_model(target, "opus")));
  }
  env.clear();
  env.extend(next_env);
  write_json(&path, &value)
}

fn write_opencode(target: &SwitchTarget) -> Result<(), AppError> {
  let model = target.model.clone().unwrap_or_else(|| "gpt-5.5".into());
  let path = path_for("opencode", "opencode.json");
  let mut value = read_json_value(&path);
  if !value.is_object() {
    value = json!({});
  }
  let root = value.as_object_mut().expect("OpenCode config should be an object");
  root.entry("$schema".to_string()).or_insert_with(|| json!("https://opencode.ai/config.json"));
  root.remove("model");
  let provider = root.entry("provider".to_string()).or_insert_with(|| json!({}));
  if !provider.is_object() {
    *provider = json!({});
  }
  let provider = provider.as_object_mut().expect("OpenCode provider should be an object");
  let mut model_map = serde_json::Map::new();
  if !target.local_route_only {
    model_map.insert(model.clone(), json!({ "name": model }));
  }
  provider.insert("ai8888".to_string(), json!({
    "npm": "@ai-sdk/openai-compatible",
    "name": "AI8888",
    "options": {
      "baseURL": effective_base_url(target),
      "apiKey": effective_api_key(target)
    },
    "models": model_map
  }));
  write_json(&path, &value)
}

fn write_openclaw(target: &SwitchTarget) -> Result<(), AppError> {
  let model = target.model.clone().unwrap_or_else(|| "gpt-5.5".into());
  let path = path_for("openclaw", "openclaw.json");
  let mut value = read_json_value(&path);
  if !value.is_object() {
    value = json!({});
  }
  let root = value.as_object_mut().expect("OpenClaw config should be an object");
  let models = root.entry("models".to_string()).or_insert_with(|| json!({ "mode": "merge", "providers": {} }));
  if !models.is_object() {
    *models = json!({ "mode": "merge", "providers": {} });
  }
  let models_obj = models.as_object_mut().expect("OpenClaw models should be an object");
  models_obj.entry("mode".to_string()).or_insert_with(|| json!("merge"));
  let providers = models_obj.entry("providers".to_string()).or_insert_with(|| json!({}));
  if !providers.is_object() {
    *providers = json!({});
  }
  providers.as_object_mut().expect("OpenClaw providers should be an object").insert("ai8888".to_string(), json!({
    "baseUrl": effective_claude_base_url(target),
    "apiKey": effective_api_key(target),
    "api": "openai-completions",
    "models": [{ "id": model, "name": model }]
  }));
  let agents = root.entry("agents".to_string()).or_insert_with(|| json!({}));
  if agents.is_object() {
    let defaults = agents.as_object_mut().expect("OpenClaw agents should be an object").entry("defaults".to_string()).or_insert_with(|| json!({}));
    if defaults.is_object() {
      defaults.as_object_mut().expect("OpenClaw defaults should be an object").insert("model".to_string(), json!({ "primary": format!("ai8888/{model}") }));
    }
  }
  write_json(&path, &value)
}

fn write_hermes(target: &SwitchTarget) -> Result<(), AppError> {
  let model = target.model.clone().unwrap_or_else(|| "gpt-5.5".into());
  let path = path_for("hermes", "config.yaml");
  let existing = fs::read_to_string(&path).unwrap_or_default();
  let mut root: serde_yaml::Value = serde_yaml::from_str(&existing).unwrap_or_else(|_| serde_yaml::Value::Mapping(Default::default()));
  if !root.is_mapping() {
    root = serde_yaml::Value::Mapping(Default::default());
  }
  let root_map = root.as_mapping_mut().expect("Hermes root should be a mapping");
  let custom_key = serde_yaml::Value::String("custom_providers".to_string());
  let mut providers = root_map
    .get(&custom_key)
    .and_then(|value| value.as_sequence())
    .cloned()
    .unwrap_or_default();
  let mut hermes_model_map = serde_json::Map::new();
  hermes_model_map.insert(model.clone(), json!({ "name": model.clone() }));
  let provider_value = serde_yaml::to_value(json!({
    "name": "ai8888",
    "base_url": effective_claude_base_url(target),
    "api_key": effective_api_key(target),
    "api_mode": "chat_completions",
    "model": model.clone(),
    "models": hermes_model_map
  })).map_err(|err| AppError::Message(err.to_string()))?;
  if let Some(existing_provider) = providers.iter_mut().find(|item| item.get("name").and_then(|name| name.as_str()) == Some("ai8888")) {
    *existing_provider = provider_value;
  } else {
    providers.push(provider_value);
  }
  root_map.insert(custom_key, serde_yaml::Value::Sequence(providers));

  let model_key = serde_yaml::Value::String("model".to_string());
  let mut model_section = root_map
    .get(&model_key)
    .cloned()
    .unwrap_or_else(|| serde_yaml::Value::Mapping(Default::default()));
  if !model_section.is_mapping() {
    model_section = serde_yaml::Value::Mapping(Default::default());
  }
  let model_map = model_section.as_mapping_mut().expect("Hermes model should be a mapping");
  model_map.insert(serde_yaml::Value::String("provider".to_string()), serde_yaml::Value::String("ai8888".to_string()));
  if !target.local_route_only {
    model_map.insert(serde_yaml::Value::String("default".to_string()), serde_yaml::Value::String(model));
  }
  root_map.insert(model_key, model_section);

  let yaml = serde_yaml::to_string(&root).map_err(|err| AppError::Message(err.to_string()))?;
  write_text(&path, &yaml)
}

pub fn build_tool_preview(target: &SwitchTarget) -> Vec<(String, String)> {
  let mut items = Vec::new();
  let tool = parse_tool(&target.tool).unwrap_or(ToolKind::Codex);
  items.extend(preview_entries_for_tool(tool, target.local_routing_enabled));
  if target.local_routing_enabled {
    for app in normalized_local_route_apps(target) {
      if app != target.tool {
        items.extend(preview_entries_for_app(&app));
      }
    }
    items.push((local_route_manifest_path().display().to_string(), "本地路由清单".into()));
  }
  dedupe_preview_items(items)
}

pub fn managed_paths_for_target(target: &SwitchTarget) -> Vec<(PathBuf, String)> {
  build_tool_preview(target).into_iter().map(|(path, label)| (PathBuf::from(path), label)).collect()
}

pub fn all_managed_config_paths() -> Vec<PathBuf> {
  [
    path_for("codex", "config.toml"),
    path_for("claude", "settings.json"),
    path_for("opencode", "opencode.json"),
    path_for("openclaw", "openclaw.json"),
    path_for("hermes", "config.yaml"),
    path_for("gemini", ".env"),
    path_for("gemini", "settings.json"),
    local_route_manifest_path(),
  ].into_iter().collect()
}

pub fn managed_paths_for_route_cleanup() -> Vec<(PathBuf, String)> {
  [
    (path_for("codex", "config.toml"), "Codex config.toml".into()),
    (path_for("claude", "settings.json"), "Claude settings.json".into()),
    (path_for("opencode", "opencode.json"), "OpenCode opencode.json".into()),
    (path_for("gemini", ".env"), "Gemini .env".into()),
    (path_for("gemini", "settings.json"), "Gemini settings.json".into()),
    (local_route_manifest_path(), "本地路由清单".into()),
  ].into_iter().collect()
}

fn preview_entries_for_tool(tool: ToolKind, _local_routing_enabled: bool) -> Vec<(String, String)> {
  match tool {
    ToolKind::Codex => vec![
      (path_for("codex", "config.toml").display().to_string(), "Codex config.toml".into()),
    ],
    ToolKind::Claude => vec![(path_for("claude", "settings.json").display().to_string(), "Claude settings.json".into())],
    ToolKind::OpenCode => vec![(path_for("opencode", "opencode.json").display().to_string(), "OpenCode opencode.json".into())],
    ToolKind::OpenClaw => vec![(path_for("openclaw", "openclaw.json").display().to_string(), "OpenClaw openclaw.json".into())],
    ToolKind::Hermes => vec![(path_for("hermes", "config.yaml").display().to_string(), "Hermes config.yaml".into())],
  }
}

fn preview_entries_for_app(app: &str) -> Vec<(String, String)> {
  match app {
    "codex" => vec![
      (path_for("codex", "config.toml").display().to_string(), "Codex config.toml".into()),
    ],
    "claude" => vec![(path_for("claude", "settings.json").display().to_string(), "Claude settings.json".into())],
    "opencode" => vec![(path_for("opencode", "opencode.json").display().to_string(), "OpenCode opencode.json".into())],
    _ => Vec::new(),
  }
}

fn dedupe_preview_items(items: Vec<(String, String)>) -> Vec<(String, String)> {
  let mut deduped = Vec::new();
  for (path, label) in items {
    if !deduped.iter().any(|(existing, _)| existing == &path) {
      deduped.push((path, label));
    }
  }
  deduped
}




fn effective_api_key(target: &SwitchTarget) -> &str {
  if target.local_routing_enabled {
    "PROXY_MANAGED"
  } else {
    &target.api_key
  }
}

fn effective_base_url(target: &SwitchTarget) -> String {
  if target.local_routing_enabled {
    LOCAL_PROXY_OPENAI_BASE_URL.to_string()
  } else {
    normalize_api_base_url(&target.base_url)
  }
}

fn effective_claude_base_url(target: &SwitchTarget) -> String {
  if target.local_routing_enabled {
    LOCAL_PROXY_BASE_URL.to_string()
  } else {
    target.base_url.clone()
  }
}

pub fn write_local_route_manifest(target: &SwitchTarget) -> Result<(), AppError> {
  write_json(&local_route_manifest_path(), &build_local_route_manifest(target))
}


pub fn build_local_route_manifest(target: &SwitchTarget) -> LocalRouteManifest {
  let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0);
  let mut entries = Vec::new();
  let selected_apps = normalized_local_route_apps(target);
  if selected_apps.iter().any(|app| app == "codex") {
    entries.push(LocalRouteEntry {
      app: "codex".into(),
      enabled: true,
      upstream_name: "AI8888".into(),
      local_base_url: LOCAL_PROXY_OPENAI_BASE_URL.into(),
      local_api_key: "PROXY_MANAGED".into(),
      model: target.model.clone(),
      model_map: HashMap::new(),
      source: "AI8888 Switch".into(),
    });
  }
  if selected_apps.iter().any(|app| app == "claude") {
    entries.push(LocalRouteEntry {
      app: "claude".into(),
      enabled: true,
      upstream_name: "AI8888".into(),
      local_base_url: LOCAL_PROXY_BASE_URL.into(),
      local_api_key: "PROXY_MANAGED".into(),
      model: target.model.clone(),
      model_map: target.local_route_model_map.clone(),
      source: "AI8888 Switch".into(),
    });
  }
  if selected_apps.iter().any(|app| app == "opencode") {
    entries.push(LocalRouteEntry {
      app: "opencode".into(),
      enabled: true,
      upstream_name: "AI8888".into(),
      local_base_url: LOCAL_PROXY_OPENAI_BASE_URL.into(),
      local_api_key: "PROXY_MANAGED".into(),
      model: target.model.clone(),
      model_map: HashMap::new(),
      source: "AI8888 Switch".into(),
    });
  }
  LocalRouteManifest {
    profile_name: LOCAL_PROXY_PROFILE_NAME.to_string(),
    updated_at: now,
    entries,
  }
}


pub fn write_local_routed_targets(target: &SwitchTarget) -> Result<(), AppError> {
  if !target.local_routing_enabled {
    write_switch_target(target)?;
    verify_primary_target_written(target)?;
    return Ok(());
  }

  let unsupported = target
    .local_route_apps
    .iter()
    .filter(|app| !supports_local_route(app))
    .cloned()
    .collect::<Vec<_>>();
  if !unsupported.is_empty() {
    return Err(AppError::Message(format!("unsupported local-routing apps: {}", unsupported.join(", "))));
  }

  let selected_apps = normalized_local_route_apps(target);
  if selected_apps.is_empty() {
    return Err(AppError::Message("local routing enabled but no supported apps were selected".into()));
  }

  for app in &selected_apps {
    write_routed_app(app, target)?;
  }
  for app in &selected_apps {
    verify_routed_app_written(app, target)?;
  }

  write_local_route_manifest(target)?;
  Ok(())
}

fn write_routed_app(app: &str, target: &SwitchTarget) -> Result<(), AppError> {
  match app {
    "codex" => write_codex(target),
    "claude" => write_claude(target),
    "opencode" => write_opencode(target),
    _ => Err(AppError::Message(format!("unsupported local-routing app: {app}"))),
  }
}

fn verify_routed_app_written(app: &str, target: &SwitchTarget) -> Result<(), AppError> {
  match app {
    "codex" => verify_codex_written(target),
    "claude" => verify_claude_written(target),
    "opencode" => verify_opencode_written(target),
    _ => Err(AppError::Message(format!("unsupported local-routing app: {app}"))),
  }
}


fn supports_local_route(app: &str) -> bool {
  matches!(app, "codex" | "claude" | "opencode")
}

fn normalized_local_route_apps(target: &SwitchTarget) -> Vec<String> {
  let preferred = if target.local_route_apps.is_empty() {
    vec!["codex".to_string(), "claude".to_string(), "opencode".to_string()]
  } else {
    target.local_route_apps.clone()
  };
  let mut apps = Vec::new();
  for app in preferred {
    if supports_local_route(&app) && !apps.iter().any(|item| item == &app) {
      apps.push(app);
    }
  }
  apps
}

fn verify_primary_target_written(target: &SwitchTarget) -> Result<(), AppError> {
  match parse_tool(&target.tool)? {
    ToolKind::Codex => verify_codex_written(target),
    ToolKind::Claude => verify_claude_written(target),
    ToolKind::OpenCode => verify_opencode_written(target),
    ToolKind::OpenClaw => verify_openclaw_written(target),
    ToolKind::Hermes => verify_hermes_written(target),
  }
}

fn verify_codex_written(target: &SwitchTarget) -> Result<(), AppError> {
  let config_path = path_for("codex", "config.toml");
  let config_text = fs::read_to_string(&config_path).map_err(|err| AppError::io(&config_path, err))?;
  let config_value: toml::Value = config_text.parse().map_err(|source| AppError::Toml { path: config_path.display().to_string(), source })?;
  let config_table = config_value.as_table().ok_or_else(|| AppError::Message("Codex config.toml is not a TOML table".into()))?;
  let provider_id = config_table.get("model_provider").and_then(|value| value.as_str()).unwrap_or_default();
  if provider_id != "ai8888" {
    return Err(AppError::Message(format!("Codex model_provider mismatch: expected ai8888, got {provider_id}")));
  }
  let providers = config_table
    .get("model_providers")
    .and_then(|value| value.as_table())
    .ok_or_else(|| AppError::Message("Codex config.toml missing model_providers".into()))?;
  let provider = providers
    .get("ai8888")
    .and_then(|value| value.as_table())
    .ok_or_else(|| AppError::Message("Codex config.toml missing model_providers.ai8888".into()))?;
  let base_url = provider.get("base_url").and_then(|value| value.as_str()).unwrap_or_default();
  if base_url != effective_base_url(target) {
    return Err(AppError::Message(format!("Codex base_url mismatch: expected {}, got {}", effective_base_url(target), base_url)));
  }
  let token = provider.get("experimental_bearer_token").and_then(|value| value.as_str()).unwrap_or_default();
  if token != effective_api_key(target) {
    return Err(AppError::Message(format!("Codex experimental_bearer_token mismatch: expected {}, got {token}", effective_api_key(target))));
  }
  Ok(())
}

fn verify_claude_written(target: &SwitchTarget) -> Result<(), AppError> {
  let path = path_for("claude", "settings.json");
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  let value: serde_json::Value = serde_json::from_str(&content).map_err(|err| AppError::json(&path, err))?;
  let env = value.get("env").and_then(|item| item.as_object()).ok_or_else(|| AppError::Message("Claude settings.json missing env".into()))?;
  let base_url = env.get("ANTHROPIC_BASE_URL").and_then(|item| item.as_str()).unwrap_or_default();
  if base_url != effective_claude_base_url(target) {
    return Err(AppError::Message(format!("Claude baseUrl mismatch: expected {}, got {}", effective_claude_base_url(target), base_url)));
  }
  let api_key = env.get("ANTHROPIC_API_KEY").and_then(|item| item.as_str()).unwrap_or_default();
  let auth_token = env.get("ANTHROPIC_AUTH_TOKEN").and_then(|item| item.as_str()).unwrap_or_default();
  if target.local_routing_enabled && target.local_route_preserve_claude_auth {
    if api_key == "PROXY_MANAGED" || auth_token == "PROXY_MANAGED" {
      return Err(AppError::Message("Claude preserved auth unexpectedly replaced with PROXY_MANAGED".into()));
    }
  } else if api_key != effective_api_key(target) || auth_token != effective_api_key(target) {
    return Err(AppError::Message(format!("Claude api key mismatch: expected {}, got api_key={}, auth_token={}", effective_api_key(target), api_key, auth_token)));
  }
  Ok(())
}

fn verify_opencode_written(target: &SwitchTarget) -> Result<(), AppError> {
  let path = path_for("opencode", "opencode.json");
  let value: serde_json::Value = read_json(&path)?;
  let provider = value
    .get("provider")
    .and_then(|item| item.get("ai8888"))
    .ok_or_else(|| AppError::Message("OpenCode opencode.json missing provider.ai8888".into()))?;
  let options = provider.get("options").and_then(|item| item.as_object()).ok_or_else(|| AppError::Message("OpenCode provider.ai8888 missing options".into()))?;
  let base_url = options.get("baseURL").and_then(|item| item.as_str()).unwrap_or_default();
  let api_key = options.get("apiKey").and_then(|item| item.as_str()).unwrap_or_default();
  if base_url != effective_base_url(target) {
    return Err(AppError::Message(format!("OpenCode baseURL mismatch: expected {}, got {base_url}", effective_base_url(target))));
  }
  if api_key != effective_api_key(target) {
    return Err(AppError::Message(format!("OpenCode apiKey mismatch: expected {}, got {api_key}", effective_api_key(target))));
  }
  Ok(())
}

fn verify_openclaw_written(target: &SwitchTarget) -> Result<(), AppError> {
  let path = path_for("openclaw", "openclaw.json");
  let value: serde_json::Value = read_json(&path)?;
  let provider = value
    .get("models")
    .and_then(|item| item.get("providers"))
    .and_then(|item| item.get("ai8888"))
    .ok_or_else(|| AppError::Message("OpenClaw openclaw.json missing models.providers.ai8888".into()))?;
  let base_url = provider.get("baseUrl").and_then(|item| item.as_str()).unwrap_or_default();
  let api_key = provider.get("apiKey").and_then(|item| item.as_str()).unwrap_or_default();
  if base_url != effective_claude_base_url(target) {
    return Err(AppError::Message(format!("OpenClaw baseUrl mismatch: expected {}, got {base_url}", effective_claude_base_url(target))));
  }
  if api_key != effective_api_key(target) {
    return Err(AppError::Message(format!("OpenClaw apiKey mismatch: expected {}, got {api_key}", effective_api_key(target))));
  }
  Ok(())
}

fn verify_hermes_written(target: &SwitchTarget) -> Result<(), AppError> {
  let path = path_for("hermes", "config.yaml");
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  let value: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|err| AppError::Message(format!("Hermes config.yaml parse failed: {err}")))?;
  let providers = value.get("custom_providers").and_then(|item| item.as_sequence()).ok_or_else(|| AppError::Message("Hermes config.yaml missing custom_providers".into()))?;
  let provider = providers
    .iter()
    .find(|item| item.get("name").and_then(|name| name.as_str()) == Some("ai8888"))
    .ok_or_else(|| AppError::Message("Hermes config.yaml missing custom_providers ai8888".into()))?;
  let base_url = provider.get("base_url").and_then(|item| item.as_str()).unwrap_or_default();
  let api_key = provider.get("api_key").and_then(|item| item.as_str()).unwrap_or_default();
  if base_url != effective_claude_base_url(target) {
    return Err(AppError::Message(format!("Hermes base_url mismatch: expected {}, got {base_url}", effective_claude_base_url(target))));
  }
  if api_key != effective_api_key(target) {
    return Err(AppError::Message(format!("Hermes api_key mismatch: expected {}, got {api_key}", effective_api_key(target))));
  }
  Ok(())
}


fn route_model(target: &SwitchTarget, key: &str) -> String {
  target
    .local_route_model_map
    .get(key)
    .filter(|value| !value.trim().is_empty())
    .cloned()
    .or_else(|| target.model.clone())
    .unwrap_or_else(|| "gpt-5.5".into())
}

fn existing_claude_auth_value() -> Option<(String, String)> {
  let settings = read_json_value(&path_for("claude", "settings.json"));
  let env = settings.get("env")?.as_object()?;
  for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
    let value = env.get(key).and_then(|item| item.as_str()).unwrap_or("").trim();
    if !value.is_empty() && value != "PROXY_MANAGED" {
      return Some((key.to_string(), value.to_string()));
    }
  }
  None
}


pub fn detect_local_route_statuses() -> Vec<LocalRouteStatus> {
  vec![detect_codex_local_route(), detect_claude_local_route(), detect_opencode_local_route()]
}

fn status_detail(base_url_matched: bool, proxy_key_matched: bool, missing: bool) -> String {
  if missing {
    return "config missing".into();
  }
  match (base_url_matched, proxy_key_matched) {
    (true, true) => "routed locally, key=PROXY_MANAGED".into(),
    (true, false) => "routed locally, key is not PROXY_MANAGED".into(),
    (false, true) => "key=PROXY_MANAGED but base URL is not local".into(),
    (false, false) => "local route not detected".into(),
  }
}

fn detect_codex_local_route() -> LocalRouteStatus {
  let config_path = path_for("codex", "config.toml");
  let auth_path = path_for("codex", "auth.json");
  let config = fs::read_to_string(&config_path).unwrap_or_default();
  let auth = fs::read_to_string(&auth_path).unwrap_or_default();
  let base_url_matched = config.contains(LOCAL_PROXY_OPENAI_BASE_URL);
  let proxy_key_matched = config.contains("PROXY_MANAGED") || auth.contains("PROXY_MANAGED");
  let oauth_preserved = auth.contains("tokens") || auth.contains("id_token") || auth.contains("refresh_token") || auth.contains("access_token");
  let mcp_preserved = config.contains("[mcp_servers") || config.contains("[mcp_servers.");
  let missing = !config_path.exists() && !auth_path.exists();
  let mut detail = status_detail(base_url_matched, proxy_key_matched, missing);
  if oauth_preserved || mcp_preserved {
    detail.push_str(&format!("; preserved {}{}", if oauth_preserved { "OAuth/Auth " } else { "" }, if mcp_preserved { "MCP" } else { "" }));
  }
  LocalRouteStatus {
    app: "codex".into(),
    detected: base_url_matched && proxy_key_matched,
    config_path: format!("{} | {}", config_path.display(), auth_path.display()),
    base_url_matched,
    proxy_key_matched,
    oauth_preserved,
    mcp_preserved,
    detail,
  }
}

fn detect_claude_local_route() -> LocalRouteStatus {
  let config_path = path_for("claude", "settings.json");
  let content = fs::read_to_string(&config_path).unwrap_or_default();
  let value: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
  let env = value.get("env").and_then(|item| item.as_object());
  let base_url_matched = env
    .and_then(|item| item.get("ANTHROPIC_BASE_URL"))
    .and_then(|item| item.as_str())
    .map(|item| item.trim_end_matches('/') == LOCAL_PROXY_BASE_URL)
    .unwrap_or_else(|| content.contains(LOCAL_PROXY_BASE_URL));
  let proxy_key_matched = env
    .and_then(|item| item.get("ANTHROPIC_AUTH_TOKEN").or_else(|| item.get("ANTHROPIC_API_KEY")))
    .and_then(|item| item.as_str())
    .map(|item| item == "PROXY_MANAGED")
    .unwrap_or_else(|| content.contains("PROXY_MANAGED"));
  let missing = !config_path.exists();
  LocalRouteStatus {
    app: "claude".into(),
    detected: base_url_matched && proxy_key_matched,
    config_path: config_path.display().to_string(),
    base_url_matched,
    proxy_key_matched,
    oauth_preserved: false,
    mcp_preserved: false,
    detail: status_detail(base_url_matched, proxy_key_matched, missing),
  }
}

fn detect_opencode_local_route() -> LocalRouteStatus {
  let config_path = path_for("opencode", "opencode.json");
  let content = fs::read_to_string(&config_path).unwrap_or_default();
  let value: serde_json::Value = serde_json::from_str(&content).unwrap_or_default();
  let options = value
    .get("provider")
    .and_then(|item| item.get("ai8888"))
    .and_then(|item| item.get("options"))
    .and_then(|item| item.as_object());
  let base_url_matched = options
    .and_then(|item| item.get("baseURL"))
    .and_then(|item| item.as_str())
    .map(|item| item.trim_end_matches('/') == LOCAL_PROXY_OPENAI_BASE_URL)
    .unwrap_or_else(|| content.contains(LOCAL_PROXY_OPENAI_BASE_URL));
  let proxy_key_matched = options
    .and_then(|item| item.get("apiKey"))
    .and_then(|item| item.as_str())
    .map(|item| item == "PROXY_MANAGED")
    .unwrap_or_else(|| content.contains("PROXY_MANAGED"));
  let missing = !config_path.exists();
  LocalRouteStatus {
    app: "opencode".into(),
    detected: base_url_matched && proxy_key_matched,
    config_path: config_path.display().to_string(),
    base_url_matched,
    proxy_key_matched,
    oauth_preserved: false,
    mcp_preserved: false,
    detail: status_detail(base_url_matched, proxy_key_matched, missing),
  }
}

fn ai8888_backup_path(path: &std::path::Path) -> std::path::PathBuf {
  let file_name = path.file_name().and_then(|name| name.to_str()).unwrap_or("config");
  path.with_file_name(format!("{file_name}.ai8888-switch.bak"))
}

fn restore_file_from_backup(path: &std::path::Path) -> Result<bool, AppError> {
  let backup = ai8888_backup_path(path);
  if !backup.exists() {
    return Ok(false);
  }
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).map_err(|err| AppError::io(parent, err))?;
  }
  fs::copy(&backup, path).map_err(|err| AppError::io(path, err))?;
  Ok(true)
}

pub fn restore_local_route_backups() -> Result<Vec<(String, String)>, AppError> {
  let targets = [
    (path_for("codex", "config.toml"), "Codex config.toml"),
    (path_for("claude", "settings.json"), "Claude settings.json"),
    (path_for("opencode", "opencode.json"), "OpenCode opencode.json"),
    (path_for("gemini", ".env"), "Gemini .env"),
    (path_for("gemini", "settings.json"), "Gemini settings.json"),
  ];
  let mut restored = Vec::new();
  for (path, label) in targets {
    if restore_file_from_backup(&path)? {
      restored.push((path.display().to_string(), format!("restored backup {label}")));
    }
  }
  Ok(restored)
}

pub fn cleanup_local_route_takeover() -> Result<Vec<(String, String)>, AppError> {
  let mut changed = Vec::new();
  cleanup_codex_takeover(&mut changed)?;
  cleanup_claude_takeover(&mut changed)?;
  cleanup_opencode_takeover(&mut changed)?;
  cleanup_gemini_takeover(&mut changed)?;
  write_json(&local_route_manifest_path(), &LocalRouteManifest {
    profile_name: LOCAL_PROXY_PROFILE_NAME.to_string(),
    updated_at: SystemTime::now().duration_since(UNIX_EPOCH).map(|value| value.as_secs()).unwrap_or(0),
    entries: Vec::new(),
  })?;
    changed.push((local_route_manifest_path().display().to_string(), "local route manifest cleaned".into()));
  Ok(changed)
}

fn cleanup_codex_takeover(changed: &mut Vec<(String, String)>) -> Result<(), AppError> {
  let config_path = path_for("codex", "config.toml");
  if config_path.exists() {
    let content = fs::read_to_string(&config_path).map_err(|err| AppError::io(&config_path, err))?;
    if content.contains("experimental_bearer_token") || content.contains(LOCAL_PROXY_OPENAI_BASE_URL) {
      let mut doc = content.parse::<toml::Value>().unwrap_or_else(|_| toml::Value::Table(TomlTable::new()));
      if let Some(table) = doc.as_table_mut() {
        if let Some(providers) = table.get_mut("model_providers").and_then(|value| value.as_table_mut()) {
          if let Some(provider) = providers.get_mut("ai8888").and_then(|value| value.as_table_mut()) {
            provider.remove("experimental_bearer_token");
            provider.insert("base_url".into(), toml::Value::String(OPENAI_BASE_URL.into()));
          }
        }
      }
      write_text(&config_path, &toml::to_string_pretty(&doc).unwrap_or_default())?;
      changed.push((config_path.display().to_string(), "Codex cleanup".into()));
    }
  }
  Ok(())
}
fn cleanup_claude_takeover(changed: &mut Vec<(String, String)>) -> Result<(), AppError> {
  let path = path_for("claude", "settings.json");
  if !path.exists() {
    return Ok(());
  }
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  if !content.contains(LOCAL_PROXY_BASE_URL) && !content.contains("PROXY_MANAGED") {
    return Ok(());
  }
  let mut value: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
  if let Some(env) = value.get_mut("env").and_then(|item| item.as_object_mut()) {
    if env.get("ANTHROPIC_BASE_URL").and_then(|item| item.as_str()).map(|item| item.trim_end_matches("/") == LOCAL_PROXY_BASE_URL).unwrap_or(false) {
      env.remove("ANTHROPIC_BASE_URL");
    }
    for key in ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"] {
      if env.get(key).and_then(|item| item.as_str()) == Some("PROXY_MANAGED") {
        env.remove(key);
      }
    }
    for key in [
      "ANTHROPIC_MODEL",
      "ANTHROPIC_DEFAULT_HAIKU_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
      "ANTHROPIC_DEFAULT_SONNET_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
      "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
    ] {
      env.remove(key);
    }
  }
  write_json(&path, &value)?;
  changed.push((path.display().to_string(), "Claude settings cleaned".into()));
  Ok(())
}

fn cleanup_opencode_takeover(changed: &mut Vec<(String, String)>) -> Result<(), AppError> {
  let path = path_for("opencode", "opencode.json");
  if !path.exists() {
    return Ok(());
  }
  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  if !content.contains(LOCAL_PROXY_OPENAI_BASE_URL) && !content.contains("PROXY_MANAGED") {
    return Ok(());
  }
  let mut value: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
  let mut changed_config = false;
  if let Some(provider) = value
    .get_mut("provider")
    .and_then(|item| item.get_mut("ai8888"))
    .and_then(|item| item.as_object_mut())
  {
    if let Some(options) = provider.get_mut("options").and_then(|item| item.as_object_mut()) {
      if options.get("baseURL").and_then(|item| item.as_str()).map(|item| item.trim_end_matches('/') == LOCAL_PROXY_OPENAI_BASE_URL).unwrap_or(false) {
        options.insert("baseURL".to_string(), json!(OPENAI_BASE_URL));
        changed_config = true;
      }
      if options.get("apiKey").and_then(|item| item.as_str()) == Some("PROXY_MANAGED") {
        options.remove("apiKey");
        changed_config = true;
      }
    }
  }
  if changed_config {
    write_json(&path, &value)?;
    changed.push((path.display().to_string(), "OpenCode opencode.json cleaned".into()));
  }
  Ok(())
}

fn cleanup_gemini_takeover(changed: &mut Vec<(String, String)>) -> Result<(), AppError> {
  let env_path = path_for("gemini", ".env");
  if env_path.exists() {
    let content = fs::read_to_string(&env_path).map_err(|err| AppError::io(&env_path, err))?;
    if content.contains(LOCAL_PROXY_BASE_URL) || content.contains("PROXY_MANAGED") {
      let lines = content
        .lines()
        .filter(|line| {
          let trimmed = line.trim_start();
          !(trimmed.starts_with("GOOGLE_GEMINI_BASE_URL=") || trimmed.starts_with("GEMINI_API_KEY=") || trimmed.starts_with("GEMINI_MODEL="))
        })
        .collect::<Vec<_>>()
        .join("\n");
      write_text(&env_path, &(lines + "\n"))?;
      changed.push((env_path.display().to_string(), "Gemini .env cleaned".into()));
    }
  }
  let settings_path = path_for("gemini", "settings.json");
  if settings_path.exists() {
    let content = fs::read_to_string(&settings_path).map_err(|err| AppError::io(&settings_path, err))?;
    if content.contains(LOCAL_PROXY_BASE_URL) || content.contains("PROXY_MANAGED") || content.contains("selectedType") {
      let mut value: serde_json::Value = serde_json::from_str(&content).unwrap_or_else(|_| json!({}));
      if let Some(obj) = value.as_object_mut() {
        if let Some(security) = obj.get_mut("security").and_then(|item| item.as_object_mut()) {
          if let Some(auth) = security.get_mut("auth").and_then(|item| item.as_object_mut()) {
            auth.insert("selectedType".to_string(), json!("oauth-personal"));
          }
        }
      }
      write_json(&settings_path, &value)?;
      changed.push((settings_path.display().to_string(), "Gemini settings cleaned".into()));
    }
  }
  Ok(())
}


#[cfg(test)]
mod tests {
  use super::*;

  fn with_test_home<T>(name: &str, test: impl FnOnce() -> T) -> T {
    let _guard = crate::config::test_home_guard();
    let root = std::env::temp_dir().join(format!("ai8888-switch-{name}-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir_all(&root).expect("create test home");
    let old = std::env::var_os("AI8888_SWITCH_TEST_HOME");
    std::env::set_var("AI8888_SWITCH_TEST_HOME", &root);
    let result = test();
    match old {
      Some(value) => std::env::set_var("AI8888_SWITCH_TEST_HOME", value),
      None => std::env::remove_var("AI8888_SWITCH_TEST_HOME"),
    }
    let _ = fs::remove_dir_all(root);
    result
  }

  fn target(tool: &str) -> SwitchTarget {
    SwitchTarget {
      tool: tool.to_string(),
      profile_name: "AI8888".to_string(),
      base_url: "https://sub.ai8888.shop/v1".to_string(),
      api_key: "sk-test".to_string(),
      model: Some("gpt-test".to_string()),
      review_model: Some("gpt-review".to_string()),
      token_type: None,
      local_routing_enabled: false,
      local_route_apps: Vec::new(),
      local_route_model_map: HashMap::new(),
      local_route_preserve_claude_auth: false,
      local_route_only: false,
    }
  }

  #[test]
  fn codex_write_updates_config_without_touching_auth_json() {
    with_test_home("codex", || {
      let config_path = path_for("codex", "config.toml");
      let auth_path = path_for("codex", "auth.json");
      write_text(&config_path, "model_provider = \"old\"\n[model_providers.old]\nname = \"Old\"\nbase_url = \"https://old.example/v1\"\n").expect("seed codex config");
      write_text(&auth_path, "{\"OPENAI_API_KEY\":\"keep-auth\",\"tokens\":{\"access_token\":\"oauth\"}}\n").expect("seed codex auth");
      write_switch_target(&target("codex")).expect("write codex");
      let config = fs::read_to_string(&config_path).expect("read codex config");
      let auth = fs::read_to_string(&auth_path).expect("read codex auth");
      assert_eq!(auth, "{\"OPENAI_API_KEY\":\"keep-auth\",\"tokens\":{\"access_token\":\"oauth\"}}\n");
      assert!(config.contains("model_provider = \"ai8888\""));
      assert!(config.contains("review_model = \"gpt-review\""));
      assert!(config.contains("[model_providers.ai8888]"));
      assert!(config.contains("base_url = \"https://sub.ai8888.shop/v1\""));
      assert!(config.contains("experimental_bearer_token = \"sk-test\""));
      assert!(config.contains("wire_api = \"responses\""));
    });
  }


  #[test]
  fn codex_official_switch_preserves_auth_and_other_routes() {
    with_test_home("codex-official", || {
      let config_path = path_for("codex", "config.toml");
      let auth_path = path_for("codex", "auth.json");
      write_text(
        &config_path,
        "model_provider = \"ai8888\"\nmodel = \"gpt-ai\"\nreview_model = \"gpt-review\"\nopenai_base_url = \"https://proxy.example/v1\"\n[model_providers.ai8888]\nbase_url = \"https://sub.ai8888.shop/v1\"\nexperimental_bearer_token = \"sk-secret\"\n",
      )
      .expect("seed codex config");
      write_text(&auth_path, "{\"tokens\":{\"access_token\":\"oauth\"}}\n").expect("seed codex auth");
      write_json(
        &local_route_manifest_path(),
        &LocalRouteManifest {
          profile_name: LOCAL_PROXY_PROFILE_NAME.into(),
          updated_at: 1,
          entries: vec![
            LocalRouteEntry { app: "codex".into(), ..Default::default() },
            LocalRouteEntry { app: "claude".into(), ..Default::default() },
          ],
        },
      )
      .expect("seed route manifest");

      activate_codex_official().expect("activate official");
      let config = fs::read_to_string(&config_path).expect("read codex config");
      let value = config.parse::<toml::Value>().expect("parse codex config");
      assert_eq!(value.get("model_provider").and_then(toml::Value::as_str), Some("openai"));
      assert!(value.get("model").is_none());
      assert!(value.get("review_model").is_none());
      assert!(value.get("openai_base_url").is_none());
      assert_eq!(
        value.get("model_providers").and_then(|item| item.get("ai8888")).and_then(|item| item.get("experimental_bearer_token")).and_then(toml::Value::as_str),
        Some("sk-secret"),
      );
      assert_eq!(fs::read_to_string(&auth_path).expect("read codex auth"), "{\"tokens\":{\"access_token\":\"oauth\"}}\n");
      let manifest: LocalRouteManifest = read_json(&local_route_manifest_path()).expect("read route manifest");
      assert_eq!(manifest.entries.len(), 1);
      assert_eq!(manifest.entries[0].app, "claude");
    });
  }

  #[test]
  fn claude_write_updates_env_and_preserves_other_settings() {
    with_test_home("claude", || {
      let path = path_for("claude", "settings.json");
      write_json(&path, &json!({
        "permissions": { "allow": ["Bash(ls)"] },
        "env": { "OLD": "remove-me" }
      })).expect("seed claude");
      write_switch_target(&target("claude")).expect("write claude");
      let value: serde_json::Value = read_json(&path).expect("read claude");
      assert_eq!(value["permissions"]["allow"][0], "Bash(ls)");
      assert_eq!(value["env"]["ANTHROPIC_BASE_URL"], "https://sub.ai8888.shop/v1");
      assert_eq!(value["env"]["ANTHROPIC_API_KEY"], "sk-test");
      assert_eq!(value["env"]["ANTHROPIC_AUTH_TOKEN"], "sk-test");
      assert_eq!(value["env"]["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"], "1");
      assert!(value["env"].get("OLD").is_none());
    });
  }



  #[test]
  fn local_route_manifest_defaults_are_not_used_when_ui_passes_claude_only() {
    let mut t = target("claude");
    t.local_routing_enabled = true;
    t.local_route_apps = vec!["claude".to_string()];
    t.local_route_model_map = HashMap::from([("sonnet".to_string(), "gpt-sonnet".to_string())]);
    let manifest = build_local_route_manifest(&t);
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].app, "claude");
    assert_eq!(manifest.entries[0].local_api_key, "PROXY_MANAGED");
    assert_eq!(manifest.entries[0].model_map.get("sonnet").map(String::as_str), Some("gpt-sonnet"));
  }


  #[test]
  fn local_route_only_opencode_does_not_modify_unselected_codex_or_claude() {
    with_test_home("route-opencode-only", || {
      let codex_path = path_for("codex", "config.toml");
      let claude_path = path_for("claude", "settings.json");
      write_text(&codex_path, r#"model_provider = "old"
[model_providers.old]
base_url = "https://old.example/v1"
"#).expect("seed codex");
      write_json(&claude_path, &json!({ "env": { "ANTHROPIC_BASE_URL": "https://old.example", "ANTHROPIC_API_KEY": "old" }, "permissions": { "allow": ["Bash(ls)"] } })).expect("seed claude");

      let mut t = target("claude");
      t.local_routing_enabled = true;
      t.local_route_apps = vec!["opencode".to_string()];
      write_local_routed_targets(&t).expect("write opencode route only");

      let codex = fs::read_to_string(&codex_path).expect("read codex");
      let claude: serde_json::Value = read_json(&claude_path).expect("read claude");
      let opencode: serde_json::Value = read_json(&path_for("opencode", "opencode.json")).expect("read opencode");
      assert!(codex.contains("https://old.example/v1"));
      assert!(!codex.contains("PROXY_MANAGED"));
      assert_eq!(claude["env"]["ANTHROPIC_BASE_URL"], "https://old.example");
      assert_eq!(claude["env"]["ANTHROPIC_API_KEY"], "old");
      assert_eq!(opencode["provider"]["ai8888"]["options"]["baseURL"], LOCAL_PROXY_OPENAI_BASE_URL);
      assert_eq!(opencode["provider"]["ai8888"]["options"]["apiKey"], "PROXY_MANAGED");
    });
  }

  #[test]
  fn local_route_can_write_and_detect_codex_claude_and_opencode() {
    with_test_home("route-all", || {
      let mut t = target("codex");
      t.local_routing_enabled = true;
      t.local_route_apps = vec!["codex".to_string(), "claude".to_string(), "opencode".to_string()];
      t.local_route_model_map = HashMap::from([("sonnet".to_string(), "gpt-sonnet".to_string())]);
      write_local_routed_targets(&t).expect("write all routes");

      let manifest = build_local_route_manifest(&t);
      assert_eq!(manifest.entries.iter().map(|entry| entry.app.as_str()).collect::<Vec<_>>(), vec!["codex", "claude", "opencode"]);
      let statuses = detect_local_route_statuses();
      assert!(statuses.iter().all(|status| status.detected), "statuses: {statuses:?}");
    });
  }

  #[test]
  fn root_endpoint_is_normalized_for_openai_compatible_tools() {
    with_test_home("root-endpoint", || {
      let mut t = target("codex");
      t.base_url = "https://ai8888.shop".to_string();
      write_switch_target(&t).expect("write codex root endpoint");
      let codex = fs::read_to_string(path_for("codex", "config.toml")).expect("read codex");
      assert!(codex.contains("base_url = \"https://ai8888.shop/v1\""));

      t.tool = "opencode".to_string();
      write_switch_target(&t).expect("write opencode root endpoint");
      let opencode: serde_json::Value = read_json(&path_for("opencode", "opencode.json")).expect("read opencode");
      assert_eq!(opencode["provider"]["ai8888"]["options"]["baseURL"], "https://ai8888.shop/v1");
    });
  }

  #[test]
  fn cleanup_local_route_takeover_cleans_opencode_proxy_marker() {
    with_test_home("cleanup-opencode", || {
      write_json(&path_for("opencode", "opencode.json"), &json!({
        "plugin": ["keep-me"],
        "provider": {
          "existing": { "name": "Existing" },
          "ai8888": {
            "npm": "@ai-sdk/openai-compatible",
            "name": "AI8888",
            "options": { "baseURL": LOCAL_PROXY_OPENAI_BASE_URL, "apiKey": "PROXY_MANAGED" },
            "models": { "gpt-test": { "name": "gpt-test" } }
          }
        }
      })).expect("seed opencode");

      cleanup_local_route_takeover().expect("cleanup");
      let value: serde_json::Value = read_json(&path_for("opencode", "opencode.json")).expect("read opencode");
      assert_eq!(value["plugin"][0], "keep-me");
      assert!(value["provider"].get("existing").is_some());
      assert_eq!(value["provider"]["ai8888"]["options"]["baseURL"], OPENAI_BASE_URL);
      assert!(value["provider"]["ai8888"]["options"].get("apiKey").is_none());
    });
  }
  #[test]
  fn cleanup_local_route_takeover_removes_proxy_markers() {
    with_test_home("cleanup", || {
      write_text(&path_for("codex", "config.toml"), "model_provider = \"ai8888\"\n[model_providers.ai8888]\nname = \"AI8888\"\nbase_url = \"http://127.0.0.1:15888/v1\"\nexperimental_bearer_token = \"PROXY_MANAGED\"\n").expect("seed codex");
      write_json(&path_for("claude", "settings.json"), &json!({
        "env": {
          "ANTHROPIC_BASE_URL": "http://127.0.0.1:15888",
          "ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED",
          "ANTHROPIC_API_KEY": "PROXY_MANAGED",
          "ANTHROPIC_MODEL": "gpt-test"
        }
      })).expect("seed claude");
      write_text(&path_for("gemini", ".env"), "GOOGLE_GEMINI_BASE_URL=\"http://127.0.0.1:15888\"\nGEMINI_API_KEY=\"PROXY_MANAGED\"\nGEMINI_MODEL=\"gpt-test\"\nKEEP=\"yes\"\n").expect("seed gemini env");
      let changed = cleanup_local_route_takeover().expect("cleanup");
      assert!(!changed.is_empty());
      let codex = fs::read_to_string(path_for("codex", "config.toml")).expect("read codex");
      let claude: serde_json::Value = read_json(&path_for("claude", "settings.json")).expect("read claude");
      let gemini = fs::read_to_string(path_for("gemini", ".env")).expect("read gemini");
      assert!(!codex.contains("PROXY_MANAGED"));
      assert!(codex.contains("https://sub.ai8888.shop/v1"));
      assert!(claude["env"].get("ANTHROPIC_BASE_URL").is_none());
      assert!(claude["env"].get("ANTHROPIC_AUTH_TOKEN").is_none());
      assert!(!gemini.contains("PROXY_MANAGED"));
      assert!(!gemini.contains("GOOGLE_GEMINI_BASE_URL"));
      assert!(gemini.contains("KEEP=\"yes\""));
    });
  }

  #[test]
  fn opencode_write_upserts_ai8888_provider_and_preserves_existing_config() {
    with_test_home("opencode", || {
      let path = path_for("opencode", "opencode.json");
      write_json(&path, &json!({
        "$schema": "https://opencode.ai/config.json",
        "plugin": ["keep-me"],
        "provider": { "existing": { "name": "Existing" } }
      })).expect("seed opencode");
      write_switch_target(&target("opencode")).expect("write opencode");
      let value: serde_json::Value = read_json(&path).expect("read opencode");
      assert_eq!(value["plugin"][0], "keep-me");
      assert!(value["provider"].get("existing").is_some());
      assert_eq!(value["provider"]["ai8888"]["npm"], "@ai-sdk/openai-compatible");
      assert_eq!(value["provider"]["ai8888"]["options"]["baseURL"], "https://sub.ai8888.shop/v1");
      assert_eq!(value["provider"]["ai8888"]["options"]["apiKey"], "sk-test");
      assert!(value.get("model").is_none());
      assert_eq!(value["provider"]["ai8888"]["models"]["gpt-test"]["name"], "gpt-test");
    });
  }

  #[test]
  fn openclaw_write_upserts_models_provider_and_preserves_existing_config() {
    with_test_home("openclaw", || {
      let path = path_for("openclaw", "openclaw.json");
      write_json(&path, &json!({
        "tools": { "keep": true },
        "models": { "mode": "merge", "providers": { "existing": { "apiKey": "old" } } }
      })).expect("seed openclaw");
      write_switch_target(&target("openclaw")).expect("write openclaw");
      let value: serde_json::Value = read_json(&path).expect("read openclaw");
      assert_eq!(value["tools"]["keep"], true);
      assert!(value["models"]["providers"].get("existing").is_some());
      assert_eq!(value["models"]["providers"]["ai8888"]["baseUrl"], "https://sub.ai8888.shop/v1");
      assert_eq!(value["models"]["providers"]["ai8888"]["apiKey"], "sk-test");
      assert_eq!(value["models"]["providers"]["ai8888"]["api"], "openai-completions");
      assert_eq!(value["agents"]["defaults"]["model"]["primary"], "ai8888/gpt-test");
    });
  }

  #[test]
  fn hermes_write_upserts_custom_provider_and_preserves_existing_sections() {
    with_test_home("hermes", || {
      let path = path_for("hermes", "config.yaml");
      write_text(&path, "memory:\n  enabled: true\ncustom_providers:\n  - name: existing\n    base_url: https://old.example/v1\n").expect("seed hermes");
      write_switch_target(&target("hermes")).expect("write hermes");
      let raw = fs::read_to_string(&path).expect("read hermes raw");
      let value: serde_yaml::Value = serde_yaml::from_str(&raw).expect("parse hermes");
      assert_eq!(value["memory"]["enabled"].as_bool(), Some(true));
      let providers = value["custom_providers"].as_sequence().expect("providers");
      assert!(providers.iter().any(|item| item["name"].as_str() == Some("existing")));
      let ai8888 = providers.iter().find(|item| item["name"].as_str() == Some("ai8888")).expect("ai8888 provider");
      assert_eq!(ai8888["base_url"].as_str(), Some("https://sub.ai8888.shop/v1"));
      assert_eq!(ai8888["api_key"].as_str(), Some("sk-test"));
      assert_eq!(ai8888["api_mode"].as_str(), Some("chat_completions"));
      assert_eq!(value["model"]["provider"].as_str(), Some("ai8888"));
      assert_eq!(value["model"]["default"].as_str(), Some("gpt-test"));
    });
  }
}

