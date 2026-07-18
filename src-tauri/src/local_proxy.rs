use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::{Body, Bytes},
    extract::OriginalUri,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::RwLock;

use crate::config::{normalize_api_base_url, LOCAL_PROXY_BASE_URL};
use crate::error::AppError;
use crate::models::SwitchTarget;
use crate::tools::build_local_route_manifest;
use crate::usage::{record_usage, UsageRecord};
use crate::workspace::{load_proxy_settings, ProxySettings};

#[derive(Debug, Clone)]
struct ProxyRouteConfig {
    base_url: String,
    api_key: String,
    default_model: String,
    model_map: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct EndpointCandidate {
    id: String,
    name: String,
    base_url: String,
}

#[derive(Debug, Clone, Default)]
struct CircuitState {
    consecutive_failures: u32,
    open_until_ms: u64,
    last_error: Option<String>,
    last_latency_ms: Option<u64>,
    last_status_code: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProxyEndpointHealth {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub healthy: bool,
    pub status: Option<u16>,
    pub latency_ms: Option<u64>,
    pub circuit_state: String,
    pub consecutive_failures: u32,
    pub error: Option<String>,
}

struct UpstreamResponse {
    response: reqwest::Response,
    endpoint: EndpointCandidate,
}

static PROXY_CONFIG: OnceLock<RwLock<Option<ProxyRouteConfig>>> = OnceLock::new();
static PROXY_STARTED: OnceLock<()> = OnceLock::new();
static CIRCUITS: OnceLock<RwLock<HashMap<String, CircuitState>>> = OnceLock::new();

fn config_lock() -> &'static RwLock<Option<ProxyRouteConfig>> {
    PROXY_CONFIG.get_or_init(|| RwLock::new(None))
}

fn circuit_lock() -> &'static RwLock<HashMap<String, CircuitState>> {
    CIRCUITS.get_or_init(|| RwLock::new(HashMap::new()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

pub async fn ensure_local_proxy(target: &SwitchTarget) -> Result<(), AppError> {
    let manifest = build_local_route_manifest(target);
    if manifest.entries.is_empty() {
        return Ok(());
    }

    let cfg = ProxyRouteConfig {
        base_url: normalize_api_base_url(&target.base_url),
        api_key: target.api_key.clone(),
        default_model: target
            .model
            .clone()
            .unwrap_or_else(|| "gpt-5.5".to_string()),
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
        .route("/v1/responses", post(handle_openai_passthrough));
    let addr: SocketAddr = LOCAL_PROXY_BASE_URL
        .trim_start_matches("http://")
        .parse()
        .map_err(|err| format!("invalid local proxy addr: {err}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|err| format!("bind {LOCAL_PROXY_BASE_URL} failed: {err}"))?;
    axum::serve(listener, app)
        .await
        .map_err(|err| err.to_string())
}

async fn handle_claude_messages(Json(body): Json<Value>) -> Response {
    let wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    match proxy_claude_messages(body, wants_stream).await {
        Ok(response) => response,
        Err((status, message)) => (
            status,
            Json(json!({ "type": "error", "error": { "type": "api_error", "message": message } })),
        )
            .into_response(),
    }
}

async fn handle_openai_passthrough(
    method: Method,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    body: Bytes,
) -> Response {
    match proxy_openai_request(method, headers, uri, body).await {
        Ok(response) => response,
        Err((status, message)) => {
            (status, Json(json!({ "error": { "message": message } }))).into_response()
        }
    }
}

fn configured_client(settings: &ProxySettings) -> Result<reqwest::Client, (StatusCode, String)> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(
            settings.connect_timeout_ms.max(1_000),
        ))
        .timeout(Duration::from_millis(
            settings.request_timeout_ms.max(5_000),
        ))
        .build()
        .map_err(|err| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("build proxy client failed: {err}"),
            )
        })
}

fn endpoint_candidates(cfg: &ProxyRouteConfig, settings: &ProxySettings) -> Vec<EndpointCandidate> {
    let mut configured = settings
        .endpoints
        .iter()
        .filter(|item| item.enabled)
        .cloned()
        .collect::<Vec<_>>();
    configured.sort_by_key(|item| item.priority);
    let mut result = configured
        .into_iter()
        .map(|item| EndpointCandidate {
            id: item.id,
            name: if item.name.trim().is_empty() {
                "Configured endpoint".into()
            } else {
                item.name
            },
            base_url: normalize_api_base_url(&item.base_url),
        })
        .collect::<Vec<_>>();
    result.push(EndpointCandidate {
        id: "active-profile".into(),
        name: "Active profile".into(),
        base_url: cfg.base_url.clone(),
    });
    let mut seen = HashSet::new();
    result.retain(|item| seen.insert(item.base_url.clone()));
    if !settings.auto_failover {
        result.truncate(1);
    }
    result
}

async fn circuit_allows(base_url: &str) -> bool {
    circuit_lock()
        .read()
        .await
        .get(base_url)
        .map(|state| state.open_until_ms <= now_ms())
        .unwrap_or(true)
}

async fn mark_endpoint_success(base_url: &str, status_code: u16, latency_ms: u64) {
    circuit_lock().write().await.insert(
        base_url.to_string(),
        CircuitState {
            last_latency_ms: Some(latency_ms),
            last_status_code: Some(status_code),
            ..Default::default()
        },
    );
}

async fn mark_endpoint_failure(
    base_url: &str,
    settings: &ProxySettings,
    status_code: Option<u16>,
    latency_ms: u64,
    error: String,
) {
    let mut circuits = circuit_lock().write().await;
    let state = circuits.entry(base_url.to_string()).or_default();
    state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    state.last_error = Some(error.chars().take(300).collect());
    state.last_latency_ms = Some(latency_ms);
    state.last_status_code = status_code;
    if state.consecutive_failures >= settings.circuit_failure_threshold.max(1) {
        state.open_until_ms = now_ms().saturating_add(settings.circuit_open_seconds.max(5) * 1_000);
    }
}

fn retryable_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn next_candidate(candidates: &[EndpointCandidate], attempt: usize) -> EndpointCandidate {
    let mut available = Vec::new();
    for candidate in candidates {
        if circuit_allows(&candidate.base_url).await {
            available.push(candidate.clone());
        }
    }
    let pool = if available.is_empty() {
        candidates.to_vec()
    } else {
        available
    };
    pool[attempt % pool.len()].clone()
}

fn upstream_openai_url(base_url: &str, uri: &Uri) -> String {
    let path_and_query = uri
        .path_and_query()
        .map(|item| item.as_str())
        .unwrap_or(uri.path());
    let upstream_path = path_and_query.strip_prefix("/v1").unwrap_or(path_and_query);
    format!("{}{}", base_url.trim_end_matches('/'), upstream_path)
}

fn apply_forward_headers(
    mut request: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for name in [header::CONTENT_TYPE, header::ACCEPT] {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value);
        }
    }
    for name in ["openai-beta", "x-stainless-helper-method"] {
        if let Some(value) = headers.get(name) {
            request = request.header(name, value);
        }
    }
    request
}

async fn send_openai_with_failover(
    client: &reqwest::Client,
    cfg: &ProxyRouteConfig,
    settings: &ProxySettings,
    candidates: &[EndpointCandidate],
    method: &reqwest::Method,
    headers: &HeaderMap,
    uri: &Uri,
    body: &Bytes,
) -> Result<UpstreamResponse, (StatusCode, String)> {
    let attempts =
        (settings.max_retries.saturating_add(1).max(1) as usize).max(if settings.auto_failover {
            candidates.len()
        } else {
            1
        });
    let mut last_error = "no upstream endpoint available".to_string();
    for attempt in 0..attempts {
        let endpoint = next_candidate(candidates, attempt).await;
        let started = Instant::now();
        let mut request = client
            .request(method.clone(), upstream_openai_url(&endpoint.base_url, uri))
            .bearer_auth(&cfg.api_key);
        request = apply_forward_headers(request, headers);
        if !body.is_empty() {
            request = request.body(body.clone());
        }
        match request.send().await {
            Ok(response) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let status = response.status();
                if retryable_status(status) {
                    last_error = format!("upstream returned {status}");
                    mark_endpoint_failure(
                        &endpoint.base_url,
                        settings,
                        Some(status.as_u16()),
                        latency_ms,
                        last_error.clone(),
                    )
                    .await;
                    if attempt + 1 < attempts {
                        continue;
                    }
                } else {
                    mark_endpoint_success(&endpoint.base_url, status.as_u16(), latency_ms).await;
                }
                return Ok(UpstreamResponse { response, endpoint });
            }
            Err(error) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                last_error = error.to_string();
                mark_endpoint_failure(
                    &endpoint.base_url,
                    settings,
                    None,
                    latency_ms,
                    last_error.clone(),
                )
                .await;
            }
        }
    }
    Err((
        StatusCode::BAD_GATEWAY,
        format!("all upstream attempts failed: {last_error}"),
    ))
}

async fn send_anthropic_with_failover(
    client: &reqwest::Client,
    cfg: &ProxyRouteConfig,
    settings: &ProxySettings,
    candidates: &[EndpointCandidate],
    body: &Value,
) -> Result<UpstreamResponse, (StatusCode, String)> {
    let attempts =
        (settings.max_retries.saturating_add(1).max(1) as usize).max(if settings.auto_failover {
            candidates.len()
        } else {
            1
        });
    let mut last_error = "no upstream endpoint available".to_string();
    for attempt in 0..attempts {
        let endpoint = next_candidate(candidates, attempt).await;
        let started = Instant::now();
        let url = format!(
            "{}/chat/completions",
            endpoint.base_url.trim_end_matches('/')
        );
        match client
            .post(url)
            .bearer_auth(&cfg.api_key)
            .json(body)
            .send()
            .await
        {
            Ok(response) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let status = response.status();
                if retryable_status(status) {
                    last_error = format!("upstream returned {status}");
                    mark_endpoint_failure(
                        &endpoint.base_url,
                        settings,
                        Some(status.as_u16()),
                        latency_ms,
                        last_error.clone(),
                    )
                    .await;
                    if attempt + 1 < attempts {
                        continue;
                    }
                } else {
                    mark_endpoint_success(&endpoint.base_url, status.as_u16(), latency_ms).await;
                }
                return Ok(UpstreamResponse { response, endpoint });
            }
            Err(error) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                last_error = error.to_string();
                mark_endpoint_failure(
                    &endpoint.base_url,
                    settings,
                    None,
                    latency_ms,
                    last_error.clone(),
                )
                .await;
            }
        }
    }
    Err((
        StatusCode::BAD_GATEWAY,
        format!("all upstream attempts failed: {last_error}"),
    ))
}

fn response_headers(upstream: &reqwest::Response) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in [header::CONTENT_TYPE, header::CACHE_CONTROL] {
        if let Some(value) = upstream.headers().get(&name).cloned() {
            headers.insert(name, value);
        }
    }
    for name in ["x-request-id", "openai-processing-ms", "openai-version"] {
        if let Some(value) = upstream.headers().get(name).cloned() {
            if let (Ok(name), Ok(value)) = (
                name.parse::<axum::http::HeaderName>(),
                HeaderValue::from_bytes(value.as_bytes()),
            ) {
                headers.insert(name, value);
            }
        }
    }
    headers
}

fn request_model(body: &[u8], fallback: &str) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| fallback.to_string())
}

async fn proxy_openai_request(
    method: Method,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Result<Response, (StatusCode, String)> {
    let cfg = config_lock().read().await.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "local proxy is not configured".to_string(),
        )
    })?;
    let settings = load_proxy_settings();
    let candidates = endpoint_candidates(&cfg, &settings);
    if candidates.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no local proxy endpoint is enabled".into(),
        ));
    }
    let client = configured_client(&settings)?;
    let reqwest_method = reqwest::Method::from_bytes(method.as_str().as_bytes())
        .map_err(|err| (StatusCode::BAD_REQUEST, format!("invalid method: {err}")))?;
    let model = request_model(&body, &cfg.default_model);
    let started = Instant::now();
    let upstream = match send_openai_with_failover(
        &client,
        &cfg,
        &settings,
        &candidates,
        &reqwest_method,
        &headers,
        &uri,
        &body,
    )
    .await
    {
        Ok(upstream) => upstream,
        Err((status, message)) => {
            record_usage(UsageRecord::new(
                "openai",
                "unavailable",
                model,
                0,
                0,
                0,
                status.as_u16(),
                started.elapsed().as_millis() as u64,
                Some(message.clone()),
            ));
            return Err((status, message));
        }
    };
    let status = StatusCode::from_u16(upstream.response.status().as_u16()).map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("invalid upstream status: {err}"),
        )
    })?;
    let response_headers = response_headers(&upstream.response);
    let endpoint = upstream.endpoint.base_url.clone();
    let is_stream = upstream
        .response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.contains("text/event-stream"))
        .unwrap_or(false);

    if is_stream {
        let stream = passthrough_usage_stream(
            upstream.response.bytes_stream(),
            endpoint,
            model,
            status.as_u16(),
            started,
        );
        return Ok((status, response_headers, Body::from_stream(stream)).into_response());
    }

    let bytes = upstream.response.bytes().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream body failed: {err}"),
        )
    })?;
    let usage = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .map(|value| extract_openai_usage(&value))
        .unwrap_or_default();
    record_usage(UsageRecord::new(
        "openai",
        endpoint,
        model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_input_tokens,
        status.as_u16(),
        started.elapsed().as_millis() as u64,
        (status.as_u16() >= 400)
            .then(|| String::from_utf8_lossy(&bytes).chars().take(300).collect()),
    ));
    Ok((status, response_headers, bytes).into_response())
}

#[derive(Debug, Clone, Copy, Default)]
struct TokenUsage {
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
}

fn extract_openai_usage(value: &Value) -> TokenUsage {
    let usage = value.get("usage").unwrap_or(value);
    TokenUsage {
        input_tokens: usage
            .get("prompt_tokens")
            .or_else(|| usage.get("input_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: usage
            .get("completion_tokens")
            .or_else(|| usage.get("output_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        cached_input_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .or_else(|| {
                usage
                    .get("input_tokens_details")
                    .and_then(|details| details.get("cached_tokens"))
            })
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

fn take_sse_frames(buffer: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    loop {
        let separator = buffer
            .windows(2)
            .position(|window| window == b"\n\n")
            .map(|index| (index, 2))
            .or_else(|| {
                buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|index| (index, 4))
            });
        let Some((index, separator_len)) = separator else {
            break;
        };
        let frame = buffer.drain(..index).collect::<Vec<_>>();
        buffer.drain(..separator_len);
        frames.push(frame);
    }
    frames
}

fn sse_data(frame: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(frame);
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    (!data.is_empty()).then_some(data)
}

fn update_usage_from_sse(buffer: &mut Vec<u8>, chunk: &[u8], usage: &mut TokenUsage) {
    buffer.extend_from_slice(chunk);
    for frame in take_sse_frames(buffer) {
        let Some(data) = sse_data(&frame) else {
            continue;
        };
        if data == "[DONE]" {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&data) {
            let candidate = extract_openai_usage(&value);
            if candidate.input_tokens > 0
                || candidate.output_tokens > 0
                || candidate.cached_input_tokens > 0
            {
                *usage = candidate;
            }
        }
    }
    if buffer.len() > 1_048_576 {
        buffer.clear();
    }
}

fn passthrough_usage_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    endpoint: String,
    model: String,
    status_code: u16,
    started: Instant,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static {
    async_stream::stream! {
      let mut upstream = Box::pin(upstream);
      let mut usage = TokenUsage::default();
      let mut buffer = Vec::new();
      let mut stream_error = None;
      while let Some(item) = upstream.next().await {
        match item {
          Ok(bytes) => {
            update_usage_from_sse(&mut buffer, &bytes, &mut usage);
            yield Ok(bytes);
          }
          Err(error) => {
            stream_error = Some(error.to_string());
            yield Err(std::io::Error::new(std::io::ErrorKind::Other, error));
            break;
          }
        }
      }
      record_usage(UsageRecord::new(
        "openai",
        endpoint,
        model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_input_tokens,
        status_code,
        started.elapsed().as_millis() as u64,
        stream_error,
      ));
    }
}

async fn proxy_claude_messages(
    body: Value,
    wants_stream: bool,
) -> Result<Response, (StatusCode, String)> {
    let cfg = config_lock().read().await.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "local proxy is not configured".to_string(),
        )
    })?;
    let settings = load_proxy_settings();
    let candidates = endpoint_candidates(&cfg, &settings);
    if candidates.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "no local proxy endpoint is enabled".into(),
        ));
    }
    let client = configured_client(&settings)?;
    let requested_model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or(&cfg.default_model)
        .to_string();
    let mapped_model = mapped_model_for_request(&cfg, &requested_model);
    let openai_body = anthropic_request_to_openai(&body, &mapped_model, wants_stream);
    let started = Instant::now();
    let upstream =
        match send_anthropic_with_failover(&client, &cfg, &settings, &candidates, &openai_body)
            .await
        {
            Ok(upstream) => upstream,
            Err((status, message)) => {
                record_usage(UsageRecord::new(
                    "anthropic",
                    "unavailable",
                    mapped_model,
                    0,
                    0,
                    0,
                    status.as_u16(),
                    started.elapsed().as_millis() as u64,
                    Some(message.clone()),
                ));
                return Err((status, message));
            }
        };
    let upstream_status = upstream.response.status();
    let status = StatusCode::from_u16(upstream_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let endpoint = upstream.endpoint.base_url;

    if !upstream_status.is_success() {
        let text = upstream
            .response
            .text()
            .await
            .unwrap_or_else(|error| error.to_string());
        record_usage(UsageRecord::new(
            "anthropic",
            endpoint,
            mapped_model,
            0,
            0,
            0,
            status.as_u16(),
            started.elapsed().as_millis() as u64,
            Some(text.clone()),
        ));
        return Err((status, text));
    }

    if wants_stream {
        let stream = anthropic_stream(
            upstream.response.bytes_stream(),
            requested_model,
            mapped_model,
            endpoint,
            status.as_u16(),
            started,
        );
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream; charset=utf-8"),
        );
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return Ok((status, headers, Body::from_stream(stream)).into_response());
    }

    let upstream_value: Value = upstream.response.json().await.map_err(|err| {
        (
            StatusCode::BAD_GATEWAY,
            format!("upstream json failed: {err}"),
        )
    })?;
    let usage = extract_openai_usage(&upstream_value);
    let response = openai_to_anthropic(upstream_value, &requested_model);
    record_usage(UsageRecord::new(
        "anthropic",
        endpoint,
        mapped_model,
        usage.input_tokens,
        usage.output_tokens,
        usage.cached_input_tokens,
        status.as_u16(),
        started.elapsed().as_millis() as u64,
        None,
    ));
    Ok((status, Json(response)).into_response())
}

fn mapped_model_for_request(cfg: &ProxyRouteConfig, requested_model: &str) -> String {
    if let Some(model) = cfg
        .model_map
        .get(requested_model)
        .filter(|item| !item.trim().is_empty())
    {
        return model.clone();
    }
    let lowered = requested_model.to_ascii_lowercase();
    for role in ["sonnet", "opus", "haiku"] {
        if lowered.contains(role) {
            if let Some(model) = cfg
                .model_map
                .get(role)
                .filter(|item| !item.trim().is_empty())
            {
                return model.clone();
            }
        }
    }
    cfg.default_model.clone()
}

fn anthropic_request_to_openai(body: &Value, mapped_model: &str, wants_stream: bool) -> Value {
    let mut messages = Vec::new();
    if let Some(system) = body.get("system") {
        let content = anthropic_content_to_openai(system);
        if !content_is_empty(&content) {
            messages.push(json!({ "role": "system", "content": content }));
        }
    }
    if let Some(items) = body.get("messages").and_then(Value::as_array) {
        for message in items {
            messages.extend(anthropic_message_to_openai(message));
        }
    }

    let mut result = Map::new();
    result.insert("model".into(), json!(mapped_model));
    result.insert("messages".into(), Value::Array(messages));
    result.insert(
        "max_tokens".into(),
        body.get("max_tokens")
            .cloned()
            .unwrap_or_else(|| json!(4096)),
    );
    result.insert("stream".into(), json!(wants_stream));
    if wants_stream {
        result.insert("stream_options".into(), json!({ "include_usage": true }));
    }
    for name in ["temperature", "top_p"] {
        if let Some(value) = body.get(name) {
            result.insert(name.into(), value.clone());
        }
    }
    if let Some(stop) = body.get("stop_sequences") {
        result.insert("stop".into(), stop.clone());
    }
    if body
        .get("thinking")
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        == Some("enabled")
    {
        let budget = body
            .get("thinking")
            .and_then(|value| value.get("budget_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(4_096);
        result.insert(
            "reasoning_effort".into(),
            json!(if budget <= 2_048 {
                "low"
            } else if budget <= 10_000 {
                "medium"
            } else {
                "high"
            }),
        );
    }
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        result.insert(
            "tools".into(),
            Value::Array(tools.iter().filter_map(anthropic_tool_to_openai).collect()),
        );
    }
    if let Some(choice) = body.get("tool_choice") {
        if let Some(value) = anthropic_tool_choice_to_openai(choice) {
            result.insert("tool_choice".into(), value);
        }
        if choice
            .get("disable_parallel_tool_use")
            .and_then(Value::as_bool)
            == Some(true)
        {
            result.insert("parallel_tool_calls".into(), json!(false));
        }
    }
    Value::Object(result)
}

fn anthropic_tool_to_openai(tool: &Value) -> Option<Value> {
    let name = tool.get("name")?.as_str()?;
    let mut function = Map::new();
    function.insert("name".into(), json!(name));
    if let Some(description) = tool.get("description") {
        function.insert("description".into(), description.clone());
    }
    function.insert(
        "parameters".into(),
        tool.get("input_schema")
            .cloned()
            .unwrap_or_else(|| json!({ "type": "object", "properties": {} })),
    );
    Some(json!({ "type": "function", "function": function }))
}

fn anthropic_tool_choice_to_openai(choice: &Value) -> Option<Value> {
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => choice
            .get("name")
            .and_then(Value::as_str)
            .map(|name| json!({ "type": "function", "function": { "name": name } })),
        _ => None,
    }
}

fn content_is_empty(content: &Value) -> bool {
    match content {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        _ => false,
    }
}

fn anthropic_message_to_openai(message: &Value) -> Vec<Value> {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("user");
    let Some(parts) = message.get("content").and_then(Value::as_array) else {
        return vec![
            json!({ "role": role, "content": anthropic_content_to_openai(message.get("content").unwrap_or(&Value::Null)) }),
        ];
    };

    if role == "assistant" {
        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        for part in parts {
            if part.get("type").and_then(Value::as_str) == Some("tool_use") {
                let id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_call");
                let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                let input = part.get("input").cloned().unwrap_or_else(|| json!({}));
                let arguments = serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                tool_calls.push(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": arguments } }));
            } else if let Some(converted) = anthropic_part_to_openai(part) {
                content_parts.push(converted);
            }
        }
        let content = compact_openai_content(content_parts);
        let mut converted = json!({ "role": "assistant", "content": content });
        if !tool_calls.is_empty() {
            converted["tool_calls"] = Value::Array(tool_calls);
        }
        return vec![converted];
    }

    let mut result = Vec::new();
    let mut pending_content = Vec::new();
    for part in parts {
        if part.get("type").and_then(Value::as_str) == Some("tool_result") {
            if !pending_content.is_empty() {
                result.push(json!({ "role": role, "content": compact_openai_content(std::mem::take(&mut pending_content)) }));
            }
            let tool_call_id = part
                .get("tool_use_id")
                .and_then(Value::as_str)
                .unwrap_or("tool_call");
            result.push(json!({ "role": "tool", "tool_call_id": tool_call_id, "content": tool_result_content(part.get("content")) }));
        } else if let Some(converted) = anthropic_part_to_openai(part) {
            pending_content.push(converted);
        }
    }
    if !pending_content.is_empty() || result.is_empty() {
        result.push(json!({ "role": role, "content": compact_openai_content(pending_content) }));
    }
    result
}

fn tool_result_content(content: Option<&Value>) -> Value {
    match content {
        Some(Value::String(text)) => json!(text),
        Some(value) => anthropic_content_to_openai(value),
        None => json!(""),
    }
}

fn anthropic_content_to_openai(content: &Value) -> Value {
    match content {
        Value::String(text) => json!(text),
        Value::Array(parts) => {
            compact_openai_content(parts.iter().filter_map(anthropic_part_to_openai).collect())
        }
        _ => json!(""),
    }
}

fn compact_openai_content(parts: Vec<Value>) -> Value {
    if parts.len() == 1 && parts[0].get("type").and_then(Value::as_str) == Some("text") {
        return parts[0].get("text").cloned().unwrap_or_else(|| json!(""));
    }
    Value::Array(parts)
}

fn anthropic_part_to_openai(part: &Value) -> Option<Value> {
    match part.get("type").and_then(Value::as_str)? {
        "text" => Some(
            json!({ "type": "text", "text": part.get("text").and_then(Value::as_str).unwrap_or("") }),
        ),
        "image" => {
            let source = part.get("source")?;
            let url = match source.get("type").and_then(Value::as_str) {
                Some("base64") => format!(
                    "data:{};base64,{}",
                    source
                        .get("media_type")
                        .and_then(Value::as_str)
                        .unwrap_or("image/png"),
                    source.get("data").and_then(Value::as_str).unwrap_or("")
                ),
                Some("url") => source
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                _ => return None,
            };
            Some(json!({ "type": "image_url", "image_url": { "url": url } }))
        }
        _ => None,
    }
}

fn openai_to_anthropic(upstream: Value, requested_model: &str) -> Value {
    let choice = upstream
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let message = choice.get("message").cloned().unwrap_or_else(|| json!({}));
    let mut content = Vec::new();
    if let Some(thinking) = message
        .get("reasoning_content")
        .or_else(|| message.get("reasoning"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        content.push(json!({ "type": "thinking", "thinking": thinking }));
    }
    match message.get("content") {
        Some(Value::String(text)) if !text.is_empty() => {
            content.push(json!({ "type": "text", "text": text }))
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    content.push(json!({ "type": "text", "text": text }));
                }
            }
        }
        _ => {}
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for tool in tool_calls {
            let function = tool.get("function").cloned().unwrap_or_else(|| json!({}));
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            let input = serde_json::from_str::<Value>(arguments)
                .unwrap_or_else(|_| json!({ "_raw": arguments }));
            content.push(json!({
              "type": "tool_use",
              "id": tool.get("id").and_then(Value::as_str).unwrap_or("tool_call"),
              "name": function.get("name").and_then(Value::as_str).unwrap_or("tool"),
              "input": input
            }));
        }
    }
    let finish_reason = choice.get("finish_reason").and_then(Value::as_str);
    let usage = extract_openai_usage(&upstream);
    json!({
      "id": upstream.get("id").cloned().unwrap_or_else(|| json!("msg_ai8888")),
      "type": "message",
      "role": "assistant",
      "model": requested_model,
      "content": content,
      "stop_reason": anthropic_stop_reason(finish_reason),
      "stop_sequence": null,
      "usage": {
        "input_tokens": usage.input_tokens,
        "output_tokens": usage.output_tokens,
        "cache_read_input_tokens": usage.cached_input_tokens
      }
    })
}

fn anthropic_stop_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("tool_calls") | Some("function_call") => "tool_use",
        Some("length") => "max_tokens",
        Some("stop") | None => "end_turn",
        _ => "end_turn",
    }
}

#[derive(Default)]
struct ToolStreamBlock {
    content_index: usize,
    id: String,
    name: String,
}

struct AnthropicStreamState {
    started: bool,
    message_id: String,
    requested_model: String,
    next_content_index: usize,
    thinking_content_index: Option<usize>,
    text_content_index: Option<usize>,
    tool_blocks: HashMap<u64, ToolStreamBlock>,
    stop_reason: Option<String>,
    usage: TokenUsage,
}

impl AnthropicStreamState {
    fn new(requested_model: String) -> Self {
        Self {
            started: false,
            message_id: "msg_ai8888".into(),
            requested_model,
            next_content_index: 0,
            thinking_content_index: None,
            text_content_index: None,
            tool_blocks: HashMap::new(),
            stop_reason: None,
            usage: TokenUsage::default(),
        }
    }

    fn ensure_started(&mut self, value: Option<&Value>) -> Vec<String> {
        if self.started {
            return Vec::new();
        }
        if let Some(id) = value
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
        {
            self.message_id = id.to_string();
        }
        self.started = true;
        vec![sse_event(
            "message_start",
            json!({
              "type": "message_start",
              "message": {
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.requested_model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": { "input_tokens": self.usage.input_tokens, "output_tokens": 0 }
              }
            }),
        )]
    }

    fn process_chunk(&mut self, value: &Value) -> Vec<String> {
        let usage = extract_openai_usage(value);
        if usage.input_tokens > 0 || usage.output_tokens > 0 || usage.cached_input_tokens > 0 {
            self.usage = usage;
        }
        let mut events = self.ensure_started(Some(value));
        let Some(choice) = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
        else {
            return events;
        };
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop_reason = Some(anthropic_stop_reason(Some(reason)).to_string());
        }
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        if let Some(thinking) = delta
            .get("reasoning_content")
            .or_else(|| delta.get("reasoning"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let index = match self.thinking_content_index {
                Some(index) => index,
                None => {
                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    self.thinking_content_index = Some(index);
                    events.push(sse_event("content_block_start", json!({ "type": "content_block_start", "index": index, "content_block": { "type": "thinking", "thinking": "" } })));
                    index
                }
            };
            events.push(sse_event("content_block_delta", json!({ "type": "content_block_delta", "index": index, "delta": { "type": "thinking_delta", "thinking": thinking } })));
        }
        if let Some(text) = delta
            .get("content")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let index = match self.text_content_index {
                Some(index) => index,
                None => {
                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    self.text_content_index = Some(index);
                    events.push(sse_event("content_block_start", json!({ "type": "content_block_start", "index": index, "content_block": { "type": "text", "text": "" } })));
                    index
                }
            };
            events.push(sse_event("content_block_delta", json!({ "type": "content_block_delta", "index": index, "delta": { "type": "text_delta", "text": text } })));
        }
        if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for tool_call in tool_calls {
                let openai_index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let function = tool_call.get("function").cloned().unwrap_or(Value::Null);
                if !self.tool_blocks.contains_key(&openai_index) {
                    let index = self.next_content_index;
                    self.next_content_index += 1;
                    let id = tool_call
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("tool_call")
                        .to_string();
                    let name = function
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    self.tool_blocks.insert(
                        openai_index,
                        ToolStreamBlock {
                            content_index: index,
                            id: id.clone(),
                            name: name.clone(),
                        },
                    );
                    events.push(sse_event("content_block_start", json!({ "type": "content_block_start", "index": index, "content_block": { "type": "tool_use", "id": id, "name": name, "input": {} } })));
                }
                let block = self
                    .tool_blocks
                    .get_mut(&openai_index)
                    .expect("tool block inserted");
                if let Some(id) = tool_call
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    block.id = id.to_string();
                }
                if let Some(name) = function
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    block.name = name.to_string();
                }
                if let Some(arguments) = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    events.push(sse_event("content_block_delta", json!({ "type": "content_block_delta", "index": block.content_index, "delta": { "type": "input_json_delta", "partial_json": arguments } })));
                }
            }
        }
        events
    }

    fn finish(&mut self) -> Vec<String> {
        let mut events = self.ensure_started(None);
        let mut indices = self
            .tool_blocks
            .values()
            .map(|block| block.content_index)
            .collect::<Vec<_>>();
        if let Some(index) = self.text_content_index {
            indices.push(index);
        }
        if let Some(index) = self.thinking_content_index {
            indices.push(index);
        }
        indices.sort_unstable();
        indices.dedup();
        for index in indices {
            events.push(sse_event(
                "content_block_stop",
                json!({ "type": "content_block_stop", "index": index }),
            ));
        }
        events.push(sse_event("message_delta", json!({
      "type": "message_delta",
      "delta": { "stop_reason": self.stop_reason.as_deref().unwrap_or("end_turn"), "stop_sequence": null },
      "usage": { "output_tokens": self.usage.output_tokens }
    })));
        events.push(sse_event("message_stop", json!({ "type": "message_stop" })));
        events
    }
}

fn anthropic_stream(
    upstream: impl Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
    requested_model: String,
    mapped_model: String,
    endpoint: String,
    status_code: u16,
    started: Instant,
) -> impl Stream<Item = Result<Bytes, Infallible>> + Send + 'static {
    async_stream::stream! {
      let mut upstream = Box::pin(upstream);
      let mut state = AnthropicStreamState::new(requested_model);
      let mut buffer = Vec::new();
      let mut stream_error = None;
      let mut finished = false;
      let mut failed = false;
      while let Some(item) = upstream.next().await {
        match item {
          Ok(bytes) => {
            buffer.extend_from_slice(&bytes);
            for frame in take_sse_frames(&mut buffer) {
              let Some(data) = sse_data(&frame) else { continue };
              if data == "[DONE]" {
                for event in state.finish() { yield Ok(Bytes::from(event)); }
                finished = true;
                continue;
              }
              match serde_json::from_str::<Value>(&data) {
                Ok(value) => {
                  for event in state.process_chunk(&value) { yield Ok(Bytes::from(event)); }
                }
                Err(error) => {
                  stream_error = Some(format!("invalid upstream SSE data: {error}"));
                  yield Ok(Bytes::from(sse_event("error", json!({ "type": "error", "error": { "type": "api_error", "message": error.to_string() } }))));
                }
              }
            }
          }
          Err(error) => {
            stream_error = Some(error.to_string());
            failed = true;
            yield Ok(Bytes::from(sse_event("error", json!({ "type": "error", "error": { "type": "api_error", "message": error.to_string() } }))));
            break;
          }
        }
      }
      if !finished && !failed {
        for event in state.finish() { yield Ok(Bytes::from(event)); }
      }
      record_usage(UsageRecord::new(
        "anthropic",
        endpoint,
        mapped_model,
        state.usage.input_tokens,
        state.usage.output_tokens,
        state.usage.cached_input_tokens,
        status_code,
        started.elapsed().as_millis() as u64,
        stream_error,
      ));
    }
}

fn sse_event(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

#[tauri::command]
pub async fn app_probe_proxy_endpoints() -> Result<Vec<ProxyEndpointHealth>, String> {
    let settings = load_proxy_settings();
    let cfg = config_lock().read().await.clone();
    let fallback_cfg = ProxyRouteConfig {
        base_url: String::new(),
        api_key: String::new(),
        default_model: String::new(),
        model_map: HashMap::new(),
    };
    let cfg_ref = cfg.as_ref().unwrap_or(&fallback_cfg);
    let mut candidates = if cfg.is_some() {
        endpoint_candidates(cfg_ref, &settings)
    } else {
        let mut endpoints = settings
            .endpoints
            .iter()
            .filter(|item| item.enabled)
            .cloned()
            .collect::<Vec<_>>();
        endpoints.sort_by_key(|item| item.priority);
        endpoints
            .into_iter()
            .map(|item| EndpointCandidate {
                id: item.id,
                name: item.name,
                base_url: normalize_api_base_url(&item.base_url),
            })
            .collect()
    };
    let mut seen = HashSet::new();
    candidates.retain(|item| seen.insert(item.base_url.clone()));
    let client = configured_client(&settings).map_err(|(_, error)| error)?;
    let mut results = Vec::new();
    for endpoint in candidates {
        let started = Instant::now();
        let mut request = client.get(format!(
            "{}/models",
            endpoint.base_url.trim_end_matches('/')
        ));
        if !cfg_ref.api_key.is_empty() {
            request = request.bearer_auth(&cfg_ref.api_key);
        }
        let (healthy, status_code, error) = match request.send().await {
            Ok(response) => {
                let status = response.status();
                (
                    status.is_success(),
                    Some(status.as_u16()),
                    (!status.is_success()).then(|| format!("HTTP {status}")),
                )
            }
            Err(error) => (false, None, Some(error.to_string())),
        };
        let latency_ms = started.elapsed().as_millis() as u64;
        if healthy {
            mark_endpoint_success(&endpoint.base_url, status_code.unwrap_or(200), latency_ms).await;
        } else if status_code
            .map(|code| code == 408 || code == 429 || code >= 500)
            .unwrap_or(true)
        {
            mark_endpoint_failure(
                &endpoint.base_url,
                &settings,
                status_code,
                latency_ms,
                error.clone().unwrap_or_else(|| "probe failed".into()),
            )
            .await;
        } else {
            circuit_lock().write().await.insert(
                endpoint.base_url.clone(),
                CircuitState {
                    last_error: error.clone(),
                    last_latency_ms: Some(latency_ms),
                    last_status_code: status_code,
                    ..Default::default()
                },
            );
        }
        let circuit = circuit_lock()
            .read()
            .await
            .get(&endpoint.base_url)
            .cloned()
            .unwrap_or_default();
        results.push(ProxyEndpointHealth {
            id: endpoint.id,
            name: endpoint.name,
            base_url: endpoint.base_url,
            healthy,
            status: status_code,
            latency_ms: Some(latency_ms),
            circuit_state: if circuit.open_until_ms > now_ms() {
                "open".into()
            } else if circuit.consecutive_failures > 0 {
                "degraded".into()
            } else {
                "closed".into()
            },
            consecutive_failures: circuit.consecutive_failures,
            error: error.or(circuit.last_error),
        });
    }
    Ok(results)
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
        assert_eq!(
            mapped_model_for_request(&cfg, "claude-3-haiku"),
            "gpt-haiku-exact"
        );
        assert_eq!(
            mapped_model_for_request(&cfg, "claude-sonnet-4-20250514"),
            "gpt-sonnet"
        );
        assert_eq!(
            mapped_model_for_request(&cfg, "unknown-model"),
            "gpt-default"
        );
    }

    #[test]
    fn openai_passthrough_preserves_path_and_query() {
        let uri: Uri = "/v1/responses?stream=true".parse().expect("uri");
        assert_eq!(
            upstream_openai_url("https://example.test/v1", &uri),
            "https://example.test/v1/responses?stream=true"
        );
    }

    #[test]
    fn converts_anthropic_tools_images_and_results() {
        let request = anthropic_request_to_openai(
            &json!({
              "system": "system text",
              "messages": [
                { "role": "user", "content": [{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "abc" } }] },
                { "role": "assistant", "content": [{ "type": "tool_use", "id": "call_1", "name": "read", "input": { "path": "a" } }] },
                { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "call_1", "content": "ok" }] }
              ],
              "tools": [{ "name": "read", "description": "Read", "input_schema": { "type": "object" } }],
              "tool_choice": { "type": "tool", "name": "read" },
              "max_tokens": 100
            }),
            "gpt-test",
            false,
        );
        assert_eq!(request["messages"][0]["role"], "system");
        assert_eq!(request["messages"][1]["content"][0]["type"], "image_url");
        assert_eq!(
            request["messages"][2]["tool_calls"][0]["function"]["name"],
            "read"
        );
        assert_eq!(request["messages"][3]["role"], "tool");
        assert_eq!(request["tool_choice"]["function"]["name"], "read");
    }

    #[test]
    fn converts_openai_tool_calls_to_anthropic() {
        let response = openai_to_anthropic(
            json!({
              "id": "msg_1",
              "choices": [{ "message": { "content": "thinking", "tool_calls": [{ "id": "call_1", "function": { "name": "read", "arguments": "{\"path\":\"a\"}" } }] }, "finish_reason": "tool_calls" }],
              "usage": { "prompt_tokens": 10, "completion_tokens": 4 }
            }),
            "claude-sonnet",
        );
        assert_eq!(response["content"][1]["type"], "tool_use");
        assert_eq!(response["stop_reason"], "tool_use");
        assert_eq!(response["usage"]["input_tokens"], 10);
    }

    #[test]
    fn converts_streaming_text_and_tool_events() {
        let mut state = AnthropicStreamState::new("claude-sonnet".into());
        let first = state.process_chunk(&json!({ "id": "msg_1", "choices": [{ "delta": { "content": "hello" }, "finish_reason": null }] })).join("");
        let tool = state.process_chunk(&json!({ "choices": [{ "delta": { "tool_calls": [{ "index": 0, "id": "call_1", "function": { "name": "read", "arguments": "{\"path\":" } }] }, "finish_reason": null }] })).join("");
        assert!(first.contains("message_start"));
        assert!(first.contains("text_delta"));
        assert!(tool.contains("tool_use"));
        assert!(tool.contains("input_json_delta"));
    }
}
