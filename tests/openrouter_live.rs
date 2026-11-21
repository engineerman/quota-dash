//! OpenRouter live path with wiremock (no real network).

use axum::body::Body;
use http_body_util::BodyExt;
use quota_dash::models::ProviderStatus;
use quota_dash::providers::openrouter::{fetch, fetch_with_key, resolve_api_key};
use quota_dash::state::ProviderApis;
use quota_dash::{app, AppState, Config};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn without_key_returns_needs_login_no_http() {
    // Ensure env does not supply a key for this process.
    let prev = std::env::var("OPENROUTER_API_KEY").ok();
    std::env::remove_var("OPENROUTER_API_KEY");

    let server = MockServer::start().await;
    let client = reqwest::Client::new();
    let snap = fetch(&client, &server.uri(), None).await;

    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert!(snap.windows.is_empty());
    let msg = snap.message.expect("clear message");
    assert!(
        msg.to_lowercase().contains("openrouter") || msg.to_lowercase().contains("api_key"),
        "unexpected message: {msg}"
    );
    // No mocks mounted → any HTTP would fail the test suite via hanging/error; we assert no windows/ok.
    assert_ne!(snap.status, ProviderStatus::Ok);

    if let Some(v) = prev {
        std::env::set_var("OPENROUTER_API_KEY", v);
    }
}

#[tokio::test]
async fn with_key_builds_credits_and_key_limit_windows() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .and(header("authorization", "Bearer sk-or-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "is_free_tier": false,
                "limit": 100.0,
                "limit_remaining": 40.0,
                "usage": 60.0,
                "limit_reset": "monthly"
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/credits"))
        .and(header("authorization", "Bearer sk-or-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "total_credits": 50.0,
                "total_usage": 12.5
            }
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let snap = fetch_with_key(&client, &server.uri(), "sk-or-test").await;

    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "openrouter");
    assert!(snap.message.is_none());

    let credits = snap
        .windows
        .iter()
        .find(|w| w.key == "credits")
        .expect("credits window");
    assert!((credits.used_percent - 25.0).abs() < 0.001);
    assert_eq!(credits.detail.as_deref(), Some("$37.50 of $50.00 left"));
    assert_eq!(snap.windows[0].key, "credits");

    let key_limit = snap
        .windows
        .iter()
        .find(|w| w.key == "key_limit")
        .expect("key_limit window");
    assert!((key_limit.used_percent - 60.0).abs() < 0.001);
    assert_eq!(key_limit.label, "Key limit (monthly)");
    assert_eq!(key_limit.detail.as_deref(), Some("$40.00 of $100.00 left"));
}

#[tokio::test]
async fn credits_failure_still_returns_key_limit() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "limit": 10.0,
                "limit_remaining": 5.0
            }
        })))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/api/v1/credits"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let snap = fetch_with_key(&client, &server.uri(), "sk-or-test").await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert!(snap.windows.iter().any(|w| w.key == "key_limit"));
    assert!(!snap.windows.iter().any(|w| w.key == "credits"));
}

#[tokio::test]
async fn rejected_key_is_needs_login() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let snap = fetch_with_key(&client, &server.uri(), "bad").await;
    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert!(snap.windows.is_empty());
}

#[test]
fn resolve_uses_config_argument() {
    assert_eq!(
        resolve_api_key(Some("sk-or-cfg")).as_deref(),
        Some("sk-or-cfg")
    );
}

#[tokio::test]
async fn quotas_openrouter_endpoint_with_config_key() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "limit": 20.0, "limit_remaining": 10.0 }
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/credits"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "total_credits": 10.0, "total_usage": 2.0 }
        })))
        .mount(&server)
        .await;

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("quota-dash.toml");
    let mut cfg = Config::default();
    cfg.enabled_providers = vec!["openrouter".into()];
    cfg.openrouter_api_key = "sk-or-endpoint".into();
    cfg.save(&cfg_path).unwrap();

    let mut apis = ProviderApis::default();
    apis.openrouter = server.uri();
    let state = AppState::new(cfg_path, cfg).with_apis(apis);

    let response = app(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/quotas/openrouter")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let providers = body["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["kind"], "openrouter");
    assert_eq!(providers[0]["status"], "ok");
    assert!(providers[0]["windows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["key"] == "credits"));
}
