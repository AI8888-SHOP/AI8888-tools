use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
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
      (
        guard.running,
        guard.mode.clone(),
        guard.result.clone(),
        guard.succeeded,
        guard.output.iter().cloned().collect::<Vec<_>>(),
      )
    };
    let (active_provider, ai8888_config_available, credential_store) = inspect_codex_config();

    match resolve_codex_executable() {
      Ok((executable, version)) => {
        if runtime.0 {
          return CodexAuthStatus {
            cli_available: true,
            cli_version: Some(version),
            authenticated: false,
            auth_method: "checking".into(),
            status_message: "正在等待 Codex 完成官方登录".into(),
            active_provider,
            ai8888_config_available,
            credential_store,
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
          active_provider,
          ai8888_config_available,
          credential_store,
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
        active_provider,
        ai8888_config_available,
        credential_store,
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
          child.kill().map_err(|error| format!("取消 Codex 登录失败：{error}"))?;
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
      let _ = child.kill();
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

fn resolve_codex_executable() -> Result<(PathBuf, String), String> {
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

fn hidden_command(executable: &PathBuf) -> Command {
  let mut command = Command::new(executable);
  #[cfg(windows)]
  {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    command.creation_flags(CREATE_NO_WINDOW);
  }
  command
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
  let message = clean_output(output).unwrap_or_else(|| {
    if output.status.success() { "Codex 已登录".into() } else { "Codex 尚未登录".into() }
  });
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

fn inspect_codex_config() -> (String, bool, String) {
  let path = path_for("codex", "config.toml");
  let value = std::fs::read_to_string(path)
    .ok()
    .and_then(|content| content.parse::<toml::Value>().ok())
    .filter(|value| value.is_table())
    .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));
  let provider = value.get("model_provider").and_then(toml::Value::as_str).unwrap_or("openai");
  let active_provider = match provider {
    "openai" => "official",
    "ai8888" => "ai8888",
    _ => "custom",
  }
  .to_string();
  let ai8888_config_available = value
    .get("model_providers")
    .and_then(|item| item.get("ai8888"))
    .and_then(toml::Value::as_table)
    .map(|table| table.get("base_url").and_then(toml::Value::as_str).is_some() && table.get("experimental_bearer_token").and_then(toml::Value::as_str).is_some())
    .unwrap_or(false);
  let credential_store = value
    .get("cli_auth_credentials_store")
    .and_then(toml::Value::as_str)
    .unwrap_or("default")
    .to_string();
  (active_provider, ai8888_config_available, credential_store)
}

#[cfg(test)]
mod tests {
  use super::*;

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
