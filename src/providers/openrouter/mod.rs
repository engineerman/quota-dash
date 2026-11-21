//! OpenRouter live quota: GET `/api/v1/key` + `/api/v1/credits`.

use chrono::Utc;
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

pub const DEFAULT_API_BASE: &str = "https://openrouter.ai";
pub const KIND: &str = "openrouter";
pub const DISPLAY_NAME: &str = "OpenRouter";

/// Prefer a non-empty caller key (e.g. from config); otherwise `OPENROUTER_API_KEY`.
pub fn resolve_api_key(config_key: Option<&str>) -> Option<String> {
    if let Some(k) = config_key.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(k.to_string());
    }
    std::env::var("OPENROUTER_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Fetch OpenRouter quota. `config_key` is typically `Config.openrouter_api_key` when wired.
pub async fn fetch(
    http: &reqwest::Client,
    api_base: &str,
    config_key: Option<&str>,
) -> ProviderSnapshot {
    let Some(key) = resolve_api_key(config_key) else {
        return fail(
            ProviderStatus::NeedsLogin,
            "Set OPENROUTER_API_KEY or configure openrouter_api_key.",
        );
    };
    fetch_with_key(http, api_base, &key).await
}

pub async fn fetch_with_key(http: &reqwest::Client, api_base: &str, key: &str) -> ProviderSnapshot {
    let key_json = match get_json(http, api_base, key, "/api/v1/key").await {
        Ok(v) => v,
        Err(ApiError::Unauthorized) => {
            return fail(
                ProviderStatus::NeedsLogin,
                "Key rejected. Check it or create a new one.",
            );
        }
        Err(ApiError::Other(msg)) => return fail(ProviderStatus::Error, &msg),
    };

    let mut snap = ok_base();
    let mut windows: Vec<UsageWindow> = Vec::new();

    if let Some(data) = key_json.get("data").filter(|d| d.is_object()) {
        if data.get("is_free_tier").and_then(|v| v.as_bool()) == Some(true) {
            snap.plan = Some("Free".into());
        }
        if let Some(limit) = num(data, "limit").filter(|l| *l > 0.0) {
            let remaining = num(data, "limit_remaining")
                .unwrap_or_else(|| limit - num(data, "usage").unwrap_or(0.0));
            let used_pct = ((limit - remaining) / limit * 100.0).clamp(0.0, 100.0);
            let mut label = "Key limit".to_string();
            if let Some(lr) = data.get("limit_reset").and_then(|v| v.as_str()) {
                if !lr.is_empty() {
                    label = format!("Key limit ({lr})");
                }
            }
            windows.push(UsageWindow {
                key: "key_limit".into(),
                label,
                used_percent: used_pct,
                resets_at: None,
                detail: Some(format!("{} of {} left", money(remaining), money(limit))),
            });
        }
    }

    match get_json(http, api_base, key, "/api/v1/credits").await {
        Ok(credits_json) => {
            if let Some(cr) = credits_json.get("data") {
                if let Some(total) = num(cr, "total_credits").filter(|t| *t > 0.0) {
                    let used = num(cr, "total_usage").unwrap_or(0.0);
                    let used_pct = (used / total * 100.0).clamp(0.0, 100.0);
                    windows.insert(
                        0,
                        UsageWindow {
                            key: "credits".into(),
                            label: "Credits".into(),
                            used_percent: used_pct,
                            resets_at: None,
                            detail: Some(format!(
                                "{} of {} left",
                                money(total - used),
                                money(total)
                            )),
                        },
                    );
                }
            }
        }
        Err(ApiError::Unauthorized) | Err(ApiError::Other(_)) => {
            // Credits endpoint can require a different key type; keep key windows.
        }
    }

    snap.windows = windows;
    snap.fetched_at = Utc::now();
    snap
}

enum ApiError {
    Unauthorized,
    Other(String),
}

async fn get_json(
    http: &reqwest::Client,
    api_base: &str,
    key: &str,
    path: &str,
) -> Result<Value, ApiError> {
    let url = format!(
        "{}{}",
        api_base.trim_end_matches('/'),
        if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        }
    );
    let res = http
        .get(&url)
        .bearer_auth(key)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("OpenRouter API HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

fn num(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_u64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn money(v: f64) -> String {
    format!("${v:.2}")
}

fn ok_base() -> ProviderSnapshot {
    ProviderSnapshot {
        id: KIND.into(),
        kind: KIND.into(),
        display_name: DISPLAY_NAME.into(),
        account: None,
        plan: None,
        status: ProviderStatus::Ok,
        message: None,
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: KIND.into(),
        kind: KIND.into(),
        display_name: DISPLAY_NAME.into(),
        account: None,
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_prefers_config_over_empty() {
        assert_eq!(
            resolve_api_key(Some("  sk-or-from-config  ")).as_deref(),
            Some("sk-or-from-config")
        );
        assert_eq!(resolve_api_key(Some("")), None);
        assert_eq!(resolve_api_key(Some("   ")), None);
        assert_eq!(resolve_api_key(None), None);
    }
}
