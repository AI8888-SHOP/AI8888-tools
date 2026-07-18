use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;
use serde_json::Value;

use crate::codex_sessions;

const MAX_SESSION_FILES: usize = 5_000;
const MAX_LISTED_SESSIONS: usize = 1_000;
const MAX_SCAN_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MESSAGE_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_SCAN_DEPTH: usize = 10;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedSessionMeta {
  pub source: String,
  pub source_label: String,
  pub session_id: String,
  pub title: Option<String>,
  pub summary: Option<String>,
  pub project_dir: Option<String>,
  pub created_at: Option<String>,
  pub last_active_at: Option<String>,
  pub model: Option<String>,
  pub source_path: String,
  pub resume_command: Option<String>,
  pub archived: bool,
  pub modified_at: u64,
  pub message_count: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedSessionMessage {
  pub role: String,
  pub content: String,
  pub timestamp: Option<String>,
  pub message_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionSource {
  Codex,
  Claude,
  OpenCode,
  OpenClaw,
  Hermes,
  Gemini,
}

impl SessionSource {
  const ALL: [Self; 6] = [Self::Codex, Self::Claude, Self::OpenCode, Self::OpenClaw, Self::Hermes, Self::Gemini];

  fn key(self) -> &'static str {
    match self {
      Self::Codex => "codex",
      Self::Claude => "claude",
      Self::OpenCode => "opencode",
      Self::OpenClaw => "openclaw",
      Self::Hermes => "hermes",
      Self::Gemini => "gemini",
    }
  }

  fn label(self) -> &'static str {
    match self {
      Self::Codex => "Codex",
      Self::Claude => "Claude Code",
      Self::OpenCode => "OpenCode",
      Self::OpenClaw => "OpenClaw",
      Self::Hermes => "Hermes",
      Self::Gemini => "Gemini CLI",
    }
  }

  fn parse(value: &str) -> Option<Self> {
    match value.trim().to_ascii_lowercase().replace(['-', '_', ' '], "").as_str() {
      "codex" | "openai" => Some(Self::Codex),
      "claude" | "claudecode" => Some(Self::Claude),
      "opencode" => Some(Self::OpenCode),
      "openclaw" | "clawdbot" | "moltbot" => Some(Self::OpenClaw),
      "hermes" | "hermesagent" => Some(Self::Hermes),
      "gemini" | "geminicli" => Some(Self::Gemini),
      _ => None,
    }
  }
}

#[derive(Debug, Clone)]
struct RootSpec {
  root: PathBuf,
  extensions: &'static [&'static str],
}

#[tauri::command]
pub fn app_list_unified_sessions(
  source: Option<String>,
  query: Option<String>,
) -> Result<Vec<UnifiedSessionMeta>, String> {
  let sources = selected_sources(source.as_deref())?;
  let query = query.unwrap_or_default().trim().to_ascii_lowercase();
  let mut sessions = Vec::new();
  for source in sources {
    let mut source_sessions = scan_source(source);
    if !query.is_empty() {
      source_sessions.retain(|session| session_matches(session, &query));
    }
    sessions.extend(source_sessions);
  }
  sessions.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
  sessions.truncate(MAX_LISTED_SESSIONS);
  Ok(sessions)
}

#[tauri::command]
pub fn app_get_unified_session_messages(
  source: String,
  source_path: String,
) -> Result<Vec<UnifiedSessionMessage>, String> {
  let source = SessionSource::parse(&source).ok_or_else(|| format!("unsupported session source: {source}"))?;
  if source == SessionSource::Codex {
    return codex_sessions::load_messages(&source_path)
      .map(|messages| messages.into_iter().map(|message| UnifiedSessionMessage {
        role: message.role,
        content: message.content,
        timestamp: message.timestamp,
        message_type: Some("message".into()),
      }).collect())
      .map_err(|error| error.to_string());
  }
  let path = validate_source_path(source, &source_path)?;
  if source == SessionSource::OpenCode {
    if let Ok(messages) = load_opencode_messages(&path) {
      if !messages.is_empty() {
        return Ok(messages);
      }
    }
  }
  load_messages_from_file(source, &path)
}

fn selected_sources(source: Option<&str>) -> Result<Vec<SessionSource>, String> {
  let Some(source) = source.map(str::trim).filter(|value| !value.is_empty()) else {
    return Ok(SessionSource::ALL.to_vec());
  };
  if source.eq_ignore_ascii_case("all") {
    return Ok(SessionSource::ALL.to_vec());
  }
  SessionSource::parse(source).map(|source| vec![source]).ok_or_else(|| format!("unsupported session source: {source}"))
}

fn scan_source(source: SessionSource) -> Vec<UnifiedSessionMeta> {
  if source == SessionSource::Codex {
    return codex_sessions::scan_sessions().into_iter().map(|session| UnifiedSessionMeta {
      source: source.key().into(),
      source_label: source.label().into(),
      session_id: session.session_id,
      title: session.title,
      summary: session.summary,
      project_dir: session.project_dir,
      created_at: session.created_at,
      last_active_at: session.last_active_at,
      model: session.model_provider,
      source_path: session.source_path,
      resume_command: Some(session.resume_command),
      archived: session.archived,
      modified_at: session.modified_at,
      message_count: None,
    }).collect();
  }

  let mut files = Vec::new();
  let mut seen = HashSet::new();
  for spec in source_roots(source) {
    collect_session_files(&spec.root, spec.extensions, 0, &mut files, &mut seen);
    if files.len() >= MAX_SESSION_FILES {
      break;
    }
  }
  files.into_iter().filter_map(|path| parse_session_file(source, &path)).collect()
}

fn source_roots(source: SessionSource) -> Vec<RootSpec> {
  let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
  let xdg_data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local").join("share"));
  let app_data = std::env::var_os("APPDATA").map(PathBuf::from);
  let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
  let mut roots = Vec::new();
  let mut add = |root: PathBuf, extensions: &'static [&'static str]| {
    if !roots.iter().any(|item: &RootSpec| item.root == root) {
      roots.push(RootSpec { root, extensions });
    }
  };

  match source {
    SessionSource::Codex => {}
    SessionSource::Claude => {
      let config = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home.join(".claude"));
      add(config.join("projects"), &["jsonl"]);
    }
    SessionSource::OpenCode => {
      if let Some(root) = std::env::var_os("OPENCODE_DATA_HOME").map(PathBuf::from) {
        add(root.join("storage").join("session"), &["json"]);
      }
      add(xdg_data.join("opencode").join("storage").join("session"), &["json"]);
      add(home.join(".opencode").join("storage").join("session"), &["json"]);
      if let Some(root) = local_app_data {
        add(root.join("opencode").join("storage").join("session"), &["json"]);
      }
    }
    SessionSource::OpenClaw => {
      let root = std::env::var_os("OPENCLAW_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".openclaw"));
      add(root.join("agents"), &["jsonl"]);
      add(root.join("sessions"), &["json", "jsonl"]);
      add(home.join(".clawdbot").join("agents"), &["jsonl"]);
      add(home.join(".moltbot").join("agents"), &["jsonl"]);
    }
    SessionSource::Hermes => {
      let root = std::env::var_os("HERMES_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".hermes"));
      add(root.join("sessions"), &["json", "jsonl"]);
      add(root.join("conversations"), &["json", "jsonl"]);
      add(root.join("state").join("sessions"), &["json", "jsonl"]);
    }
    SessionSource::Gemini => {
      let root = std::env::var_os("GEMINI_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".gemini"));
      add(root.join("tmp"), &["json"]);
      add(root.join("history"), &["json", "jsonl"]);
      add(root.join("sessions"), &["json", "jsonl"]);
      if let Some(root) = app_data {
        add(root.join("gemini").join("sessions"), &["json", "jsonl"]);
      }
    }
  }
  roots
}

fn collect_session_files(
  root: &Path,
  extensions: &[&str],
  depth: usize,
  files: &mut Vec<PathBuf>,
  seen: &mut HashSet<PathBuf>,
) {
  if depth > MAX_SCAN_DEPTH || files.len() >= MAX_SESSION_FILES || !root.is_dir() {
    return;
  }
  let Ok(entries) = fs::read_dir(root) else { return; };
  for entry in entries.flatten() {
    if files.len() >= MAX_SESSION_FILES { break; }
    let path = entry.path();
    let Ok(file_type) = entry.file_type() else { continue; };
    if file_type.is_symlink() { continue; }
    if file_type.is_dir() {
      collect_session_files(&path, extensions, depth + 1, files, seen);
      continue;
    }
    if !file_type.is_file() || !has_extension(&path, extensions) || is_non_session_file(&path) { continue; }
    let Ok(metadata) = entry.metadata() else { continue; };
    if metadata.len() == 0 || metadata.len() > MAX_SCAN_FILE_BYTES { continue; }
    let canonical = path.canonicalize().unwrap_or(path);
    if seen.insert(canonical.clone()) { files.push(canonical); }
  }
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
  path.extension().and_then(|extension| extension.to_str())
    .map(|extension| extensions.iter().any(|allowed| extension.eq_ignore_ascii_case(allowed)))
    .unwrap_or(false)
}

fn is_non_session_file(path: &Path) -> bool {
  let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("").to_ascii_lowercase();
  matches!(name.as_str(), "sessions.json" | "index.json" | "config.json" | "settings.json" | "projects.json")
    || name.ends_with(".metadata.json")
}

fn parse_session_file(source: SessionSource, path: &Path) -> Option<UnifiedSessionMeta> {
  let modified_at = modified_at_ms(path).unwrap_or(0);
  let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("");
  let (metadata, messages) = if extension.eq_ignore_ascii_case("jsonl") {
    parse_jsonl_summary(source, path).ok()?
  } else {
    let value = read_json_value(path, MAX_SCAN_FILE_BYTES).ok()?;
    let messages = messages_from_json(source, &value);
    (value, messages)
  };

  if source == SessionSource::Claude && is_claude_sidechain(&metadata) {
    return None;
  }

  let session_id = find_string(&metadata, &["sessionId", "session_id", "conversationId", "conversation_id", "id"])
    .or_else(|| infer_session_id(path))?;
  if session_id.trim().is_empty() {
    return None;
  }

  let first_user = messages.iter().find(|message| message.role == "user");
  let last_message = messages.last();
  let title = find_string(&metadata, &["title", "name", "displayName"])
    .or_else(|| first_user.map(|message| truncate(&message.content, 96)))
    .or_else(|| project_from_value(&metadata).and_then(|project| path_basename(&project)));
  let summary = find_string(&metadata, &["summary", "description"])
    .or_else(|| last_message.map(|message| truncate(&message.content, 180)));
  let created_at = find_timestamp(&metadata, &["createdAt", "created_at", "startTime", "start_time", "timestamp"])
    .or_else(|| metadata.get("time").and_then(|time| find_timestamp(time, &["created", "start"])))
    .or_else(|| messages.first().and_then(|message| message.timestamp.clone()));
  let last_active_at = find_timestamp(&metadata, &["updatedAt", "updated_at", "lastUpdated", "last_active_at"])
    .or_else(|| metadata.get("time").and_then(|time| find_timestamp(time, &["updated", "completed", "created"])))
    .or_else(|| messages.iter().rev().find_map(|message| message.timestamp.clone()));
  let project_dir = project_from_value(&metadata);
  let model = find_string_recursive(&metadata, &["model", "modelId", "model_id"], 3);
  let archived = path.components()
    .any(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase().contains("archiv"));

  Some(UnifiedSessionMeta {
    source: source.key().into(),
    source_label: source.label().into(),
    resume_command: resume_command(source, &session_id),
    session_id,
    title: title.filter(|value| !value.trim().is_empty()).map(|value| truncate(&value, 96)),
    summary: summary.filter(|value| !value.trim().is_empty()).map(|value| truncate(&value, 180)),
    project_dir,
    created_at,
    last_active_at,
    model,
    source_path: path.to_string_lossy().to_string(),
    archived,
    modified_at,
    message_count: (!messages.is_empty()).then_some(messages.len()),
  })
}

fn parse_jsonl_summary(source: SessionSource, path: &Path) -> Result<(Value, Vec<UnifiedSessionMessage>), String> {
  let file = File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
  let mut metadata = Value::Object(Default::default());
  let mut messages = Vec::new();
  for (index, line) in BufReader::new(file).lines().enumerate() {
    let Ok(line) = line else { continue; };
    let Ok(value) = serde_json::from_str::<Value>(&line) else { continue; };
    if index == 0 || looks_like_session_metadata(&value) {
      merge_metadata(&mut metadata, &value);
    }
    if let Some(message) = message_from_value(source, &value) {
      messages.push(message);
    }
  }
  Ok((metadata, messages))
}

fn load_messages_from_file(source: SessionSource, path: &Path) -> Result<Vec<UnifiedSessionMessage>, String> {
  let metadata = path.metadata().map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
  if metadata.len() > MAX_MESSAGE_FILE_BYTES {
    return Err(format!("session file exceeds the {} MiB safety limit", MAX_MESSAGE_FILE_BYTES / 1024 / 1024));
  }
  if path.extension().and_then(|value| value.to_str()).map(|value| value.eq_ignore_ascii_case("jsonl")).unwrap_or(false) {
    return parse_jsonl_summary(source, path).map(|(_, messages)| messages);
  }
  let value = read_json_value(path, MAX_MESSAGE_FILE_BYTES)?;
  Ok(messages_from_json(source, &value))
}

fn read_json_value(path: &Path, max_bytes: u64) -> Result<Value, String> {
  let mut file = File::open(path).map_err(|error| format!("failed to open {}: {error}", path.display()))?;
  let metadata = file.metadata().map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
  if metadata.len() > max_bytes {
    return Err(format!("session file exceeds the {} MiB safety limit", max_bytes / 1024 / 1024));
  }
  let mut content = String::with_capacity(metadata.len().min(4 * 1024 * 1024) as usize);
  file.read_to_string(&mut content).map_err(|error| format!("failed to read {}: {error}", path.display()))?;
  serde_json::from_str(&content).map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn messages_from_json(source: SessionSource, value: &Value) -> Vec<UnifiedSessionMessage> {
  if let Some(items) = value.as_array() {
    return items.iter().filter_map(|item| message_from_value(source, item)).collect();
  }
  for key in ["messages", "history", "conversation", "turns", "entries"] {
    if let Some(items) = value.get(key).and_then(Value::as_array) {
      return items.iter().filter_map(|item| message_from_value(source, item)).collect();
    }
  }
  value.as_object().and_then(|_| message_from_value(source, value)).into_iter().collect()
}

fn message_from_value(_source: SessionSource, value: &Value) -> Option<UnifiedSessionMessage> {
  let envelope_type = value.get("type").and_then(Value::as_str).unwrap_or("");
  if matches!(envelope_type, "session" | "session_meta" | "summary" | "file-history-snapshot" | "progress") {
    return None;
  }
  let payload = value.get("message").filter(|value| value.is_object()).unwrap_or(value);
  let role = find_string(payload, &["role", "author", "speaker"])
    .or_else(|| payload.get("author").and_then(|author| find_string(author, &["role", "type"])))
    .or_else(|| role_from_type(envelope_type))?;
  let role = normalize_role(&role);
  let content_value = payload.get("content")
    .or_else(|| payload.get("parts"))
    .or_else(|| payload.get("text"))
    .or_else(|| payload.get("output"))
    .or_else(|| payload.get("result"));
  let content = content_value.map(extract_content).unwrap_or_default();
  if content.trim().is_empty() {
    return None;
  }
  let timestamp = find_timestamp(value, &["timestamp", "createdAt", "created_at", "time"])
    .or_else(|| find_timestamp(payload, &["timestamp", "createdAt", "created_at", "time"]));
  Some(UnifiedSessionMessage {
    role,
    content,
    timestamp,
    message_type: (!envelope_type.is_empty()).then(|| envelope_type.to_string()),
  })
}

fn role_from_type(value: &str) -> Option<String> {
  match value.to_ascii_lowercase().as_str() {
    "user" | "human" => Some("user".into()),
    "assistant" | "ai" | "model" => Some("assistant".into()),
    "system" => Some("system".into()),
    "info" | "warning" | "error" => Some("system".into()),
    "tool" | "tool_result" | "toolresult" => Some("tool".into()),
    _ => None,
  }
}

fn normalize_role(value: &str) -> String {
  match value.to_ascii_lowercase().as_str() {
    "human" => "user".into(),
    "ai" | "model" | "bot" => "assistant".into(),
    "function" | "tool_result" | "toolresult" => "tool".into(),
    other => other.to_string(),
  }
}

fn extract_content(value: &Value) -> String {
  match value {
    Value::Null => String::new(),
    Value::String(text) => text.clone(),
    Value::Number(number) => number.to_string(),
    Value::Bool(value) => value.to_string(),
    Value::Array(items) => items.iter().map(extract_content).filter(|part| !part.trim().is_empty()).collect::<Vec<_>>().join("\n"),
    Value::Object(object) => {
      let item_type = object.get("type").and_then(Value::as_str).unwrap_or("");
      if matches!(item_type, "tool_use" | "tool_call" | "function_call") {
        let name = object.get("name").and_then(Value::as_str).unwrap_or("tool");
        return format!("[Tool: {name}]");
      }
      for key in ["text", "content", "parts", "output", "result", "message", "value"] {
        if let Some(value) = object.get(key) {
          let content = extract_content(value);
          if !content.trim().is_empty() { return content; }
        }
      }
      String::new()
    }
  }
}

fn load_opencode_messages(session_path: &Path) -> Result<Vec<UnifiedSessionMessage>, String> {
  let session = read_json_value(session_path, MAX_MESSAGE_FILE_BYTES)?;
  let session_id = find_string(&session, &["id", "sessionId", "session_id"]).or_else(|| infer_session_id(session_path));
  let Some(session_id) = session_id else { return Ok(Vec::new()); };
  if !safe_path_segment(&session_id) {
    return Err("invalid OpenCode session id".into());
  }
  let Some(storage_root) = opencode_storage_root(session_path) else { return Ok(Vec::new()); };
  let message_root = storage_root.join("message").join(&session_id);
  if !message_root.is_dir() {
    return Ok(messages_from_json(SessionSource::OpenCode, &session));
  }

  let mut message_files = Vec::new();
  let mut seen = HashSet::new();
  collect_session_files(&message_root, &["json"], 0, &mut message_files, &mut seen);
  message_files.sort();
  let mut messages = Vec::new();
  for message_path in message_files {
    let Ok(info) = read_json_value(&message_path, MAX_MESSAGE_FILE_BYTES) else { continue; };
    let role = find_string(&info, &["role"]).map(|value| normalize_role(&value)).unwrap_or_else(|| "unknown".into());
    let timestamp = find_timestamp(&info, &["createdAt", "created_at", "time"])
      .or_else(|| info.get("time").and_then(|time| find_timestamp(time, &["created", "completed"])));
    let message_id = find_string(&info, &["id", "messageId", "message_id"]).or_else(|| infer_session_id(&message_path));
    let mut content = extract_content(info.get("content").unwrap_or(&Value::Null));
    if let Some(message_id) = message_id.filter(|value| safe_path_segment(value)) {
      let part_root = storage_root.join("part").join(&message_id);
      let mut part_files = Vec::new();
      let mut part_seen = HashSet::new();
      collect_session_files(&part_root, &["json"], 0, &mut part_files, &mut part_seen);
      part_files.sort();
      let part_content = part_files.into_iter()
        .filter_map(|path| read_json_value(&path, MAX_MESSAGE_FILE_BYTES).ok())
        .map(|part| extract_content(&part))
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
      if !part_content.is_empty() {
        content = part_content;
      }
    }
    if !content.trim().is_empty() {
      messages.push(UnifiedSessionMessage { role, content, timestamp, message_type: Some("message".into()) });
    }
  }
  Ok(messages)
}

fn opencode_storage_root(path: &Path) -> Option<PathBuf> {
  for ancestor in path.ancestors() {
    if ancestor.file_name().and_then(|name| name.to_str()) == Some("session") {
      return ancestor.parent().map(Path::to_path_buf);
    }
  }
  None
}

fn validate_source_path(source: SessionSource, source_path: &str) -> Result<PathBuf, String> {
  let requested = PathBuf::from(source_path);
  if !requested.is_file() {
    return Err("session source is not a file".into());
  }
  let canonical = requested.canonicalize()
    .map_err(|error| format!("failed to resolve session source {}: {error}", requested.display()))?;
  for spec in source_roots(source) {
    if !spec.root.is_dir() { continue; }
    let Ok(root) = spec.root.canonicalize() else { continue; };
    if canonical.starts_with(&root) && has_extension(&canonical, spec.extensions) {
      return Ok(canonical);
    }
  }
  Err(format!("session file is outside trusted {} session directories", source.label()))
}

fn session_matches(session: &UnifiedSessionMeta, query: &str) -> bool {
  let metadata = [
    Some(session.session_id.as_str()),
    session.title.as_deref(),
    session.summary.as_deref(),
    session.project_dir.as_deref(),
    session.model.as_deref(),
    Some(session.source_path.as_str()),
  ];
  if metadata.into_iter().flatten().any(|value| value.to_ascii_lowercase().contains(query)) {
    return true;
  }
  app_get_unified_session_messages(session.source.clone(), session.source_path.clone())
    .map(|messages| messages.into_iter().any(|message| message.content.to_ascii_lowercase().contains(query)))
    .unwrap_or(false)
}

fn resume_command(source: SessionSource, session_id: &str) -> Option<String> {
  if !safe_cli_argument(session_id) { return None; }
  match source {
    SessionSource::Codex => Some(format!("codex resume {session_id}")),
    SessionSource::Claude => Some(format!("claude --resume {session_id}")),
    SessionSource::OpenCode => Some(format!("opencode -s {session_id}")),
    SessionSource::OpenClaw => Some(format!("openclaw tui --session {session_id}")),
    SessionSource::Hermes => Some(format!("hermes --resume {session_id}")),
    SessionSource::Gemini => Some(format!("gemini --resume {session_id}")),
  }
}

fn safe_cli_argument(value: &str) -> bool {
  !value.is_empty()
    && value.len() <= 256
    && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn safe_path_segment(value: &str) -> bool {
  safe_cli_argument(value) && value != "." && value != ".."
}

fn merge_metadata(target: &mut Value, candidate: &Value) {
  let Some(target) = target.as_object_mut() else { return; };
  let Some(candidate) = candidate.as_object() else { return; };
  for key in [
    "id", "sessionId", "session_id", "conversationId", "conversation_id", "cwd", "directory", "projectDir",
    "project_dir", "title", "name", "summary", "model", "modelId", "timestamp", "createdAt", "updatedAt",
    "startTime", "lastUpdated", "isSidechain", "agentId",
  ] {
    if !target.contains_key(key) {
      if let Some(value) = candidate.get(key) {
        target.insert(key.to_string(), value.clone());
      }
    }
  }
}

fn looks_like_session_metadata(value: &Value) -> bool {
  matches!(value.get("type").and_then(Value::as_str), Some("session" | "session_meta"))
    || value.get("sessionId").is_some()
    || value.get("session_id").is_some()
}

fn is_claude_sidechain(value: &Value) -> bool {
  value.get("isSidechain").and_then(Value::as_bool) == Some(true)
    || value.get("agentId").and_then(Value::as_str).map(|value| !value.is_empty()).unwrap_or(false)
}

fn find_string(value: &Value, keys: &[&str]) -> Option<String> {
  let object = value.as_object()?;
  for key in keys {
    if let Some(text) = object.get(*key).and_then(value_as_string) {
      if !text.trim().is_empty() { return Some(text); }
    }
  }
  None
}

fn find_string_recursive(value: &Value, keys: &[&str], depth: usize) -> Option<String> {
  if let Some(value) = find_string(value, keys) { return Some(value); }
  if depth == 0 { return None; }
  let object = value.as_object()?;
  for key in ["metadata", "config", "message", "session"] {
    if let Some(value) = object.get(key) {
      if let Some(found) = find_string_recursive(value, keys, depth - 1) { return Some(found); }
    }
  }
  None
}

fn find_timestamp(value: &Value, keys: &[&str]) -> Option<String> {
  let object = value.as_object()?;
  for key in keys {
    let Some(value) = object.get(*key) else { continue; };
    if let Some(text) = value_as_string(value) { return Some(text); }
    if let Some(created) = value.as_object().and_then(|value| value.get("created")).and_then(value_as_string) {
      return Some(created);
    }
  }
  None
}

fn value_as_string(value: &Value) -> Option<String> {
  match value {
    Value::String(value) => Some(value.clone()),
    Value::Number(value) => Some(value.to_string()),
    _ => None,
  }
}

fn project_from_value(value: &Value) -> Option<String> {
  find_string(value, &["cwd", "directory", "projectDir", "project_dir", "workingDirectory", "working_directory"])
    .or_else(|| value.get("project").and_then(|project| find_string(project, &["path", "directory", "root"])))
}

fn path_basename(value: &str) -> Option<String> {
  Path::new(value).file_name().and_then(|name| name.to_str()).filter(|name| !name.is_empty()).map(str::to_string)
}

fn infer_session_id(path: &Path) -> Option<String> {
  let stem = path.file_stem()?.to_str()?;
  let id = stem.strip_prefix("session-")
    .or_else(|| stem.strip_prefix("session_"))
    .or_else(|| stem.strip_prefix("chat-"))
    .unwrap_or(stem);
  (!id.is_empty()).then(|| id.to_string())
}

fn truncate(value: &str, max_chars: usize) -> String {
  let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
  let mut chars = compact.chars();
  let truncated = chars.by_ref().take(max_chars).collect::<String>();
  if chars.next().is_some() { format!("{truncated}...") } else { truncated }
}

fn modified_at_ms(path: &Path) -> Option<u64> {
  path.metadata().ok()?.modified().ok()?.duration_since(UNIX_EPOCH).ok()
    .map(|duration| duration.as_millis() as u64)
}
