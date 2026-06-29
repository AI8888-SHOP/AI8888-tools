use crate::config::{API_BASE_URL, SITE_BASE_URL};
use crate::error::AppError;
use crate::models::{AccountSummary, ApiKeySummary, AuthSession, EndpointProbeResult, EndpointProbeSummary, GroupSummary, LoginResult, ModelSummary, Pagination, SubscriptionProgressInfo, SubscriptionSummary};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const ENDPOINT_PROBE_DOMAINS: [&str; 3] = ["ai8888.shop", "www.ai8888.shop", "sub.ai8888.shop"];
const ENDPOINT_PROBE_ATTEMPTS: u32 = 3;

#[derive(Clone)]
pub struct ApiClient {
  client: reqwest::Client,
  base_url: Arc<RwLock<String>>,
  site_url: Arc<RwLock<String>>,
  selected_endpoint: Arc<RwLock<Option<EndpointProbeSummary>>>,
  endpoint_probe_lock: Arc<tokio::sync::Mutex<()>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPayload {
  pub email: String,
  pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshPayload {
  pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CreateKeyPayload {
  pub name: String,
  #[serde(alias = "groupId", rename = "group_id", skip_serializing_if = "Option::is_none")]
  pub group_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub custom_key: Option<String>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub ip_whitelist: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub ip_blacklist: Option<Vec<String>>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub quota: Option<f64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub expires_in_days: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rate_limit_5h: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rate_limit_1d: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub rate_limit_7d: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateKeyPayload {
  #[serde(skip_serializing_if = "Option::is_none")]
  pub name: Option<String>,
  #[serde(alias = "groupId", rename = "group_id", skip_serializing_if = "Option::is_none")]
  pub group_id: Option<u64>,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelsQuery {
  #[serde(default)]
  pub base_url: String,
  #[serde(default)]
  pub api_key: String,
  #[serde(default)]
  pub is_full_url: bool,
  #[serde(default)]
  pub models_url_override: Option<String>,
  #[serde(default)]
  pub user_agent: Option<String>,
}

impl ApiClient {
  pub fn new() -> Result<Self, AppError> {
    Ok(Self {
      client: reqwest::Client::builder().build()?,
      base_url: Arc::new(RwLock::new(API_BASE_URL.to_string())),
      site_url: Arc::new(RwLock::new(SITE_BASE_URL.to_string())),
      selected_endpoint: Arc::new(RwLock::new(None)),
      endpoint_probe_lock: Arc::new(tokio::sync::Mutex::new(())),
    })
  }

  fn url(&self, path: &str) -> String {
    let base_url = self.api_base_url();
    format!("{}{}", base_url.trim_end_matches('/'), path)
  }

  pub fn api_base_url(&self) -> String {
    self.base_url.read().map(|value| value.clone()).unwrap_or_else(|_| API_BASE_URL.to_string())
  }

  pub fn site_base_url(&self) -> String {
    self.site_url.read().map(|value| value.clone()).unwrap_or_else(|_| SITE_BASE_URL.to_string())
  }

  pub fn login_url(&self) -> String {
    format!("{}/login", self.site_base_url().trim_end_matches('/'))
  }

  fn apply_site_base_url(&self, site_base_url: &str) {
    let site_base_url = site_base_url.trim().trim_end_matches('/').to_string();
    if site_base_url.is_empty() {
      return;
    }
    if let Ok(mut site) = self.site_url.write() {
      *site = site_base_url.clone();
    }
    if let Ok(mut api) = self.base_url.write() {
      *api = format!("{site_base_url}/api/v1");
    }
  }

  fn current_endpoint_summary(&self) -> EndpointProbeSummary {
    let selected_base_url = self.site_base_url();
    let selected_domain = selected_base_url.trim_start_matches("https://").trim_start_matches("http://").to_string();
    EndpointProbeSummary {
      selected_base_url: selected_base_url.clone(),
      selected_domain: selected_domain.clone(),
      results: vec![EndpointProbeResult {
        domain: selected_domain,
        base_url: selected_base_url,
        attempts: 0,
        success_count: 0,
        packet_loss: 0.0,
        average_latency_ms: None,
        best_latency_ms: None,
        selected: true,
        error: None,
      }],
    }
  }

  pub async fn ensure_best_endpoint(&self) -> Result<EndpointProbeSummary, AppError> {
    if let Ok(guard) = self.selected_endpoint.read() {
      if let Some(summary) = guard.clone() {
        return Ok(summary);
      }
    }

    let _guard = self.endpoint_probe_lock.lock().await;
    if let Ok(guard) = self.selected_endpoint.read() {
      if let Some(summary) = guard.clone() {
        return Ok(summary);
      }
    }

    match self.probe_best_endpoint().await {
      Ok(summary) => {
        self.apply_endpoint_summary(summary.clone());
        Ok(summary)
      }
      Err(error) => {
        let fallback = self.current_endpoint_summary();
        if let Ok(mut selected) = self.selected_endpoint.write() {
          *selected = Some(fallback.clone());
        }
        Err(error)
      }
    }
  }

  pub async fn select_best_endpoint(&self) -> Result<EndpointProbeSummary, AppError> {
    let _guard = self.endpoint_probe_lock.lock().await;
    let summary = self.probe_best_endpoint().await?;
    self.apply_endpoint_summary(summary.clone());
    Ok(summary)
  }

  fn apply_endpoint_summary(&self, summary: EndpointProbeSummary) {
    self.apply_site_base_url(&summary.selected_base_url);
    if let Ok(mut selected) = self.selected_endpoint.write() {
      *selected = Some(summary);
    }
  }

  fn auth_headers(token: &str) -> Result<HeaderMap, AppError> {
    let mut headers = HeaderMap::new();
    headers.insert(
      AUTHORIZATION,
      HeaderValue::from_str(&format!("Bearer {token}")).map_err(|err| AppError::Message(err.to_string()))?,
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(headers)
  }

  async fn send_value(&self, request: reqwest::RequestBuilder) -> Result<Value, AppError> {
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;

    if !status.is_success() {
      let detail = if text.trim().is_empty() { status.to_string() } else { text.chars().take(700).collect::<String>() };
      return Err(AppError::Message(format!("AI8888 API {status}: {detail}")));
    }

    if text.trim().is_empty() {
      Ok(Value::Null)
    } else {
      serde_json::from_str(&text).map_err(|err| AppError::Message(format!("invalid API JSON: {err}; body={}", text.chars().take(300).collect::<String>())))
    }
  }

  fn data_or_self(value: Value) -> Value {
    value.get("data").cloned().unwrap_or(value)
  }

  fn decode_data<T: DeserializeOwned>(value: Value) -> Result<T, AppError> {
    serde_json::from_value(Self::data_or_self(value)).map_err(|err| AppError::Message(err.to_string()))
  }

  fn decode_page<T: DeserializeOwned + Default>(value: Value) -> Result<Pagination<T>, AppError> {
    let data = Self::data_or_self(value);
    if data.is_array() {
      let items: Vec<T> = serde_json::from_value(data).map_err(|err| AppError::Message(err.to_string()))?;
      return Ok(Pagination { total: items.len() as u64, items, ..Default::default() });
    }
    let mut page: Pagination<T> = serde_json::from_value(data.clone()).map_err(|err| AppError::Message(err.to_string()))?;
    if page.items.is_empty() {
      for key in ["items", "data", "records", "list"] {
        if let Some(items) = data.get(key).cloned() {
          page.items = serde_json::from_value(items).map_err(|err| AppError::Message(err.to_string()))?;
          break;
        }
      }
    }
    if page.total == 0 {
      page.total = page.items.len() as u64;
    }
    Ok(page)
  }

  pub async fn login(&self, payload: &LoginPayload) -> Result<LoginResult, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.post(self.url("/auth/login")).json(payload)).await?;
    let data = Self::data_or_self(value);
    let response: LoginResponseData = serde_json::from_value(data).map_err(|err| AppError::Message(err.to_string()))?;
    let access_token = response.access_token.or(response.token).ok_or_else(|| AppError::Message("登录响应缺少 access token".into()))?;
    let account = response.user.or(response.account).unwrap_or_default();
    Ok(LoginResult {
      session: AuthSession { access_token, refresh_token: response.refresh_token.unwrap_or_default(), expires_in: response.expires_in.unwrap_or(0) },
      account,
    })
  }

  pub async fn refresh(&self, payload: &RefreshPayload) -> Result<AuthSession, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.post(self.url("/auth/refresh")).json(payload)).await?;
    let response: RefreshResponseData = Self::decode_data(value)?;
    Ok(AuthSession {
      access_token: response.access_token.or(response.token).unwrap_or_default(),
      refresh_token: response.refresh_token.unwrap_or_else(|| payload.refresh_token.clone()),
      expires_in: response.expires_in.unwrap_or(0),
    })
  }

  pub async fn get_account(&self, token: &str) -> Result<AccountSummary, AppError> {
    let _ = self.ensure_best_endpoint().await;
    for path in ["/auth/me", "/user/profile", "/profile"] {
      let result = self.send_value(self.client.get(self.url(path)).headers(Self::auth_headers(token)?)).await.and_then(Self::decode_data::<AccountSummary>);
      if result.is_ok() {
        return result;
      }
    }
    Err(AppError::Message("无法获取账号信息".into()))
  }

  pub async fn get_profile(&self, token: &str) -> Result<AccountSummary, AppError> {
    self.get_account(token).await
  }

  pub async fn get_subscriptions(&self, token: &str) -> Result<Vec<SubscriptionSummary>, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.get(self.url("/subscriptions")).headers(Self::auth_headers(token)?)).await?;
    let data = Self::data_or_self(value);
    if data.is_array() {
      serde_json::from_value(data).map_err(|err| AppError::Message(err.to_string()))
    } else if let Some(items) = data.get("items").or_else(|| data.get("data")).cloned() {
      serde_json::from_value(items).map_err(|err| AppError::Message(err.to_string()))
    } else {
      Ok(Vec::new())
    }
  }

  pub async fn get_subscription_progress(&self, token: &str) -> Result<Vec<SubscriptionProgressInfo>, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.get(self.url("/subscriptions/progress")).headers(Self::auth_headers(token)?)).await?;
    let data = Self::data_or_self(value);
    if data.is_array() {
      serde_json::from_value(data).map_err(|err| AppError::Message(err.to_string()))
    } else if let Some(items) = data.get("items").or_else(|| data.get("data")).cloned() {
      serde_json::from_value(items).map_err(|err| AppError::Message(err.to_string()))
    } else {
      Ok(Vec::new())
    }
  }

  pub async fn get_groups(&self, token: &str) -> Result<Vec<GroupSummary>, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let mut last_error: Option<String> = None;
    for path in ["/groups/available", "/groups", "/user/groups", "/key-groups", "/api-key-groups", "/subscriptions/groups"] {
      match self.send_value(self.client.get(self.url(path)).headers(Self::auth_headers(token)?)).await {
        Ok(value) => {
          let groups = decode_groups_value(value)?;
          if !groups.is_empty() {
            return Ok(groups);
          }
        }
        Err(err) => last_error = Some(err.to_string()),
      }
    }
    if let Some(err) = last_error {
      Err(AppError::Message(err))
    } else {
      Ok(Vec::new())
    }
  }

  pub async fn get_keys(&self, token: &str) -> Result<Pagination<ApiKeySummary>, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.get(self.url("/keys")).headers(Self::auth_headers(token)?)).await?;
    Self::decode_page(value)
  }

  pub async fn create_key(&self, token: &str, payload: &CreateKeyPayload) -> Result<ApiKeySummary, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.post(self.url("/keys")).headers(Self::auth_headers(token)?).json(payload)).await?;
    Self::decode_data(value)
  }

  pub async fn update_key(&self, token: &str, key_id: u64, payload: &UpdateKeyPayload) -> Result<ApiKeySummary, AppError> {
    let _ = self.ensure_best_endpoint().await;
    let value = self.send_value(self.client.put(self.url(&format!("/keys/{key_id}"))).headers(Self::auth_headers(token)?).json(payload)).await?;
    Self::decode_data(value)
  }

  pub async fn delete_key(&self, token: &str, key_id: u64) -> Result<(), AppError> {
    let _ = self.ensure_best_endpoint().await;
    self.send_value(self.client.delete(self.url(&format!("/keys/{key_id}"))).headers(Self::auth_headers(token)?)).await?;
    Ok(())
  }

  pub async fn fetch_models(&self, query: &ModelsQuery) -> Result<Vec<ModelSummary>, AppError> {
    if query.api_key.trim().is_empty() {
      return Err(AppError::Message("请先填写 API Key".into()));
    }

    let candidates = model_url_candidates(&query.base_url, query.is_full_url, query.models_url_override.as_deref())?;
    let mut last_error = String::new();

    for url in candidates {
      let mut request = self
        .client
        .get(&url)
        .header(AUTHORIZATION, format!("Bearer {}", query.api_key.trim()));
      if let Some(ua) = &query.user_agent {
        request = request.header(USER_AGENT, ua);
      }

      let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
          last_error = err.to_string();
          continue;
        }
      };

      let status = response.status();
      let text = response.text().await?;
      if !status.is_success() {
        last_error = format!("{url} 返回 {status}: {}", text.chars().take(300).collect::<String>());
        continue;
      }

      let value: Value = serde_json::from_str(&text).map_err(|err| AppError::Message(format!("模型列表 JSON 解析失败: {err}")))?;
      let data = Self::data_or_self(value);
      let items = if data.is_array() {
        data
      } else if let Some(items) = data.get("items").or_else(|| data.get("data")).cloned() {
        items
      } else {
        Value::Array(Vec::new())
      };
      let mut models: Vec<ModelSummary> = serde_json::from_value(items).map_err(|err| AppError::Message(err.to_string()))?;
      models.sort_by(|left, right| left.id.cmp(&right.id));
      return Ok(models);
    }

    Err(AppError::Message(format!("获取模型失败: {last_error}")))
  }

  pub async fn probe_best_endpoint(&self) -> Result<EndpointProbeSummary, AppError> {
    let probe_client = reqwest::Client::builder()
      .connect_timeout(Duration::from_secs(3))
      .timeout(Duration::from_secs(5))
      .build()?;
    let mut results = Vec::new();

    for domain in ENDPOINT_PROBE_DOMAINS {
      results.push(probe_endpoint(&probe_client, domain).await);
    }

    let selected_index = results
      .iter()
      .enumerate()
      .min_by(|(_, left), (_, right)| compare_endpoint_probe(left, right))
      .map(|(index, _)| index)
      .ok_or_else(|| AppError::Message("没有可检测的端点".into()))?;

    if results[selected_index].success_count == 0 {
      return Err(AppError::Message("端点检测失败：所有端点均不可达".into()));
    }

    results[selected_index].selected = true;
    let selected_base_url = results[selected_index].base_url.clone();
    let selected_domain = results[selected_index].domain.clone();
    Ok(EndpointProbeSummary { selected_base_url, selected_domain, results })
  }
}

async fn probe_endpoint(client: &reqwest::Client, domain: &str) -> EndpointProbeResult {
  let base_url = format!("https://{domain}");
  let probe_url = format!("{base_url}/v1/models");
  let mut latencies = Vec::new();
  let mut errors = Vec::new();

  for _ in 0..ENDPOINT_PROBE_ATTEMPTS {
    let start = Instant::now();
    match client.get(&probe_url).send().await {
      Ok(response) => {
        let elapsed = start.elapsed().as_secs_f64() * 1000.0;
        if response.status().as_u16() < 500 {
          latencies.push(elapsed);
        } else {
          errors.push(format!("HTTP {}", response.status()));
        }
      }
      Err(err) => errors.push(err.to_string()),
    }
  }

  let success_count = latencies.len() as u32;
  let packet_loss = 1.0 - (success_count as f64 / ENDPOINT_PROBE_ATTEMPTS as f64);
  let average_latency_ms = if latencies.is_empty() { None } else { Some(latencies.iter().sum::<f64>() / latencies.len() as f64) };
  let best_latency_ms = latencies.iter().copied().reduce(f64::min);
  let error = if errors.is_empty() { None } else { Some(errors.join("; ")) };

  EndpointProbeResult {
    domain: domain.to_string(),
    base_url,
    attempts: ENDPOINT_PROBE_ATTEMPTS,
    success_count,
    packet_loss,
    average_latency_ms,
    best_latency_ms,
    selected: false,
    error,
  }
}

fn compare_endpoint_probe(left: &EndpointProbeResult, right: &EndpointProbeResult) -> std::cmp::Ordering {
  left
    .packet_loss
    .total_cmp(&right.packet_loss)
    .then_with(|| left.average_latency_ms.unwrap_or(f64::MAX).total_cmp(&right.average_latency_ms.unwrap_or(f64::MAX)))
    .then_with(|| left.best_latency_ms.unwrap_or(f64::MAX).total_cmp(&right.best_latency_ms.unwrap_or(f64::MAX)))
    .then_with(|| left.domain.cmp(&right.domain))
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginResponseData {
  #[serde(default, alias = "access_token")]
  access_token: Option<String>,
  #[serde(default)]
  token: Option<String>,
  #[serde(default, alias = "refresh_token")]
  refresh_token: Option<String>,
  #[serde(default, alias = "expires_in")]
  expires_in: Option<u64>,
  #[serde(default)]
  user: Option<AccountSummary>,
  #[serde(default)]
  account: Option<AccountSummary>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshResponseData {
  #[serde(default, alias = "access_token")]
  access_token: Option<String>,
  #[serde(default)]
  token: Option<String>,
  #[serde(default, alias = "refresh_token")]
  refresh_token: Option<String>,
  #[serde(default, alias = "expires_in")]
  expires_in: Option<u64>,
}

fn model_url_candidates(base_url: &str, is_full_url: bool, override_url: Option<&str>) -> Result<Vec<String>, AppError> {
  if let Some(value) = override_url.map(str::trim).filter(|value| !value.is_empty()) {
    return Ok(vec![value.to_string()]);
  }

  let trimmed = base_url.trim().trim_end_matches('/');
  if trimmed.is_empty() {
    return Err(AppError::Message("请先填写 Base URL".into()));
  }

  if is_full_url {
    return Ok(vec![trimmed.to_string()]);
  }

  let mut candidates = Vec::new();
  if trimmed.rsplit('/').next().is_some_and(|segment| {
    segment.strip_prefix('v').is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
  }) {
    candidates.push(format!("{trimmed}/models"));
  } else {
    candidates.push(format!("{trimmed}/v1/models"));
    candidates.push(format!("{trimmed}/models"));
  }

  let mut unique = Vec::new();
  for candidate in candidates {
    if !unique.iter().any(|item| item == &candidate) {
      unique.push(candidate);
    }
  }
  Ok(unique)
}


fn decode_groups_value(value: Value) -> Result<Vec<GroupSummary>, AppError> {
  let data = ApiClient::data_or_self(value);
  if data.is_array() {
    return serde_json::from_value(data).map_err(|err| AppError::Message(err.to_string()));
  }

  for key in ["groups", "items", "records", "list", "data"] {
    if let Some(items) = data.get(key).cloned() {
      if items.is_array() {
        return serde_json::from_value(items).map_err(|err| AppError::Message(err.to_string()));
      }
      if let Some(nested) = items.get("groups").cloned() {
        return serde_json::from_value(nested).map_err(|err| AppError::Message(err.to_string()));
      }
    }
  }

  Ok(Vec::new())
}
