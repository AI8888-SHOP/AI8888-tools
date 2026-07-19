use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::config::path_for;

const MAX_OUTPUT_LINES: usize = 40;
const MAX_OUTPUT_CHARS: usize = 500;
const DEVICE_AUTH_URL: &str = "https://auth.openai.com/codex/device";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAuthStatus {
  pub cli_available: bool,
  pub cli_version: Option<String>,
  pub authenticated: bool,
  pub auth_method: String,
  pub status_message: String,
  pub active_provider: String,
  pub ai8888_config_available: bool,
  pub config_exists: bool,
  pub config_valid: bool,
  pub config_error: Option<String>,
  pub configured_model: Option<String>,
  pub configured_review_model: Option<String>,
  pub configured_base_url: Option<String>,
  pub configured_key_id: Option<u64>,
  pub configured_key_name: Option<String>,
  #[serde(skip_serializing)]
  pub(crate) configured_api_key: Option<String>,
  pub credential_store: String,
  pub config_path: String,
  pub login_running: bool,
  pub login_mode: Option<String>,
  pub login_message: Option<String>,
  pub login_succeeded: Option<bool>,
  pub login_output: Vec<String>,
}

#[derive(Default)]
struct LoginProcessState {
  child: Option<Child>,
  running: bool,
  mode: Option<String>,
  output: VecDeque<String>,
  result: Option<String>,
  succeeded: Option<bool>,
  cancel_requested: bool,
}

#[derive(Default)]
pub struct CodexAuthManager {
  state: Arc<Mutex<LoginProcessState>>,
}

impl CodexAuthManager {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn status(&self) -> CodexAuthStatus {
    let runtime = {
      let guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      (guard.running, guard.mode.clone(), guard.result.clone(), guard.succeeded, guard.output.iter().cloned().collect::<Vec<_>>())
    };
    let config = inspect_codex_config();

    match resolve_codex_executable() {
      Ok((executable, version)) => {
        if runtime.0 {
          return CodexAuthStatus {
            cli_available: true,
            cli_version: Some(version),
            authenticated: false,
            auth_method: "checking".into(),
            status_message: "正在等待 Codex 完成官方登录".into(),
            active_provider: config.active_provider,
            ai8888_config_available: config.ai8888_config_available,
            config_exists: config.config_exists,
            config_valid: config.config_valid,
            config_error: config.config_error,
            configured_model: config.configured_model,
            configured_review_model: config.configured_review_model,
            configured_base_url: config.configured_base_url,
            configured_key_id: None,
            configured_key_name: None,
            configured_api_key: config.configured_api_key,
            credential_store: config.credential_store,
            config_path: path_for("codex", "config.toml").display().to_string(),
            login_running: runtime.0,
            login_mode: runtime.1,
            login_message: runtime.2,
            login_succeeded: runtime.3,
            login_output: runtime.4,
          };
        }

        let login_status = run_codex(&executable, &["login", "status"]);
        let (authenticated, auth_method, status_message) = match login_status {
          Ok(output) => parse_login_status(&output),
          Err(error) => (false, "signed_out".into(), error),
        };
        CodexAuthStatus {
          cli_available: true,
          cli_version: Some(version),
          authenticated,
          auth_method,
          status_message,
          active_provider: config.active_provider,
          ai8888_config_available: config.ai8888_config_available,
          config_exists: config.config_exists,
          config_valid: config.config_valid,
          config_error: config.config_error,
          configured_model: config.configured_model,
          configured_review_model: config.configured_review_model,
          configured_base_url: config.configured_base_url,
          configured_key_id: None,
          configured_key_name: None,
          configured_api_key: config.configured_api_key,
          credential_store: config.credential_store,
          config_path: path_for("codex", "config.toml").display().to_string(),
          login_running: runtime.0,
          login_mode: runtime.1,
          login_message: runtime.2,
          login_succeeded: runtime.3,
          login_output: runtime.4,
        }
      }
      Err(error) => CodexAuthStatus {
        cli_available: false,
        cli_version: None,
        authenticated: false,
        auth_method: "unavailable".into(),
        status_message: error,
        active_provider: config.active_provider,
        ai8888_config_available: config.ai8888_config_available,
        config_exists: config.config_exists,
        config_valid: config.config_valid,
        config_error: config.config_error,
        configured_model: config.configured_model,
        configured_review_model: config.configured_review_model,
        configured_base_url: config.configured_base_url,
        configured_key_id: None,
        configured_key_name: None,
        configured_api_key: config.configured_api_key,
        credential_store: config.credential_store,
        config_path: path_for("codex", "config.toml").display().to_string(),
        login_running: runtime.0,
        login_mode: runtime.1,
        login_message: runtime.2,
        login_succeeded: runtime.3,
        login_output: runtime.4,
      },
    }
  }

  pub fn start_login(&self, mode: &str) -> Result<CodexAuthStatus, String> {
    let mode = match mode {
      "browser" => "browser",
      "device" => "device",
      _ => return Err("不支持的 Codex 登录方式".into()),
    };
    let (executable, _) = resolve_codex_executable()?;

    let mut guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.running {
      return Err("Codex 登录正在进行中".into());
    }

    let mut command = hidden_command(&executable);
    command.arg("login");
    if mode == "device" {
      command.arg("--device-auth");
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| format!("无法启动 Codex 登录：{error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    guard.child = Some(child);
    guard.running = true;
    guard.mode = Some(mode.to_string());
    guard.output.clear();
    guard.result = Some(if mode == "device" {
      "请按登录信息中的提示完成设备授权".into()
    } else {
      "已启动官方浏览器登录，请在浏览器中完成授权".into()
    });
    guard.succeeded = None;
    guard.cancel_requested = false;
    drop(guard);

    if let Some(stdout) = stdout {
      spawn_output_reader(stdout, self.state.clone());
    }
    if let Some(stderr) = stderr {
      spawn_output_reader(stderr, self.state.clone());
    }
    spawn_process_waiter(self.state.clone());

    Ok(self.status())
  }

  pub fn cancel_login(&self) -> Result<CodexAuthStatus, String> {
    {
      let mut guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      if guard.running {
        guard.cancel_requested = true;
        guard.result = Some("正在取消 Codex 登录".into());
        if let Some(child) = guard.child.as_mut() {
          terminate_child(child).map_err(|error| format!("取消 Codex 登录失败：{error}"))?;
        }
      }
    }
    Ok(self.status())
  }

  pub fn logout(&self) -> Result<CodexAuthStatus, String> {
    {
      let guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      if guard.running {
        return Err("请先完成或取消当前 Codex 登录".into());
      }
    }
    let (executable, _) = resolve_codex_executable()?;
    let output = run_codex(&executable, &["logout"])?;
    if !output.status.success() {
      return Err(clean_output(&output).unwrap_or_else(|| "Codex 注销失败".into()));
    }
    {
      let mut guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      guard.result = Some("已通过 Codex CLI 注销官方账户".into());
      guard.succeeded = None;
      guard.output.clear();
    }
    Ok(self.status())
  }
}

impl Drop for CodexAuthManager {
  fn drop(&mut self) {
    let mut guard = self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(child) = guard.child.as_mut() {
      let _ = terminate_child(child);
    }
  }
}

pub fn open_device_auth_page() -> Result<(), String> {
  #[cfg(windows)]
  {
    let mut command = hidden_command(&PathBuf::from("rundll32.exe"));
    command.arg("url.dll,FileProtocolHandler").arg(DEVICE_AUTH_URL);
    command.spawn().map_err(|error| format!("无法打开官方认证页面：{error}"))?;
  }
  #[cfg(target_os = "macos")]
  {
    Command::new("open").arg(DEVICE_AUTH_URL).spawn().map_err(|error| format!("无法打开官方认证页面：{error}"))?;
  }
  #[cfg(all(unix, not(target_os = "macos")))]
  {
    Command::new("xdg-open").arg(DEVICE_AUTH_URL).spawn().map_err(|error| format!("无法打开官方认证页面：{error}"))?;
  }
  Ok(())
}

pub(crate) fn resolve_codex_executable() -> Result<(PathBuf, String), String> {
  let mut candidates = Vec::<PathBuf>::new();
  if let Some(configured) = std::env::var_os("CODEX_BIN").filter(|value| !value.is_empty()) {
    candidates.push(PathBuf::from(configured));
  }
  candidates.push(PathBuf::from("codex"));

  if let Some(home) = dirs::home_dir() {
    candidates.push(home.join(".local").join("bin").join("codex"));
    candidates.push(home.join(".npm-global").join("bin").join("codex"));
  }
  #[cfg(unix)]
  {
    candidates.push(PathBuf::from("/usr/local/bin/codex"));
    candidates.push(PathBuf::from("/usr/bin/codex"));
    candidates.push(PathBuf::from("/snap/bin/codex"));
  }
  #[cfg(target_os = "macos")]
  {
    candidates.push(PathBuf::from("/opt/homebrew/bin/codex"));
  }
  #[cfg(windows)]
  {
    add_windows_codex_candidates(&mut candidates);
    if let Some(app_data) = std::env::var_os("APPDATA") {
      candidates.push(PathBuf::from(app_data).join("npm").join("codex.cmd"));
    }
  }
  #[cfg(windows)]
  {
    candidates.push(PathBuf::from("codex.exe"));
    candidates.push(PathBuf::from("codex.cmd"));
  }

  let mut last_error = None;
  for candidate in candidates {
    match run_codex(&candidate, &["--version"]) {
      Ok(output) if output.status.success() => {
        let version = clean_output(&output).unwrap_or_else(|| "Codex CLI".into());
        return Ok((candidate, version));
      }
      Ok(output) => {
        last_error = clean_output(&output);
      }
      Err(error) => last_error = Some(error),
    }
  }
  Err(last_error.unwrap_or_else(|| "未检测到 Codex CLI，请先安装或将 codex 加入 PATH".into()))
}

#[cfg(windows)]
fn add_windows_codex_candidates(candidates: &mut Vec<PathBuf>) {
  let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
    return;
  };
  let root = PathBuf::from(local_app_data).join("OpenAI").join("Codex").join("bin");

  // The desktop app keeps the current CLI in a changing hash-named directory.
  // Prefer the newest readable copy, then fall back to the older flat install.
  let mut versioned = std::fs::read_dir(&root)
    .ok()
    .into_iter()
    .flat_map(|entries| entries.filter_map(Result::ok))
    .filter_map(|entry| {
      if !entry.file_type().ok()?.is_dir() {
        return None;
      }
      let executable = entry.path().join("codex.exe");
      if !executable.is_file() {
        return None;
      }
      let modified = executable.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
      Some((modified, executable))
    })
    .collect::<Vec<_>>();
  versioned.sort_by(|left, right| right.0.cmp(&left.0));
  candidates.extend(versioned.into_iter().map(|(_, path)| path));
  candidates.push(root.join("codex.exe"));
  candidates.push(root.join("codex.cmd"));
}

fn run_codex(executable: &PathBuf, args: &[&str]) -> Result<Output, String> {
  let mut command = hidden_command(executable);
  command.args(args).stdin(Stdio::null());
  command.output().map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      "未检测到 Codex CLI，请先安装或将 codex 加入 PATH".into()
    } else {
      format!("运行 Codex CLI 失败：{error}")
    }
  })
}

pub(crate) fn command_for_executable(executable: &Path) -> Command {
  #[cfg(windows)]
  if executable
    .extension()
    .and_then(|value| value.to_str())
    .map(|value| value.eq_ignore_ascii_case("cmd") || value.eq_ignore_ascii_case("bat"))
    .unwrap_or(false)
  {
    let mut command = Command::new("cmd.exe");
    command.args(["/D", "/C", "call"]).arg(executable);
    return command;
  }
  Command::new(executable)
}

fn hidden_command(executable: &Path) -> Command {
  let mut command = command_for_executable(executable);
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
  }
  command
}

fn terminate_child(child: &mut Child) -> std::io::Result<()> {
  #[cfg(windows)]
  {
    // npm installs run through cmd.exe; taskkill /T also stops the spawned
    // Node/Codex process so a canceled login cannot finish in the background.
    let pid = child.id().to_string();
    let mut command = hidden_command(Path::new("taskkill.exe"));
    let status = command.args(["/PID", &pid, "/T", "/F"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    if matches!(status, Ok(exit) if exit.success()) {
      return Ok(());
    }
  }
  match child.kill() {
    Ok(()) => Ok(()),
    // The waiter may have reaped the process between taskkill and fallback.
    Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
    Err(error) => Err(error),
  }
}

fn spawn_output_reader<R>(reader: R, state: Arc<Mutex<LoginProcessState>>)
where
  R: Read + Send + 'static,
{
  thread::spawn(move || {
    let mut reader = BufReader::new(reader);
    let mut buffer = Vec::new();
    loop {
      buffer.clear();
      match reader.read_until(b'\n', &mut buffer) {
        Ok(0) => break,
        Ok(_) => {
          let line = String::from_utf8_lossy(&buffer);
          append_output(&state, &line);
        }
        Err(_) => break,
      }
    }
  });
}

fn spawn_process_waiter(state: Arc<Mutex<LoginProcessState>>) {
  thread::spawn(move || loop {
    let finished = {
      let mut guard = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
      let result = match guard.child.as_mut() {
        Some(child) => child.try_wait(),
        None => return,
      };
      match result {
        Ok(Some(exit_status)) => {
          let canceled = guard.cancel_requested;
          guard.child = None;
          guard.running = false;
          guard.succeeded = Some(exit_status.success() && !canceled);
          guard.result = Some(if canceled {
            "Codex 登录已取消".into()
          } else if exit_status.success() {
            "Codex 官方账户登录成功".into()
          } else {
            format!("Codex 登录未完成（退出代码 {}）", exit_status.code().map(|code| code.to_string()).unwrap_or_else(|| "未知".into()))
          });
          true
        }
        Ok(None) => false,
        Err(error) => {
          guard.child = None;
          guard.running = false;
          guard.succeeded = Some(false);
          guard.result = Some(format!("读取 Codex 登录状态失败：{error}"));
          true
        }
      }
    };
    if finished {
      return;
    }
    thread::sleep(Duration::from_millis(250));
  });
}

fn append_output(state: &Arc<Mutex<LoginProcessState>>, line: &str) {
  let line = sanitize_line(line);
  if line.is_empty() {
    return;
  }
  let mut guard = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
  guard.output.push_back(line);
  while guard.output.len() > MAX_OUTPUT_LINES {
    guard.output.pop_front();
  }
}

fn sanitize_line(line: &str) -> String {
  let trimmed = line.trim();
  if trimmed.is_empty() {
    return String::new();
  }
  let lowered = trimmed.to_ascii_lowercase();
  if ["access_token", "refresh_token", "id_token", "authorization_code", "code_verifier", "authorization:", "bearer "]
    .iter()
    .any(|needle| lowered.contains(needle))
    || (trimmed.contains("eyJ") && trimmed.matches('.').count() >= 2)
  {
    return "[已隐藏敏感认证输出]".into();
  }
  trimmed.chars().take(MAX_OUTPUT_CHARS).collect()
}

fn clean_output(output: &Output) -> Option<String> {
  let stdout = String::from_utf8_lossy(&output.stdout);
  let stderr = String::from_utf8_lossy(&output.stderr);
  let combined = format!("{}\n{}", stdout.trim(), stderr.trim());
  let lines = combined.lines().map(sanitize_line).filter(|line| !line.is_empty()).collect::<Vec<_>>();
  (!lines.is_empty()).then(|| lines.join("\n"))
}

fn parse_login_status(output: &Output) -> (bool, String, String) {
  let message = clean_output(output).unwrap_or_else(|| if output.status.success() { "Codex 已登录".into() } else { "Codex 尚未登录".into() });
  let lowered = message.to_ascii_lowercase();
  let authenticated = output.status.success() && !lowered.contains("not logged") && !lowered.contains("signed out");
  let method = if !authenticated {
    "signed_out"
  } else if lowered.contains("chatgpt") {
    "chatgpt"
  } else if lowered.contains("api key") || lowered.contains("api-key") {
    "api_key"
  } else {
    "authenticated"
  };
  (authenticated, method.into(), message)
}

struct CodexConfigInspection {
  active_provider: String,
  ai8888_config_available: bool,
  config_exists: bool,
  config_valid: bool,
  config_error: Option<String>,
  configured_model: Option<String>,
  configured_review_model: Option<String>,
  configured_base_url: Option<String>,
  configured_api_key: Option<String>,
  credential_store: String,
}

fn invalid_codex_config(config_exists: bool, error: String) -> CodexConfigInspection {
  CodexConfigInspection {
    active_provider: "invalid".into(),
    ai8888_config_available: false,
    config_exists,
    config_valid: false,
    config_error: Some(error),
    configured_model: None,
    configured_review_model: None,
    configured_base_url: None,
    configured_api_key: None,
    credential_store: "default".into(),
  }
}

fn inspect_codex_config_content(content: &str, config_exists: bool) -> CodexConfigInspection {
  let value = match content.parse::<toml::Value>() {
    Ok(value) if value.is_table() => value,
    Ok(_) => return invalid_codex_config(config_exists, "Codex config.toml root must be a table".into()),
    Err(error) => return invalid_codex_config(config_exists, format!("Codex config.toml parse failed: {error}")),
  };
  let provider = value.get("model_provider").and_then(toml::Value::as_str).unwrap_or("openai");
  let active_provider = match provider {
    "openai" => "official",
    "ai8888" => "ai8888",
    _ => "custom",
  }
  .to_string();
  let provider_table = value.get("model_providers").and_then(|item| item.get(provider)).and_then(toml::Value::as_table);
  let configured_base_url = provider_table
    .and_then(|table| table.get("base_url"))
    .and_then(toml::Value::as_str)
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_string);
  let configured_api_key = provider_table
    .and_then(|table| table.get("experimental_bearer_token"))
    .and_then(toml::Value::as_str)
    .map(str::trim)
    .filter(|value| !value.is_empty())
    .map(str::to_string);
  let ai8888_config_available = provider == "ai8888" && configured_base_url.is_some() && configured_api_key.is_some();
  let string_value = |key: &str| value.get(key).and_then(toml::Value::as_str).map(str::trim).filter(|value| !value.is_empty()).map(str::to_string);

  CodexConfigInspection {
    active_provider,
    ai8888_config_available,
    config_exists,
    config_valid: true,
    config_error: None,
    configured_model: string_value("model"),
    configured_review_model: string_value("review_model"),
    configured_base_url,
    configured_api_key,
    credential_store: string_value("cli_auth_credentials_store").unwrap_or_else(|| "default".into()),
  }
}

fn inspect_codex_config() -> CodexConfigInspection {
  let path = path_for("codex", "config.toml");
  match std::fs::read_to_string(&path) {
    Ok(content) => inspect_codex_config_content(&content, true),
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => inspect_codex_config_content("", false),
    Err(error) => invalid_codex_config(path.exists(), format!("Cannot read {}: {error}", path.display())),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn reports_malformed_codex_config_instead_of_defaulting_to_official() {
    let status = inspect_codex_config_content("model_provider = \"ai8888\"\n[broken\n", true);
    assert!(!status.config_valid);
    assert_eq!(status.active_provider, "invalid");
    assert!(status.config_error.as_deref().is_some_and(|error| error.contains("parse failed")));
    assert!(!status.ai8888_config_available);
  }

  #[test]
  fn reports_actual_codex_model_provider_and_endpoint() {
    let status = inspect_codex_config_content(
      "model = \"gpt-main\"\nreview_model = \"gpt-review\"\nmodel_provider = \"ai8888\"\n[model_providers.ai8888]\nbase_url = \"https://sub.ai8888.shop/v1\"\nexperimental_bearer_token = \"sk-test\"\n",
      true,
    );
    assert!(status.config_valid);
    assert!(status.ai8888_config_available);
    assert_eq!(status.active_provider, "ai8888");
    assert_eq!(status.configured_model.as_deref(), Some("gpt-main"));
    assert_eq!(status.configured_review_model.as_deref(), Some("gpt-review"));
    assert_eq!(status.configured_base_url.as_deref(), Some("https://sub.ai8888.shop/v1"));
    assert_eq!(status.configured_api_key.as_deref(), Some("sk-test"));
  }

  #[test]
  fn redacts_token_bearing_output() {
    assert_eq!(sanitize_line("access_token=secret"), "[已隐藏敏感认证输出]");
    assert_eq!(sanitize_line("Open this URL: https://auth.openai.com/codex/device"), "Open this URL: https://auth.openai.com/codex/device");
  }

  #[test]
  fn parses_chatgpt_login_status() {
    let output = Output {
      status: success_exit_status(),
      stdout: b"Logged in using ChatGPT\n".to_vec(),
      stderr: Vec::new(),
    };
    let (authenticated, method, _) = parse_login_status(&output);
    assert!(authenticated);
    assert_eq!(method, "chatgpt");
  }

  #[cfg(unix)]
  fn success_exit_status() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
  }

  #[cfg(windows)]
  fn success_exit_status() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(0)
  }
}
