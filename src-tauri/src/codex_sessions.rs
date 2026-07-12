use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{atomic_write, path_for};
use crate::error::AppError;

const VSCODE_CONTEXT_PREFIX: &str = "# Context from my IDE setup:";
const CODEX_REQUEST_MARKER: &str = "my request for codex";
const DEFAULT_MODEL_PROVIDER: &str = "ai8888";

#[derive(Default)]
struct ModelProviderLookup {
  aliases: HashMap<String, String>,
  active_key: Option<String>,
  active_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionMeta {
  pub session_id: String,
  pub title: Option<String>,
  pub summary: Option<String>,
  pub project_dir: Option<String>,
  pub created_at: Option<String>,
  pub last_active_at: Option<String>,
  pub model_provider: Option<String>,
  pub model_provider_key: Option<String>,
  pub source_path: String,
  pub resume_command: String,
  pub archived: bool,
  pub modified_at: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionMessage {
  pub role: String,
  pub content: String,
  pub timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionVisibilityRepairRequest {
  pub session_id: String,
  pub source_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexSessionVisibilityRepairOutcome {
  pub session_id: String,
  pub source_path: String,
  pub success: bool,
  pub changed: bool,
  pub error: Option<String>,
}


pub fn scan_sessions() -> Vec<CodexSessionMeta> {
  let providers = load_model_provider_lookup();
  let mut files = Vec::new();
  for root in session_roots() {
    collect_jsonl_files(&root, &mut files);
  }

  let mut sessions = files
    .into_iter()
    .filter_map(|path| parse_session(&path, &providers))
    .collect::<Vec<_>>();

  sessions.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
  sessions
}

pub fn load_messages(source_path: &str) -> Result<Vec<CodexSessionMessage>, AppError> {
  let path = validate_session_source(source_path)?;
  let file = File::open(&path).map_err(|err| AppError::io(&path, err))?;
  let reader = BufReader::new(file);
  let mut messages = Vec::new();

  for line in reader.lines() {
    let line = match line {
      Ok(line) => line,
      Err(_) => continue,
    };
    let value: Value = match serde_json::from_str(&line) {
      Ok(value) => value,
      Err(_) => continue,
    };

    if value.get("type").and_then(Value::as_str) != Some("response_item") {
      continue;
    }

    let Some(payload) = value.get("payload") else { continue; };
    let payload_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
    let (role, content) = match payload_type {
      "message" => {
        let role = payload.get("role").and_then(Value::as_str).unwrap_or("unknown").to_string();
        let content = payload.get("content").map(extract_text).unwrap_or_default();
        (role, content)
      }
      "function_call" | "custom_tool_call" => {
        let name = payload.get("name").and_then(Value::as_str).unwrap_or("tool");
        ("assistant".to_string(), format!("[Tool: {name}]"))
      }
      "function_call_output" | "custom_tool_call_output" => {
        let output = payload.get("output").and_then(Value::as_str).unwrap_or("").to_string();
        ("tool".to_string(), output)
      }
      _ => continue,
    };

    if content.trim().is_empty() {
      continue;
    }

    messages.push(CodexSessionMessage {
      role,
      content,
      timestamp: value.get("timestamp").and_then(Value::as_str).map(str::to_string),
    });
  }

  Ok(messages)
}

pub fn launch_resume(session_id: &str, cwd: Option<&str>, model_provider_key: Option<&str>) -> Result<(), AppError> {
  if session_id.trim().is_empty() || session_id.chars().any(|ch| ch.is_control()) {
    return Err(AppError::Message("invalid Codex session id".into()));
  }
  let model_provider_key = validate_model_provider_key(model_provider_key)?;

  #[cfg(target_os = "windows")]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x00000010;

    let mut command = std::process::Command::new("cmd");
    command.args(["/K", "codex", "resume"]);
    if let Some(provider) = model_provider_key {
      command.args(["-c", &format!("model_provider={provider}")]);
    }
    command.arg(session_id);
    command.creation_flags(CREATE_NEW_CONSOLE);
    if let Some(cwd) = cwd.and_then(existing_dir) {
      command.current_dir(cwd);
    }
    command.spawn().map_err(|err| AppError::Message(format!("failed to launch Codex: {err}")))?;
    Ok(())
  }

  #[cfg(not(target_os = "windows"))]
  {
    let mut command = std::process::Command::new("codex");
    command.arg("resume");
    if let Some(provider) = model_provider_key {
      command.args(["-c", &format!("model_provider={provider}")]);
    }
    command.arg(session_id);
    if let Some(cwd) = cwd.and_then(existing_dir) {
      command.current_dir(cwd);
    }
    match command.spawn() {
      Ok(_) => Ok(()),
      Err(err) => Err(AppError::Message(format!(
        "failed to launch Codex resume on this platform ({err}). Copy and run the resume command manually instead."
      ))),
    }
  }
}

pub fn repair_visibility(requests: &[CodexSessionVisibilityRepairRequest]) -> Vec<CodexSessionVisibilityRepairOutcome> {
  let providers = load_model_provider_lookup();
  // Codex filters/resumes by provider key (e.g. "ai8888"), not display name (e.g. "AI8888").
  let visible_provider = providers
    .active_key
    .as_deref()
    .unwrap_or(DEFAULT_MODEL_PROVIDER);
  requests
    .iter()
    .map(|request| match repair_session_visibility(&request.session_id, &request.source_path, visible_provider, &providers) {
      Ok(changed) => CodexSessionVisibilityRepairOutcome {
        session_id: request.session_id.clone(),
        source_path: request.source_path.clone(),
        success: true,
        changed,
        error: None,
      },
      Err(error) => CodexSessionVisibilityRepairOutcome {
        session_id: request.session_id.clone(),
        source_path: request.source_path.clone(),
        success: false,
        changed: false,
        error: Some(error.to_string()),
      },
    })
    .collect()
}

fn repair_session_visibility(session_id: &str, source_path: &str, visible_provider: &str, providers: &ModelProviderLookup) -> Result<bool, AppError> {
  if session_id.trim().is_empty() || session_id.chars().any(|ch| ch.is_control()) {
    return Err(AppError::Message("invalid Codex session id".into()));
  }

  let path = validate_session_source(source_path)?;
  let meta = parse_session(&path, providers).ok_or_else(|| AppError::Message("failed to parse Codex session metadata".into()))?;
  if meta.session_id != session_id {
    return Err(AppError::Message(format!("Codex session ID mismatch: expected {session_id}, found {}", meta.session_id)));
  }

  let content = fs::read_to_string(&path).map_err(|err| AppError::io(&path, err))?;
  let mut saw_session_meta = false;
  let mut changed = false;
  let mut next_lines = Vec::new();

  for line in content.lines() {
    let mut value: Value = match serde_json::from_str(line) {
      Ok(value) => value,
      Err(_) => {
        next_lines.push(line.to_string());
        continue;
      }
    };

    let is_session_meta = value.get("type").and_then(Value::as_str) == Some("session_meta");
    if !is_session_meta {
      next_lines.push(line.to_string());
      continue;
    }

    saw_session_meta = true;
    let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) else {
      next_lines.push(line.to_string());
      continue;
    };

    let recorded_id = payload
      .get("id")
      .and_then(Value::as_str)
      .or_else(|| payload.get("session_id").and_then(Value::as_str));
    if let Some(id) = recorded_id {
      if id != session_id {
        return Err(AppError::Message(format!("Codex session ID mismatch: expected {session_id}, found {id}")));
      }
    }

    let current_provider = payload.get("model_provider").and_then(Value::as_str);
    // Already stored as the active provider key: leave it alone.
    // Display names / aliases that differ are rewritten to the active key.
    if current_provider == Some(visible_provider) {
      next_lines.push(line.to_string());
      continue;
    }

    payload.insert("model_provider".to_string(), Value::String(visible_provider.to_string()));
    let serialized = serde_json::to_string(&value).map_err(|err| AppError::Message(err.to_string()))?;
    next_lines.push(serialized);
    changed = true;
  }

  if !saw_session_meta {
    return Err(AppError::Message("Codex session metadata line not found".into()));
  }

  if changed {
    let mut next = next_lines.join("\n");
    if content.ends_with('\n') {
      next.push('\n');
    }
    atomic_write(&path, next.as_bytes())?;
  }

  Ok(changed)
}
fn existing_dir(path: &str) -> Option<PathBuf> {
  let path = PathBuf::from(path);
  path.is_dir().then_some(path)
}

fn codex_dir() -> PathBuf {
  std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| path_for("codex", ""))
}

fn load_model_provider_lookup() -> ModelProviderLookup {
  let path = codex_dir().join("config.toml");
  let Ok(content) = fs::read_to_string(path) else { return ModelProviderLookup::default(); };
  let Ok(config) = toml::from_str::<toml::Value>(&content) else { return ModelProviderLookup::default(); };
  let active_key = config.get("model_provider").and_then(toml::Value::as_str);
  let mut lookup = ModelProviderLookup::default();
  lookup.active_key = active_key.map(str::to_string);
  if let Some(key) = active_key {
    lookup.aliases.insert(key.to_ascii_lowercase(), key.to_string());
  }

  if let Some(providers) = config.get("model_providers").and_then(toml::Value::as_table) {
    for (key, value) in providers {
      lookup.aliases.insert(key.to_ascii_lowercase(), key.clone());
      if let Some(name) = value.get("name").and_then(toml::Value::as_str) {
        lookup.aliases.insert(name.to_ascii_lowercase(), key.clone());
      }
    }
    lookup.active_name = active_key.and_then(|key| {
      providers
        .get(key)
        .and_then(|value| value.get("name"))
        .and_then(toml::Value::as_str)
        .or(Some(key))
        .map(str::to_string)
    });
  }
  if lookup.active_name.is_none() {
    lookup.active_name = active_key.map(str::to_string);
  }
  if lookup.active_key.is_none() {
    lookup.active_key = active_key.map(str::to_string);
  }

  lookup
}

fn validate_model_provider_key(value: Option<&str>) -> Result<Option<&str>, AppError> {
  let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else { return Ok(None); };
  if value.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')) {
    Ok(Some(value))
  } else {
    Err(AppError::Message("invalid Codex model provider key".into()))
  }
}

fn session_roots() -> Vec<PathBuf> {
  let dir = codex_dir();
  vec![dir.join("sessions"), dir.join("archived_sessions")]
}

fn validate_session_source(source_path: &str) -> Result<PathBuf, AppError> {
  let path = PathBuf::from(source_path);
  let source = path.canonicalize().map_err(|err| AppError::io(&path, err))?;

  for root in session_roots() {
    if !root.exists() {
      continue;
    }
    let root = root.canonicalize().map_err(|err| AppError::io(&root, err))?;
    if source.starts_with(root) {
      return Ok(source);
    }
  }

  Err(AppError::Message("session file is outside Codex session directories".into()))
}

fn parse_session(path: &Path, providers: &ModelProviderLookup) -> Option<CodexSessionMeta> {
  let (head, tail) = read_head_tail_lines(path, 40, 80).ok()?;
  let mut session_id = None;
  let mut project_dir = None;
  let mut created_at = None;
  let mut model_provider = None;
  let mut first_user_message = None;

  for line in &head {
    let value: Value = serde_json::from_str(line).ok()?;
    if created_at.is_none() {
      created_at = value.get("timestamp").and_then(Value::as_str).map(str::to_string);
    }
    if value.get("type").and_then(Value::as_str) == Some("session_meta") {
      if let Some(payload) = value.get("payload") {
        if is_subagent_source(payload.get("source")) {
          return None;
        }
        session_id = session_id.or_else(|| {
          payload
            .get("id")
            .and_then(Value::as_str)
            .or_else(|| payload.get("session_id").and_then(Value::as_str))
            .map(str::to_string)
        });
        project_dir = project_dir.or_else(|| payload.get("cwd").and_then(Value::as_str).map(str::to_string));
        model_provider = model_provider.or_else(|| payload.get("model_provider").and_then(Value::as_str).map(str::to_string));
      }
    }
    if first_user_message.is_none() && value.get("type").and_then(Value::as_str) == Some("response_item") {
      if let Some(payload) = value.get("payload") {
        if payload.get("type").and_then(Value::as_str) == Some("message") && payload.get("role").and_then(Value::as_str) == Some("user") {
          let text = payload.get("content").map(extract_text).unwrap_or_default();
          first_user_message = title_candidate_from_user_message(&text);
        }
      }
    }
  }

  let mut last_active_at = None;
  let mut summary = None;
  for line in tail.iter().rev() {
    let value: Value = match serde_json::from_str(line) {
      Ok(value) => value,
      Err(_) => continue,
    };
    if last_active_at.is_none() {
      last_active_at = value.get("timestamp").and_then(Value::as_str).map(str::to_string);
    }
    if summary.is_none() && value.get("type").and_then(Value::as_str) == Some("response_item") {
      if let Some(payload) = value.get("payload") {
        if payload.get("type").and_then(Value::as_str) == Some("message") {
          let text = payload.get("content").map(extract_text).unwrap_or_default();
          if !text.trim().is_empty() {
            summary = Some(truncate_summary(&text, 180));
          }
        }
      }
    }
  }

  let session_id = session_id.or_else(|| infer_session_id_from_filename(path))?;
  let title = first_user_message
    .map(|value| truncate_summary(&value, 88))
    .or_else(|| project_dir.as_deref().and_then(path_basename).map(str::to_string));
  let modified_at = modified_at_ms(path).unwrap_or(0);
  let source_path = path.to_string_lossy().to_string();
  let archived = source_path.contains("archived_sessions");
  let model_provider_key = model_provider
    .as_deref()
    .and_then(|provider| providers.aliases.get(&provider.to_ascii_lowercase()).cloned().or_else(|| validate_model_provider_key(Some(provider)).ok().flatten().map(str::to_string)));
  let resume_command = match model_provider_key.as_deref() {
    Some(provider) => format!("codex resume -c model_provider={provider} {session_id}"),
    None => format!("codex resume {session_id}"),
  };

  Some(CodexSessionMeta {
    session_id: session_id.clone(),
    title,
    summary,
    project_dir,
    created_at,
    last_active_at,
    model_provider,
    model_provider_key,
    source_path,
    resume_command,
    archived,
    modified_at,
  })
}

fn read_head_tail_lines(path: &Path, head_count: usize, tail_count: usize) -> Result<(Vec<String>, Vec<String>), AppError> {
  let file = File::open(path).map_err(|err| AppError::io(path, err))?;
  let reader = BufReader::new(file);
  let mut head = Vec::new();
  let mut tail = VecDeque::with_capacity(tail_count);

  for line in reader.lines() {
    let line = match line {
      Ok(line) => line,
      Err(_) => continue,
    };
    if head.len() < head_count {
      head.push(line.clone());
    }
    if tail.len() == tail_count {
      tail.pop_front();
    }
    tail.push_back(line);
  }

  Ok((head, tail.into_iter().collect()))
}

fn collect_jsonl_files(root: &Path, files: &mut Vec<PathBuf>) {
  let Ok(entries) = fs::read_dir(root) else { return; };
  for entry in entries.flatten() {
    let path = entry.path();
    if path.is_dir() {
      collect_jsonl_files(&path, files);
    } else if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
      files.push(path);
    }
  }
}

fn extract_text(value: &Value) -> String {
  match value {
    Value::String(text) => text.clone(),
    Value::Array(items) => items
      .iter()
      .filter_map(|item| {
        item.get("text")
          .or_else(|| item.get("content"))
          .and_then(Value::as_str)
          .map(str::to_string)
      })
      .collect::<Vec<_>>()
      .join("\n"),
    Value::Object(_) => value.get("text").and_then(Value::as_str).unwrap_or("").to_string(),
    _ => String::new(),
  }
}

fn title_candidate_from_user_message(text: &str) -> Option<String> {
  let trimmed = text.trim();
  if trimmed.is_empty() || trimmed.starts_with("# AGENTS.md") || trimmed.starts_with("<environment_context>") {
    return None;
  }
  if trimmed.starts_with(VSCODE_CONTEXT_PREFIX) {
    return extract_codex_prompt_from_ide_context(trimmed);
  }
  Some(trimmed.to_string())
}

fn extract_codex_prompt_from_ide_context(text: &str) -> Option<String> {
  let normalized = text.replace("\r\n", "\n");
  let lines = normalized.lines().collect::<Vec<_>>();
  let mut prompt = None;
  for (index, line) in lines.iter().enumerate() {
    let Some(inline) = codex_request_heading_payload(line) else { continue; };
    if inline.is_empty() {
      let following = lines[index + 1..].join("\n").trim().to_string();
      prompt = (!following.is_empty()).then_some(following);
    } else {
      prompt = Some(inline.to_string());
    }
  }
  prompt
}

fn codex_request_heading_payload(line: &str) -> Option<&str> {
  let trimmed = line.trim();
  if !trimmed.starts_with('#') {
    return None;
  }
  let heading = trimmed.trim_start_matches('#').trim_start();
  let lowered = heading.to_ascii_lowercase();
  if !lowered.starts_with(CODEX_REQUEST_MARKER) {
    return None;
  }
  let suffix = heading[CODEX_REQUEST_MARKER.len()..].trim_start();
  if suffix.is_empty() {
    return Some("");
  }
  let separator = suffix.chars().next()?;
  if !matches!(separator, ':' | '-' | '?') {
    return None;
  }
  Some(suffix.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '-' | '?')).trim())
}

fn is_subagent_source(source: Option<&Value>) -> bool {
  source.and_then(Value::as_object).map(|source| source.contains_key("subagent")).unwrap_or(false)
}

fn infer_session_id_from_filename(path: &Path) -> Option<String> {
  let file_name = path.file_name()?.to_string_lossy();
  file_name
    .split(|ch: char| !(ch.is_ascii_hexdigit() || ch == '-'))
    .find(|part| looks_like_uuid(part))
    .map(str::to_string)
}

fn looks_like_uuid(value: &str) -> bool {
  let bytes = value.as_bytes();
  bytes.len() == 36
    && [8, 13, 18, 23].iter().all(|index| bytes[*index] == b'-')
    && bytes.iter().enumerate().all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn path_basename(path: &str) -> Option<&str> {
  Path::new(path).file_name().and_then(|name| name.to_str()).filter(|name| !name.is_empty())
}

fn truncate_summary(value: &str, max_chars: usize) -> String {
  let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
  let mut chars = compact.chars();
  let truncated = chars.by_ref().take(max_chars).collect::<String>();
  if chars.next().is_some() {
    format!("{truncated}...")
  } else {
    truncated
  }
}

fn modified_at_ms(path: &Path) -> Option<u64> {
  let modified = path.metadata().ok()?.modified().ok()?;
  let duration = modified.duration_since(UNIX_EPOCH).ok()?;
  Some(duration.as_millis() as u64)
}

#[allow(dead_code)]
fn now_ms() -> u64 {
  SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_millis() as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::{Mutex, OnceLock};

  fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().expect("test lock")
  }

  fn with_codex_home<T>(test: impl FnOnce(&Path) -> T) -> T {
    let _guard = test_guard();
    let root = std::env::temp_dir().join(format!("ai8888-codex-sessions-{}-{}", std::process::id(), now_ms()));
    fs::create_dir_all(&root).expect("create temp codex home");
    let old = std::env::var_os("CODEX_HOME");
    std::env::set_var("CODEX_HOME", &root);
    let result = test(&root);
    match old {
      Some(value) => std::env::set_var("CODEX_HOME", value),
      None => std::env::remove_var("CODEX_HOME"),
    }
    let _ = fs::remove_dir_all(root);
    result
  }

  #[test]
  fn scans_active_and_archived_codex_sessions() {
    with_codex_home(|root| {
      let active = root.join("sessions").join("2026");
      let archived = root.join("archived_sessions");
      fs::create_dir_all(&active).expect("active dir");
      fs::create_dir_all(&archived).expect("archived dir");
      fs::write(active.join("a.jsonl"), "{\"timestamp\":\"2026-06-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"active-id\",\"cwd\":\"D:/code/app\"}}\n{\"timestamp\":\"2026-06-01T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Fix login\"}}\n").expect("write active");
      fs::write(archived.join("b.jsonl"), "{\"timestamp\":\"2026-06-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"archived-id\",\"cwd\":\"D:/code/app\"}}\n").expect("write archived");

      let sessions = scan_sessions();
      assert!(sessions.iter().any(|session| session.session_id == "active-id" && session.title.as_deref() == Some("Fix login")));
      assert!(sessions.iter().any(|session| session.session_id == "archived-id" && session.archived));
    });
  }

  #[test]
  fn maps_recorded_provider_names_to_resume_config_keys() {
    with_codex_home(|root| {
      fs::write(
        root.join("config.toml"),
        "model_provider = \"ai8888\"\n[model_providers.ai8888]\nname = \"AI8888\"\n[model_providers.custom]\nname = \"Custom Provider\"\n",
      )
      .expect("write config");
      let active = root.join("sessions");
      fs::create_dir_all(&active).expect("active dir");
      fs::write(active.join("ai8888.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"ai8888-id\",\"model_provider\":\"AI8888\"}}\n").expect("write AI8888 session");
      fs::write(active.join("custom.jsonl"), "{\"type\":\"session_meta\",\"payload\":{\"id\":\"custom-id\",\"model_provider\":\"custom\"}}\n").expect("write custom session");

      let sessions = scan_sessions();
      let ai8888 = sessions.iter().find(|session| session.session_id == "ai8888-id").expect("AI8888 session");
      assert_eq!(ai8888.model_provider.as_deref(), Some("AI8888"));
      assert_eq!(ai8888.model_provider_key.as_deref(), Some("ai8888"));
      assert_eq!(ai8888.resume_command, "codex resume -c model_provider=ai8888 ai8888-id");

      let custom = sessions.iter().find(|session| session.session_id == "custom-id").expect("custom session");
      assert_eq!(custom.model_provider_key.as_deref(), Some("custom"));
      assert_eq!(custom.resume_command, "codex resume -c model_provider=custom custom-id");
    });
  }

  #[test]
  fn rejects_message_load_outside_codex_roots() {
    with_codex_home(|_| {
      let outside = std::env::temp_dir().join(format!("outside-{}.jsonl", now_ms()));
      fs::write(&outside, "{}").expect("write outside");
      let error = load_messages(&outside.to_string_lossy()).expect_err("outside path rejected").to_string();
      assert!(error.contains("outside Codex session directories"));
      let _ = fs::remove_file(outside);
    });
  }

  #[test]
  fn repairs_session_visibility_provider_bucket() {
    with_codex_home(|root| {
      let active = root.join("sessions");
      fs::create_dir_all(&active).expect("active dir");
      let source = active.join("session.jsonl");
      fs::write(&source, "{\"timestamp\":\"2026-06-01T00:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"repair-id\",\"cwd\":\"D:/code/app\",\"model_provider\":\"openai\"}}\n{\"timestamp\":\"2026-06-01T00:00:01Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"Fix visibility\"}}\n").expect("write session");

      let outcomes = repair_visibility(&[CodexSessionVisibilityRepairRequest {
        session_id: "repair-id".into(),
        source_path: source.to_string_lossy().to_string(),
      }]);

      assert_eq!(outcomes.len(), 1);
      assert!(outcomes[0].success);
      assert!(outcomes[0].changed);
      let content = fs::read_to_string(&source).expect("read repaired session");
      assert!(content.contains("\"model_provider\":\"ai8888\""));
      assert!(!content.contains("\"model_provider\":\"openai\""));
    });
  }

  #[test]
  fn repairs_visibility_to_active_provider_key_not_display_name() {
    with_codex_home(|root| {
      fs::write(root.join("config.toml"), "model_provider = \"ai8888\"\n[model_providers.ai8888]\nname = \"AI8888\"\n").expect("write config");
      let active = root.join("sessions");
      fs::create_dir_all(&active).expect("active dir");
      let source = active.join("session.jsonl");
      fs::write(&source, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"repair-name-id\",\"model_provider\":\"custom\"}}\n").expect("write session");

      let outcomes = repair_visibility(&[CodexSessionVisibilityRepairRequest {
        session_id: "repair-name-id".into(),
        source_path: source.to_string_lossy().to_string(),
      }]);

      assert!(outcomes[0].success);
      assert!(outcomes[0].changed);
      let content = fs::read_to_string(&source).expect("read repaired session");
      assert!(content.contains("\"model_provider\":\"ai8888\""));
      assert!(!content.contains("\"model_provider\":\"AI8888\""));
      assert!(!content.contains("\"model_provider\":\"custom\""));
    });
  }

  #[test]
  fn does_not_rewrite_session_already_on_active_provider_key() {
    with_codex_home(|root| {
      fs::write(root.join("config.toml"), "model_provider = \"ai8888\"\n[model_providers.ai8888]\nname = \"AI8888\"\n").expect("write config");
      let active = root.join("sessions");
      fs::create_dir_all(&active).expect("active dir");
      let source = active.join("session.jsonl");
      let original = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"already-ok\",\"session_id\":\"already-ok\",\"model_provider\":\"ai8888\"}}\n";
      fs::write(&source, original).expect("write session");

      let outcomes = repair_visibility(&[CodexSessionVisibilityRepairRequest {
        session_id: "already-ok".into(),
        source_path: source.to_string_lossy().to_string(),
      }]);

      assert!(outcomes[0].success);
      assert!(!outcomes[0].changed);
      let content = fs::read_to_string(source).expect("read session");
      assert_eq!(content, original);
    });
  }

  #[test]
  fn normalizes_display_name_provider_to_active_key() {
    with_codex_home(|root| {
      fs::write(root.join("config.toml"), "model_provider = \"ai8888\"\n[model_providers.ai8888]\nname = \"AI8888\"\n").expect("write config");
      let active = root.join("sessions");
      fs::create_dir_all(&active).expect("active dir");
      let source = active.join("session.jsonl");
      fs::write(&source, "{\"type\":\"session_meta\",\"payload\":{\"id\":\"display-name-id\",\"model_provider\":\"AI8888\"}}\n").expect("write session");

      let outcomes = repair_visibility(&[CodexSessionVisibilityRepairRequest {
        session_id: "display-name-id".into(),
        source_path: source.to_string_lossy().to_string(),
      }]);

      assert!(outcomes[0].success);
      assert!(outcomes[0].changed);
      let content = fs::read_to_string(source).expect("read repaired session");
      assert!(content.contains("\"model_provider\":\"ai8888\""));
      assert!(!content.contains("\"model_provider\":\"AI8888\""));
    });
  }

}
