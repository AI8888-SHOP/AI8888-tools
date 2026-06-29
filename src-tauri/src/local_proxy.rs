use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::OnceLock;

use axum::{body::Bytes, extract::{OriginalUri, State}, http::{header, HeaderMap, Method, StatusCode, Uri}, response::{IntoResponse, Response}, routing::{get, post}, Json, Router};
use serde_json::{json, Value};
use tokio::sync::RwLock;

use crate::config::{normalize_api_base_url, LOCAL_PROXY_BASE_URL};
use crate::error::AppError;
use crate::models::SwitchTarget;
use crate::tools::build_local_route_manifest;

#[derive(Debug, Clone)]
struct ProxyRouteConfig {
  base_url: String,
  api_key: String,
  default_model: String,
  model_map: HashMap<String, String>,
}

static PROXY_CONFIG: OnceLock<RwLock<Option<ProxyRouteConfig>>> = OnceLock::new();
static PROXY_STARTED: OnceLock<()> = OnceLock::new();

fn config_lock() -> &'static RwLock<Option<ProxyRouteConfig>> {
  PROXY_CONFIG.get_or_init(|| RwLock::new(None))
}

pub async fn ensure_local_proxy(target: &SwitchTarget) -> Result<(), AppError> {
  let manifest = build_local_route_manifest(target);
  if manifest.entries.is_empty() {
    return Ok(());
  }

  let cfg = ProxyRouteConfig {
    base_url: normalize_api_base_url(&target.base_url),
    api_key: target.api_key.clone(),
    default_model: target.model.clone().unwrap_or_else(|| "gpt-5.5".to_string()),
    model_map: target.local_route_model_map.clone(),
  };
  *config_lock().write().await = Some(cfg);

  PROXY_STARTED.get_or_init(|| {
    tokio::spawn(async move {
      if let Err(err) = run_proxy().await {
        eprintln!("local proxy stopped: {err}");
      }
    });
  });

  Ok(())
}

async fn run_proxy() -> Result<(), String> {
  let app = Router::new()
    .route("/v1/messages", post(handle_claude_messages))
    .route("/v1/models", get(handle_openai_passthrough))
    .route("/v1/chat/completions", post(handle_openai_passthrough))
    .route("/v1/responses", post(handle_openai_passthrough))
    .with_state(reqwest::Client::new());
  let addr: SocketAddr = LOCAL_PROXY_BASE_URL.trim_start_matches("http://").parse().map_err(|err| format!("invalid local proxy addr: {err}"))?;
  let listener = tokio::net::TcpListener::bind(addr).await.map_err(|err| format!("bind {LOCAL_PROXY_BASE_URL} failed: {err}"))?;
  axum::serve(listener, app).await.map_err(|err| err.to_string())
}

async fn handle_claude_messages(State(client): State<reqwest::Client>, Json(body): Json<Value>) -> Response {
  let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
  match proxy_claude_messages(client, body).await {
    Ok(value) if wants_stream => anthropic_sse_response(value),
    Ok(value) => (StatusCode::OK, Json(value)).into_response(),
    Err((status, message)) => (status, Json(json!({ "error": { "message": message } }))).into_response(),
  }
}

async fn handle_openai_passthrough(State(client): State<reqwest::Client>, method: Method, headers: HeaderMap, OriginalUri(uri): OriginalUri, body: Bytes) -> Response {
  match proxy_openai_request(client, method, headers, uri, body).await {
    Ok(response) => response,
    Err((status, message)) => (status, Json(json!({ "error": { "message": message } }))).into_response(),
  }
}

fn upstream_openai_url(base_url: &str, uri: &Uri) -> String {
  let path_and_query = uri.path_and_query().map(|item| item.as_str()).unwrap_or(uri.path());
  let upstream_path = path_and_query.strip_prefix("/v1").unwrap_or(path_and_query);
  format!("{}{}", base_url.trim_end_matches('/'), upstream_path)
}

async fn proxy_openai_request(client: reqwest::Client, method: Method, headers: HeaderMap, uri: Uri, body: Bytes) -> Result<Response, (StatusCode, String)> {
  let cfg = config_lock()
    .read()
    .await
    .clone()
    .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "local proxy is not configured".to_string()))?;
  let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes()).map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid method: {err}")))?;
  let mut request = client.request(reqwest_method, upstream_openai_url(&cfg.base_url, &uri)).bearer_auth(cfg.api_key);
  if let Some(content_type) = headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()) {
    request = request.header(header::CONTENT_TYPE, content_type);
  }
  if !body.is_empty() {
    request = request.body(body);
  }
  let upstream = request
    .send()
    .await
    .map_err(|err| (StatusCode::BAD_GATEWAY, format!("upstream request failed: {err}")))?;
  let status = StatusCode::from_u16(upstream.status().as_u16()).map_err(|err| (StatusCode::BAD_GATEWAY, format!("invalid upstream status: {err}")))?;
  let mut response_headers = HeaderMap::new();
  if let Some(content_type) = upstream.headers().get(header::CONTENT_TYPE).cloned() {
    response_headers.insert(header::CONTENT_TYPE, content_type);
  }
  let bytes = upstream.bytes().await.map_err(|err| (StatusCode::BAD_GATEWAY, format!("upstream body failed: {err}")))?;
  Ok((status, response_headers, bytes).into_response())
}
async fn proxy_claude_messages(client: reqwest::Client, body: Value) -> Result<Value, (StatusCode, String)> {
  let cfg = config_lock()
    .read()
    .await
    .clone()
    .ok_or_else(|| (StatusCode::SERVICE_UNAVAILABLE, "local proxy is not configured".to_string()))?;

  let requested_model = body.get("model").and_then(Value::as_str).unwrap_or(&cfg.default_model);
  let mapped_model = mapped_model_for_request(&cfg, requested_model);

  let messages = body.get("messages").and_then(Value::as_array).cloned().unwrap_or_default();
  let openai_messages = messages.into_iter().map(anthropic_message_to_openai).collect::<Vec<_>>();
  let max_tokens = body.get("max_tokens").cloned().unwrap_or_else(|| json!(4096));

  let openai_body = json!({
    "model": mapped_model,
    "messages": openai_messages,
    "max_tokens": max_tokens,
    "stream": false
  });

  let url = format!("{}/chat/completions", cfg.base_url.trim_end_matches('/'));
  let response = client
    .post(url)
    .bearer_auth(cfg.api_key)
    .json(&openai_body)
    .send()
    .await
    .map_err(|err| (StatusCode::BAD_GATEWAY, format!("upstream request failed: {err}")))?;
  let status = response.status();
  let upstream: Value = response
    .json()
    .await
    .map_err(|err| (StatusCode::BAD_GATEWAY, format!("upstream json failed: {err}")))?;
  if !status.is_success() {
    return Err((StatusCode::BAD_GATEWAY, upstream.to_string()));
  }
  Ok(openai_to_anthropic(upstream, requested_model))
}

fn mapped_model_for_request(cfg: &ProxyRouteConfig, requested_model: &str) -> String {
  if let Some(model) = cfg.model_map.get(requested_model).filter(|item| !item.trim().is_empty()) {
    return model.clone();
  }
  let lowered = requested_model.to_ascii_lowercase();
  for role in ["sonnet", "opus", "haiku"] {
    if lowered.contains(role) {
      if let Some(model) = cfg.model_map.get(role).filter(|item| !item.trim().is_empty()) {
        return model.clone();
      }
    }
  }
  cfg.default_model.clone()
}

fn anthropic_message_to_openai(message: Value) -> Value {
  let role = message.get("role").and_then(Value::as_str).unwrap_or("user");
  let content = match message.get("content") {
    Some(Value::String(text)) => text.clone(),
    Some(Value::Array(parts)) => parts
      .iter()
      .filter_map(|part| part.get("text").and_then(Value::as_str))
      .collect::<Vec<_>>()
      .join("\n"),
    _ => String::new(),
  };
  json!({ "role": role, "content": content })
}

fn openai_to_anthropic(upstream: Value, requested_model: &str) -> Value {
  let choice = upstream.get("choices").and_then(Value::as_array).and_then(|items| items.first()).cloned().unwrap_or_else(|| json!({}));
  let text = choice
    .get("message")
    .and_then(|message| message.get("content"))
    .and_then(Value::as_str)
    .unwrap_or("");
  json!({
    "id": upstream.get("id").cloned().unwrap_or_else(|| json!("msg_ai8888")),
    "type": "message",
    "role": "assistant",
    "model": requested_model,
    "content": [{ "type": "text", "text": text }],
    "stop_reason": "end_turn",
    "stop_sequence": null,
    "usage": upstream.get("usage").cloned().unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }))
  })
}

fn anthropic_sse_response(message: Value) -> Response {
  let id = message.get("id").and_then(Value::as_str).unwrap_or("msg_ai8888");
  let model = message.get("model").and_then(Value::as_str).unwrap_or("gpt-5.5");
  let text = message
    .get("content")
    .and_then(Value::as_array)
    .and_then(|items| items.first())
    .and_then(|item| item.get("text"))
    .and_then(Value::as_str)
    .unwrap_or("");
  let usage = message.get("usage").cloned().unwrap_or_else(|| json!({ "input_tokens": 0, "output_tokens": 0 }));
  let input_tokens = usage.get("prompt_tokens").or_else(|| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0);
  let output_tokens = usage.get("completion_tokens").or_else(|| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0);

  let events = [
    sse_event("message_start", json!({
      "type": "message_start",
      "message": {
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": [],
        "stop_reason": null,
        "stop_sequence": null,
        "usage": { "input_tokens": input_tokens, "output_tokens": 0 }
      }
    })),
    sse_event("content_block_start", json!({ "type": "content_block_start", "index": 0, "content_block": { "type": "text", "text": "" } })),
    sse_event("content_block_delta", json!({ "type": "content_block_delta", "index": 0, "delta": { "type": "text_delta", "text": text } })),
    sse_event("content_block_stop", json!({ "type": "content_block_stop", "index": 0 })),
    sse_event("message_delta", json!({ "type": "message_delta", "delta": { "stop_reason": "end_turn", "stop_sequence": null }, "usage": { "output_tokens": output_tokens } })),
    sse_event("message_stop", json!({ "type": "message_stop" })),
  ]
  .join("");

  let mut headers = HeaderMap::new();
  headers.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/event-stream; charset=utf-8"));
  headers.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
  (headers, events).into_response()
}

fn sse_event(event: &str, data: Value) -> String {
  format!("event: {event}\ndata: {data}\n\n")
}


#[cfg(test)]
mod tests {
  use super::*;

  fn test_config() -> ProxyRouteConfig {
    ProxyRouteConfig {
      base_url: "https://example.test/v1".to_string(),
      api_key: "sk-test".to_string(),
      default_model: "gpt-default".to_string(),
      model_map: HashMap::from([
        ("sonnet".to_string(), "gpt-sonnet".to_string()),
        ("opus".to_string(), "gpt-opus".to_string()),
        ("claude-3-haiku".to_string(), "gpt-haiku-exact".to_string()),
      ]),
    }
  }

  #[test]
  fn maps_exact_model_before_role_alias() {
    let cfg = test_config();
    assert_eq!(mapped_model_for_request(&cfg, "claude-3-haiku"), "gpt-haiku-exact");
  }

  #[test]
  fn maps_claude_role_model_names() {
    let cfg = test_config();
    assert_eq!(mapped_model_for_request(&cfg, "claude-sonnet-4-20250514"), "gpt-sonnet");
    assert_eq!(mapped_model_for_request(&cfg, "claude-opus-4-20250514"), "gpt-opus");
  }

  #[test]
  fn falls_back_to_default_model() {
    let cfg = test_config();
    assert_eq!(mapped_model_for_request(&cfg, "unknown-model"), "gpt-default");
  }

  #[test]
  fn openai_passthrough_preserves_path_and_query() {
    let uri: Uri = "/v1/responses?stream=true".parse().expect("uri");
    assert_eq!(upstream_openai_url("https://example.test/v1", &uri), "https://example.test/v1/responses?stream=true");
  }

  #[test]
  fn sse_response_contains_anthropic_events() {
    let response = anthropic_sse_response(json!({
      "id": "msg_test",
      "model": "claude-sonnet",
      "content": [{ "type": "text", "text": "hello" }],
      "usage": { "prompt_tokens": 3, "completion_tokens": 5 }
    }));
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response.headers().get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or_default();
    assert!(content_type.starts_with("text/event-stream"));
  }
}
