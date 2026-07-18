use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::{ensure_app_dir, usage_path};
use crate::workspace::{load_model_prices, ModelPrice};

const DAY_MS: u64 = 86_400_000;
const DEFAULT_DASHBOARD_DAYS: u32 = 30;
const MAX_DASHBOARD_DAYS: u32 = 366;

static USAGE_FILE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn usage_file_lock() -> &'static Mutex<()> {
    USAGE_FILE_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub timestamp_ms: u64,
    pub protocol: String,
    pub endpoint: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: f64,
    pub status_code: u16,
    pub latency_ms: u64,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UsageRecord {
    pub fn new(
        protocol: impl Into<String>,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        input_tokens: u64,
        output_tokens: u64,
        cached_input_tokens: u64,
        status_code: u16,
        latency_ms: u64,
        error: Option<String>,
    ) -> Self {
        let model = model.into();
        Self {
            timestamp_ms: now_ms(),
            protocol: protocol.into(),
            endpoint: endpoint.into(),
            cost_usd: calculate_cost(&model, input_tokens, output_tokens, cached_input_tokens),
            model,
            input_tokens,
            output_tokens,
            cached_input_tokens,
            status_code,
            latency_ms,
            success: (200..400).contains(&status_code) && error.is_none(),
            error: error.map(|value| value.chars().take(300).collect()),
        }
    }
}

fn calculate_cost(
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cached_input_tokens: u64,
) -> f64 {
    let prices = load_model_prices();
    let Some(price) = find_model_price(&prices, model) else {
        return 0.0;
    };
    let cached = cached_input_tokens.min(input_tokens);
    let uncached = input_tokens.saturating_sub(cached);
    (uncached as f64 * price.input_per_million
        + cached as f64 * price.cached_input_per_million
        + output_tokens as f64 * price.output_per_million)
        / 1_000_000.0
}

fn find_model_price<'a>(prices: &'a [ModelPrice], model: &str) -> Option<&'a ModelPrice> {
    let model = model.to_ascii_lowercase();
    prices
        .iter()
        .filter(|price| {
            let candidate = price.model.to_ascii_lowercase();
            model == candidate || model.starts_with(&format!("{candidate}-"))
        })
        .max_by_key(|price| price.model.len())
}

pub fn record_usage(record: UsageRecord) {
    if ensure_app_dir().is_err() {
        return;
    }
    let Ok(line) = serde_json::to_string(&record) else {
        return;
    };
    let Ok(_guard) = usage_file_lock().lock() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(usage_path())
    {
        let _ = writeln!(file, "{line}");
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: f64,
    pub average_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageModelBreakdown {
    pub model: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: f64,
    pub average_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageEndpointBreakdown {
    pub endpoint: String,
    pub requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub average_latency_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageDailyBucket {
    pub date: String,
    pub start_ms: u64,
    pub requests: u64,
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageDashboard {
    pub generated_at_ms: u64,
    pub since_ms: u64,
    pub days: u32,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_cost_usd: f64,
    pub average_latency_ms: f64,
    pub by_model: Vec<UsageModelBreakdown>,
    pub by_endpoint: Vec<UsageEndpointBreakdown>,
    pub daily: Vec<UsageDailyBucket>,
}

#[derive(Default)]
struct TotalsAccumulator {
    totals: UsageTotals,
    latency_sum: u128,
}

impl TotalsAccumulator {
    fn add(&mut self, record: &UsageRecord) {
        self.totals.requests += 1;
        if record.success {
            self.totals.successful_requests += 1
        } else {
            self.totals.failed_requests += 1
        };
        self.totals.input_tokens += record.input_tokens;
        self.totals.output_tokens += record.output_tokens;
        self.totals.cached_input_tokens += record.cached_input_tokens;
        self.totals.cost_usd += record.cost_usd;
        self.latency_sum += record.latency_ms as u128;
    }

    fn finish(mut self) -> UsageTotals {
        if self.totals.requests > 0 {
            self.totals.average_latency_ms = self.latency_sum as f64 / self.totals.requests as f64;
        }
        self.totals
    }
}

#[tauri::command]
pub fn app_get_usage_dashboard(days: Option<u32>) -> Result<UsageDashboard, String> {
    let days = days
        .unwrap_or(DEFAULT_DASHBOARD_DAYS)
        .clamp(1, MAX_DASHBOARD_DAYS);
    let generated_at_ms = now_ms();
    let since_ms = generated_at_ms.saturating_sub(days as u64 * DAY_MS);
    let _guard = usage_file_lock()
        .lock()
        .map_err(|_| "usage file lock is poisoned".to_string())?;
    let content = match fs::read_to_string(usage_path()) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("read usage history failed: {error}")),
    };

    let mut totals = TotalsAccumulator::default();
    let mut models: HashMap<String, TotalsAccumulator> = HashMap::new();
    let mut endpoints: HashMap<String, TotalsAccumulator> = HashMap::new();
    let mut daily: HashMap<u64, TotalsAccumulator> = HashMap::new();

    for record in content
        .lines()
        .filter_map(|line| serde_json::from_str::<UsageRecord>(line).ok())
    {
        if record.timestamp_ms < since_ms
            || record.timestamp_ms > generated_at_ms.saturating_add(60_000)
        {
            continue;
        }
        totals.add(&record);
        models.entry(record.model.clone()).or_default().add(&record);
        endpoints
            .entry(record.endpoint.clone())
            .or_default()
            .add(&record);
        daily
            .entry(record.timestamp_ms / DAY_MS * DAY_MS)
            .or_default()
            .add(&record);
    }

    let mut by_model = models
        .into_iter()
        .map(|(model, value)| {
            let totals = value.finish();
            UsageModelBreakdown {
                model,
                requests: totals.requests,
                input_tokens: totals.input_tokens,
                output_tokens: totals.output_tokens,
                cached_input_tokens: totals.cached_input_tokens,
                cost_usd: totals.cost_usd,
                average_latency_ms: totals.average_latency_ms,
            }
        })
        .collect::<Vec<_>>();
    let mut by_endpoint = endpoints
        .into_iter()
        .map(|(endpoint, value)| {
            let totals = value.finish();
            UsageEndpointBreakdown {
                endpoint,
                requests: totals.requests,
                successful_requests: totals.successful_requests,
                failed_requests: totals.failed_requests,
                input_tokens: totals.input_tokens,
                output_tokens: totals.output_tokens,
                cost_usd: totals.cost_usd,
                average_latency_ms: totals.average_latency_ms,
            }
        })
        .collect::<Vec<_>>();
    let mut daily = daily
        .into_iter()
        .map(|(start_ms, value)| {
            let totals = value.finish();
            UsageDailyBucket {
                date: utc_date(start_ms),
                start_ms,
                requests: totals.requests,
                tokens: totals.input_tokens + totals.output_tokens,
                cost_usd: totals.cost_usd,
            }
        })
        .collect::<Vec<_>>();
    by_model.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then_with(|| b.requests.cmp(&a.requests))
    });
    by_endpoint.sort_by(|a, b| b.requests.cmp(&a.requests));
    daily.sort_by_key(|item| item.start_ms);

    let totals = totals.finish();
    Ok(UsageDashboard {
        generated_at_ms,
        since_ms,
        days,
        total_requests: totals.requests,
        successful_requests: totals.successful_requests,
        failed_requests: totals.failed_requests,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        cached_input_tokens: totals.cached_input_tokens,
        total_cost_usd: totals.cost_usd,
        average_latency_ms: totals.average_latency_ms,
        by_model,
        by_endpoint,
        daily,
    })
}

fn utc_date(timestamp_ms: u64) -> String {
    let days = (timestamp_ms / DAY_MS) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

#[tauri::command]
pub fn app_clear_usage() -> Result<(), String> {
    ensure_app_dir().map_err(|error| error.to_string())?;
    let _guard = usage_file_lock()
        .lock()
        .map_err(|_| "usage file lock is poisoned".to_string())?;
    fs::write(usage_path(), b"").map_err(|error| format!("clear usage history failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_totals_are_aggregated() {
        let mut accumulator = TotalsAccumulator::default();
        accumulator.add(&UsageRecord {
            success: true,
            input_tokens: 10,
            output_tokens: 5,
            cached_input_tokens: 2,
            cost_usd: 0.1,
            latency_ms: 30,
            ..Default::default()
        });
        accumulator.add(&UsageRecord {
            success: false,
            input_tokens: 3,
            output_tokens: 0,
            cost_usd: 0.02,
            latency_ms: 10,
            ..Default::default()
        });
        let totals = accumulator.finish();
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.failed_requests, 1);
        assert_eq!(totals.input_tokens, 13);
        assert_eq!(totals.average_latency_ms, 20.0);
    }
}
