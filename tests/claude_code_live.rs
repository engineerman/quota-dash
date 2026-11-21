//! Claude Code OAuth path with wiremock + temp credentials (no real network/login).

use axum::body::Body;
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use quota_dash::models::ProviderStatus;
use quota_dash::providers::claude_code::{
    fetch, fetch_with_auth, parse_credentials_json, ClaudeOAuth,
};
use quota_dash::state::ProviderApis;
use quota_dash::{app, AppState, Config};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fresh_oauth() -> ClaudeOAuth {
    ClaudeOAuth {
        access_token: "access-token".into(),
        refresh_token: Some("refresh-token".into()),
        expires_at_ms: (Utc::now() + Duration::hours(2)).timestamp_millis(),
        rate_limit_tier: Some("default_claude_max_5x".into()),
        subscription_type: Some("max".into()),
    }
}

#[tokio::test]
async fn fetch_with_auth_parses_mock_usage() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("authorization", "Bearer access-token"))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "five_hour": { "utilization": 34.5, "resets_at": "2026-11-21T20:00:00Z" },
            "seven_day": { "utilization": 58.0, "resets_at": "2026-11-28T00:00:00Z" }
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &server.uri(), &server.uri(), &fresh_oauth(), None).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "claude-code");
    assert_eq!(snap.display_name, "Claude Code");
    assert_eq!(snap.plan.as_deref(), Some("Max 5x"));
    assert!(snap.windows.iter().any(|w| w.key == "five_hour"));
    assert!(snap.windows.iter().any(|w| w.key == "seven_day"));
}

#[tokio::test]
async fn refresh_then_usage_when_token_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "new-access",
            "refresh_token": "new-refresh",
            "expires_in": 3600
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .and(header("authorization", "Bearer new-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "five_hour": { "utilization": 12.0, "resets_at": "2026-11-21T20:00:00Z" }
        })))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let cred = dir.path().join(".credentials.json");
    std::fs::write(
        &cred,
        serde_json::to_string_pretty(&json!({
            "claudeAiOauth": {
                "accessToken": "old",
                "refreshToken": "refresh-token",
                "expiresAt": 1,
                "rateLimitTier": "pro",
                "subscriptionType": "pro"
            }
        }))
        .unwrap(),
    )
    .unwrap();

    let oauth = parse_credentials_json(&std::fs::read_to_string(&cred).unwrap()).unwrap();
    assert!(oauth.is_expired_soon());

    let client = reqwest::Client::new();
    let snap = fetch_with_auth(
        &client,
        &server.uri(),
        &server.uri(),
        &oauth,
        Some(cred.as_path()),
    )
    .await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    let session = snap.windows.iter().find(|w| w.key == "five_hour").unwrap();
    assert!((session.used_percent - 12.0).abs() < 0.001);

    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cred).unwrap()).unwrap();
    assert_eq!(
        written["claudeAiOauth"]["accessToken"].as_str(),
        Some("new-access")
    );
}

#[tokio::test]
async fn needs_login_when_no_credentials() {
    let dir = tempfile::tempdir().unwrap();
    // Point CLAUDE_CONFIG_DIR at empty temp so no real ~/.claude is used.
    std::env::set_var("CLAUDE_CONFIG_DIR", dir.path());

    let client = reqwest::Client::new();
    let snap = fetch(&client, "http://127.0.0.1:9", "http://127.0.0.1:9").await;
    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert_eq!(snap.kind, "claude-code");
    assert!(
        snap.message
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("login")
            || snap
                .message
                .as_ref()
                .unwrap()
                .to_lowercase()
                .contains("credentials")
            || snap.message.as_ref().unwrap().contains("/login")
    );
    assert!(snap.windows.is_empty());

    std::env::remove_var("CLAUDE_CONFIG_DIR");
}

#[tokio::test]
async fn quotas_claude_code_endpoint_with_mock_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/oauth/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "five_hour": { "utilization": 22.0, "resets_at": "2026-11-21T20:00:00Z" },
            "seven_day": { "utilization": 40.0, "resets_at": "2026-11-28T00:00:00Z" }
        })))
        .mount(&server)
        .await;

    let claude_dir = tempfile::tempdir().unwrap();
    let expires_at = (Utc::now() + Duration::hours(2)).timestamp_millis();
    std::fs::write(
        claude_dir.path().join(".credentials.json"),
        serde_json::to_string_pretty(&json!({
            "claudeAiOauth": {
                "accessToken": "mock-access",
                "refreshToken": "mock-refresh",
                "expiresAt": expires_at,
                "rateLimitTier": "default_claude_max_5x",
                "subscriptionType": "max"
            }
        }))
        .unwrap(),
    )
    .unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", claude_dir.path());

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("quota-dash.toml");
    let mut cfg = Config::default();
    cfg.enabled_providers = vec!["claude-code".into()];
    cfg.save(&cfg_path).unwrap();

    let mut apis = ProviderApis::default();
    apis.claude_api = server.uri();
    apis.claude_oauth = server.uri();
    let state = AppState::new(cfg_path, cfg).with_apis(apis);

    let response = app(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/quotas/claude-code")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["providers"][0]["kind"], "claude-code");
    assert_eq!(json["providers"][0]["status"], "ok");
    assert!(json["providers"][0]["windows"].as_array().unwrap().len() >= 1);

    std::env::remove_var("CLAUDE_CONFIG_DIR");
    std::mem::forget(claude_dir);
}
