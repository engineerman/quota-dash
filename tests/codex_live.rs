//! Codex live path with mock usage/refresh HTTP and fake auth.json (no real network/login).

use quota_dash::models::ProviderStatus;
use quota_dash::providers::codex::{fetch, fetch_with_auth, fetch_with_auth_refresh, CodexAuth};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sample_usage() -> serde_json::Value {
    json!({
        "email": "you@example.com",
        "plan_type": "plus",
        "rate_limit": {
            "primary_window": {
                "used_percent": 42.5,
                "limit_window_seconds": 18000,
                "reset_at": 1893456000
            },
            "secondary_window": {
                "used_percent": 10.0,
                "limit_window_seconds": 604800,
                "reset_at": 1894060800
            }
        }
    })
}

#[tokio::test]
async fn fetch_with_auth_parses_mock_usage() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/backend-api/wham/usage"))
        .and(header("authorization", "Bearer fake-token"))
        .and(header("chatgpt-account-id", "acc-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sample_usage()))
        .mount(&server)
        .await;

    let auth = CodexAuth {
        access_token: "fake-token".into(),
        refresh_token: Some("rt".into()),
        account_id: Some("acc-1".into()),
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &server.uri(), &auth).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "codex");
    assert_eq!(snap.plan.as_deref(), Some("Plus"));
    assert!(snap.windows.iter().any(|w| w.key == "primary"));
    assert!(snap.windows.iter().any(|w| w.key == "secondary"));
}

#[tokio::test]
async fn refresh_on_401_then_retry() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/backend-api/wham/usage"))
        .and(header("authorization", "Bearer stale"))
        .respond_with(ResponseTemplate::new(401))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh",
            "refresh_token": "rt2",
            "token_type": "Bearer"
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/backend-api/wham/usage"))
        .and(header("authorization", "Bearer fresh"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sample_usage()))
        .mount(&server)
        .await;

    let auth = CodexAuth {
        access_token: "stale".into(),
        refresh_token: Some("rt".into()),
        account_id: None,
    };
    let refresh_url = format!("{}/oauth/token", server.uri());
    let client = reqwest::Client::new();
    let snap = fetch_with_auth_refresh(&client, &server.uri(), &refresh_url, &auth).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.account.as_deref(), Some("you@example.com"));
}

#[tokio::test]
async fn missing_auth_is_needs_login() {
    let home = tempfile::tempdir().unwrap();
    // Empty CODEX_HOME — no auth.json
    std::env::set_var("CODEX_HOME", home.path());

    let client = reqwest::Client::new();
    let snap = fetch(&client, "http://127.0.0.1:9").await;
    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert_eq!(snap.kind, "codex");
    assert!(snap
        .message
        .as_deref()
        .unwrap_or("")
        .to_lowercase()
        .contains("codex login"));

    std::env::remove_var("CODEX_HOME");
    std::mem::forget(home);
}
