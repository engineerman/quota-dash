mod keys;
mod parse;

pub use keys::{default_opencode_dir, discover_keys, OpenCodeKey};
pub use parse::parse_usage;

use crate::models::{ProviderSnapshot, ProviderStatus};
use chrono::Utc;
use serde_json::Value;

/// Host root; usage is fetched from `{base}/zen/go/v1/usage`.
pub const DEFAULT_API_BASE: &str = "https://opencode.ai";

/// Discover local OpenCode Go keys and fetch live usage.
///
/// `config_key` is an optional pasted key from config when discovery is empty.
/// Pass [`DEFAULT_API_BASE`] in production; tests inject a wiremock base.
pub async fn fetch(
    http: &reqwest::Client,
    api_base: &str,
    config_key: Option<&str>,
) -> ProviderSnapshot {
    fetch_from(
        http,
        api_base,
        default_opencode_dir().as_deref(),
        config_key,
    )
    .await
}

/// Like [`fetch`], but with injectable API base and OpenCode data directory (for tests).
pub async fn fetch_from(
    http: &reqwest::Client,
    api_base: &str,
    opencode_dir: Option<&std::path::Path>,
    config_key: Option<&str>,
) -> ProviderSnapshot {
    let mut keys = match opencode_dir {
        Some(dir) => discover_keys(dir),
        None => Vec::new(),
    };
    if keys.is_empty() {
        if let Some(k) = config_key.map(str::trim).filter(|s| !s.is_empty()) {
            keys.push(OpenCodeKey {
                id: "config".into(),
                label: "Config".into(),
                key: k.to_string(),
                active: true,
            });
        }
    }
    if keys.is_empty() {
        // Honest needs_login once — do not hit the network or spam errors.
        return parse::fail(
            ProviderStatus::NeedsLogin,
            "No OpenCode key found. Paste one here or use OpenCode `/connect`.",
            None,
        );
    }

    let mut results = Vec::with_capacity(keys.len());
    for k in &keys {
        let snap = fetch_with_key(http, api_base, k).await;
        results.push((k.active, snap));
    }
    pick_preferred(results)
}

/// Fetch usage for one known API key against `api_base` (wiremock-friendly).
pub async fn fetch_with_key(
    http: &reqwest::Client,
    api_base: &str,
    key: &OpenCodeKey,
) -> ProviderSnapshot {
    match get_usage(http, api_base, &key.key).await {
        Ok(json) => {
            let mut snap = parse_usage(&json, key);
            snap.fetched_at = Utc::now();
            snap
        }
        Err(ApiError::Unauthorized) => {
            parse::fail(ProviderStatus::NeedsLogin, "API key rejected.", Some(key))
        }
        Err(ApiError::Other(msg)) => parse::fail(ProviderStatus::Error, &msg, Some(key)),
    }
}

enum ApiError {
    Unauthorized,
    Other(String),
}

async fn get_usage(
    http: &reqwest::Client,
    api_base: &str,
    api_key: &str,
) -> Result<Value, ApiError> {
    let url = format!("{}/zen/go/v1/usage", api_base.trim_end_matches('/'));
    let res = http
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let status = res.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        return Err(ApiError::Other(format!("OpenCode usage HTTP {status}")));
    }
    res.json::<Value>()
        .await
        .map_err(|e| ApiError::Other(e.to_string()))
}

/// Several keys of the same Go subscription report identical usage: keep one (prefer active).
fn pick_preferred(results: Vec<(bool, ProviderSnapshot)>) -> ProviderSnapshot {
    use std::collections::HashMap;

    let mut best_by_sig: HashMap<String, (bool, ProviderSnapshot)> = HashMap::new();
    for (active, snap) in results {
        let sig = if snap.status == ProviderStatus::Ok {
            usage_sig(&snap)
        } else {
            snap.id.clone()
        };
        best_by_sig
            .entry(sig)
            .and_modify(|(existing_active, existing)| {
                if active && !*existing_active {
                    *existing_active = true;
                    *existing = snap.clone();
                }
            })
            .or_insert((active, snap));
    }

    let mut out: Vec<_> = best_by_sig.into_values().collect();
    out.sort_by(|(a_active, a), (b_active, b)| {
        b_active.cmp(a_active).then_with(|| a.id.cmp(&b.id))
    });
    out.into_iter()
        .next()
        .map(|(_, snap)| snap)
        .unwrap_or_else(|| {
            parse::fail(
                ProviderStatus::NeedsLogin,
                "No OpenCode key found. Add one in OpenCode (`/connect`).",
                None,
            )
        })
}

fn usage_sig(s: &ProviderSnapshot) -> String {
    s.windows
        .iter()
        .filter(|w| w.key != "rolling")
        .map(|w| {
            let reset_min = w.resets_at.map(|t| t.timestamp() / 60).unwrap_or(0);
            format!("{}:{:.1}:{}", w.key, w.used_percent, reset_min)
        })
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn usage_body() -> serde_json::Value {
        json!({
            "usage": {
                "rolling": { "percent": 10.0, "resetsAt": "2026-12-01T00:00:00Z", "status": "ok" },
                "weekly": { "percent": 20.0, "resetsAt": "2026-12-07T00:00:00Z" },
                "monthly": { "percent": 30.0, "resetsAt": "2027-01-01T00:00:00Z" }
            }
        })
    }

    #[tokio::test]
    async fn fetch_with_key_parses_mock_usage() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zen/go/v1/usage"))
            .and(header("authorization", "Bearer sk-mock"))
            .respond_with(ResponseTemplate::new(200).set_body_json(usage_body()))
            .mount(&server)
            .await;

        let key = OpenCodeKey {
            id: "k1".into(),
            label: "Go Main".into(),
            key: "sk-mock".into(),
            active: true,
        };
        let client = reqwest::Client::new();
        let snap = fetch_with_key(&client, &server.uri(), &key).await;
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.kind, "opencode");
        assert_eq!(snap.plan.as_deref(), Some("Go"));
        assert!(snap.windows.iter().any(|w| w.key == "weekly"));
        assert!(snap.windows.iter().any(|w| w.key == "monthly"));
    }

    #[tokio::test]
    async fn fetch_with_key_unauthorized_is_needs_login() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/zen/go/v1/usage"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let key = OpenCodeKey {
            id: "bad".into(),
            label: "Bad".into(),
            key: "sk-bad".into(),
            active: false,
        };
        let client = reqwest::Client::new();
        let snap = fetch_with_key(&client, &server.uri(), &key).await;
        assert_eq!(snap.status, ProviderStatus::NeedsLogin);
        assert!(snap.windows.is_empty());
    }

    #[tokio::test]
    async fn fetch_from_missing_keys_is_needs_login_without_http() {
        let dir = tempfile::tempdir().unwrap();
        let client = reqwest::Client::new();
        // api_base would fail if contacted; empty dir must not call network.
        let snap = fetch_from(&client, "http://127.0.0.1:1", Some(dir.path()), None).await;
        assert_eq!(snap.status, ProviderStatus::NeedsLogin);
        assert!(snap
            .message
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .contains("no opencode key"));
    }
}
