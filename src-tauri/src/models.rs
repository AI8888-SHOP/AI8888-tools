use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AuthSession {
  #[serde(default, alias = "access_token")]
  pub access_token: String,
  #[serde(default, alias = "refresh_token")]
  pub refresh_token: String,
  #[serde(default, alias = "expires_in")]
  pub expires_in: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub email: String,
  #[serde(default)]
  pub username: Option<String>,
  #[serde(default)]
  pub role: Option<String>,
  #[serde(default)]
  pub balance: f64,
  #[serde(default)]
  pub concurrency: u32,
  #[serde(default)]
  pub status: String,
  #[serde(default, alias = "run_mode")]
  pub run_mode: Option<String>,
  #[serde(default, alias = "created_at")]
  pub created_at: Option<String>,
  #[serde(default, alias = "updated_at")]
  pub updated_at: Option<String>,
  #[serde(flatten)]
  pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindowProgress {
  #[serde(default, alias = "limit_usd")]
  pub limit_usd: Option<f64>,
  #[serde(default, alias = "used_usd")]
  pub used_usd: Option<f64>,
  #[serde(default, alias = "remaining_usd")]
  pub remaining_usd: Option<f64>,
  #[serde(default)]
  pub percentage: Option<f64>,
  #[serde(default, alias = "window_start")]
  pub window_start: Option<String>,
  #[serde(default, alias = "resets_at")]
  pub resets_at: Option<String>,
  #[serde(default, alias = "resets_in_seconds")]
  pub resets_in_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionProgress {
  #[serde(default)]
  pub id: u64,
  #[serde(default, alias = "group_name")]
  pub group_name: Option<String>,
  #[serde(default, alias = "expires_at")]
  pub expires_at: Option<String>,
  #[serde(default, alias = "expires_in_days")]
  pub expires_in_days: Option<i64>,
  #[serde(default)]
  pub daily: Option<UsageWindowProgress>,
  #[serde(default)]
  pub weekly: Option<UsageWindowProgress>,
  #[serde(default)]
  pub monthly: Option<UsageWindowProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionProgressInfo {
  pub subscription: SubscriptionSummary,
  #[serde(default)]
  pub progress: Option<SubscriptionProgress>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
  #[serde(default)]
  pub id: u64,
  #[serde(default)]
  pub name: String,
  #[serde(default)]
  pub platform: Option<String>,
  #[serde(default)]
  pub status: Option<String>,
  #[serde(default, alias = "subscription_type")]
  pub subscription_type: Option<String>,
  #[serde(default, alias = "rate_multiplier")]
  pub rate_multiplier: Option<f64>,
  #[serde(default, alias = "daily_limit_usd", alias = "daily_limit", alias = "dailyQuotaUsd")]
  pub daily_limit_usd: Option<f64>,
  #[serde(default, alias = "weekly_limit_usd", alias = "weekly_limit", alias = "weeklyQuotaUsd")]
  pub weekly_limit_usd: Option<f64>,
  #[serde(default, alias = "monthly_limit_usd", alias = "monthly_limit", alias = "monthlyQuotaUsd")]
  pub monthly_limit_usd: Option<f64>,
  #[serde(default, alias = "quota", alias = "quota_usd", alias = "monthly_quota", alias = "monthly_quota_usd", alias = "amount", alias = "amount_usd", alias = "credit", alias = "credit_usd")]
  pub quota: Option<f64>,
  #[serde(flatten)]
  pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionSummary {
  #[serde(default)]
  pub id: u64,
  #[serde(default, alias = "user_id")]
  pub user_id: u64,
  #[serde(default, alias = "group_id")]
  pub group_id: u64,
  #[serde(default, alias = "starts_at")]
  pub starts_at: Option<String>,
  #[serde(default, alias = "expires_at")]
  pub expires_at: Option<String>,
  #[serde(default)]
  pub status: String,
  #[serde(default, alias = "daily_usage_usd")]
  pub daily_usage_usd: f64,
  #[serde(default, alias = "weekly_usage_usd")]
  pub weekly_usage_usd: f64,
  #[serde(default, alias = "monthly_usage_usd")]
  pub monthly_usage_usd: f64,
  #[serde(default, alias = "quota", alias = "quota_usd", alias = "monthly_quota", alias = "monthly_quota_usd", alias = "amount", alias = "amount_usd", alias = "credit", alias = "credit_usd")]
  pub quota: Option<f64>,
  #[serde(default, alias = "remaining", alias = "remaining_usd", alias = "balance", alias = "balance_usd")]
  pub remaining: Option<f64>,
  #[serde(default)]
  pub group: Option<GroupSummary>,
  #[serde(flatten)]
  pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySummary {
  #[serde(default)]
  pub id: u64,
  #[serde(default, alias = "user_id")]
  pub user_id: u64,
  #[serde(default)]
  pub key: Option<String>,
  #[serde(default)]
  pub name: String,
  #[serde(default, alias = "group_id")]
  pub group_id: Option<u64>,
  #[serde(default)]
  pub status: Option<String>,
  #[serde(default)]
  pub quota: Option<f64>,
  #[serde(default, alias = "quota_used")]
  pub quota_used: Option<f64>,
  #[serde(default, alias = "expires_at")]
  pub expires_at: Option<String>,
  #[serde(default, alias = "last_used_at")]
  pub last_used_at: Option<String>,
  #[serde(default, alias = "created_at")]
  pub created_at: Option<String>,
  #[serde(default, alias = "updated_at")]
  pub updated_at: Option<String>,
  #[serde(default)]
  pub group: Option<GroupSummary>,
  #[serde(flatten)]
  pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
  pub id: String,
  #[serde(default)]
  pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EndpointProbeResult {
  pub domain: String,
  pub base_url: String,
  pub attempts: u32,
  pub success_count: u32,
  pub packet_loss: f64,
  pub average_latency_ms: Option<f64>,
  pub best_latency_ms: Option<f64>,
  pub selected: bool,
  pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EndpointProbeSummary {
  pub selected_base_url: String,
  pub selected_domain: String,
  pub results: Vec<EndpointProbeResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination<T> {
  #[serde(default, alias = "data", alias = "records", alias = "list")]
  pub items: Vec<T>,
  #[serde(default)]
  pub total: u64,
  #[serde(default)]
  pub page: u64,
  #[serde(default, alias = "page_size", alias = "per_page")]
  pub page_size: u64,
  #[serde(default)]
  pub pages: u64,
}

impl<T> Default for Pagination<T> {
  fn default() -> Self {
    Self {
      items: Vec::new(),
      total: 0,
      page: 1,
      page_size: 20,
      pages: 0,
    }
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolProfile {
  pub tool: String,
  pub display_name: String,
  pub description: String,
  pub directory: String,
  pub config_path: String,
  pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SwitchTarget {
  pub tool: String,
  pub profile_name: String,
  pub base_url: String,
  pub api_key: String,
  pub model: Option<String>,
  pub token_type: Option<String>,
  #[serde(default)]
  pub local_routing_enabled: bool,
  #[serde(default)]
  pub local_route_apps: Vec<String>,
  #[serde(default)]
  pub local_route_model_map: HashMap<String, String>,
  #[serde(default)]
  pub local_route_preserve_claude_auth: bool,
  #[serde(default)]
  pub local_route_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProfileInput {
  pub name: String,
  pub tool: String,
  pub base_url: String,
  pub key_id: Option<u64>,
  #[serde(default)]
  pub api_key: Option<String>,
  pub model: Option<String>,
  #[serde(default)]
  pub local_routing_enabled: bool,
  #[serde(default)]
  pub local_route_apps: Vec<String>,
  #[serde(default)]
  pub local_route_model_map: HashMap<String, String>,
  #[serde(default)]
  pub local_route_preserve_claude_auth: bool,
  #[serde(default)]
  pub local_route_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigProfile {
  pub id: String,
  pub created_at: u64,
  pub updated_at: u64,
  pub name: String,
  pub tool: String,
  pub base_url: String,
  pub key_id: Option<u64>,
  pub key_hint: Option<String>,
  #[serde(default)]
  pub has_stored_key: bool,
  pub model: Option<String>,
  #[serde(default)]
  pub local_routing_enabled: bool,
  #[serde(default)]
  pub local_route_apps: Vec<String>,
  #[serde(default)]
  pub local_route_model_map: HashMap<String, String>,
  #[serde(default)]
  pub local_route_preserve_claude_auth: bool,
  #[serde(default)]
  pub local_route_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalRouteEntry {
  pub app: String,
  pub enabled: bool,
  pub upstream_name: String,
  pub local_base_url: String,
  pub local_api_key: String,
  pub model: Option<String>,
  #[serde(default)]
  pub model_map: HashMap<String, String>,
  pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalRouteManifest {
  pub profile_name: String,
  pub updated_at: u64,
  pub entries: Vec<LocalRouteEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LocalRouteStatus {
  pub app: String,
  pub detected: bool,
  pub config_path: String,
  pub base_url_matched: bool,
  pub proxy_key_matched: bool,
  #[serde(default)]
  pub oauth_preserved: bool,
  #[serde(default)]
  pub mcp_preserved: bool,
  pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoredSession {
  pub access_token: String,
  pub refresh_token: String,
  pub expires_in: u64,
  pub account: Option<AccountSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppStateData {
  pub session: Option<StoredSession>,
  pub account: Option<AccountSummary>,
  #[serde(default)]
  pub subscriptions: Vec<SubscriptionSummary>,
  #[serde(default)]
  pub subscription_progress: Vec<SubscriptionProgressInfo>,
  #[serde(default)]
  pub groups: Vec<GroupSummary>,
  #[serde(default)]
  pub keys: Pagination<ApiKeySummary>,
  #[serde(default = "default_tool")]
  pub selected_tool: String,
  pub selected_key_id: Option<u64>,
  #[serde(default)]
  pub login_window_open: bool,
  pub last_error: Option<String>,
}

fn default_tool() -> String {
  "codex".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LoginResult {
  pub session: AuthSession,
  pub account: AccountSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckResult {
  pub current_version: String,
  pub latest_version: Option<String>,
  pub update_available: bool,
  pub release_url: Option<String>,
  pub download_url: Option<String>,
  pub download_accelerated: bool,
  pub mainland_china: bool,
  pub repository: String,
  pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppPreferences {
  #[serde(default)]
  pub onboarding_completed: bool,
  #[serde(default)]
  pub onboarding_step: u32,
  #[serde(default)]
  pub dismissed_alert_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInstallResult {
  pub success: bool,
  pub installer_path: Option<String>,
  pub launched: bool,
  pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpdateDownloadProgress {
  pub task_id: String,
  pub status: String,
  pub downloaded_bytes: u64,
  pub total_bytes: u64,
  pub percent: f64,
  pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshotFile {
  pub path: String,
  pub label: String,
  pub existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSnapshotSummary {
  pub id: String,
  pub created_at: u64,
  pub label: String,
  pub files: Vec<ConfigSnapshotFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigTransactionResult {
  pub snapshot: ConfigSnapshotSummary,
  pub artifacts: Vec<(String, String)>,
  pub message: String,
}
