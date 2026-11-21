//! Antigravity live path with mock API + fake auth (no WinCred / real network).

use axum::body::Body;
use http_body_util::BodyExt;
use quota_dash::antigravity::{fetch_with_auth, parse_cred_blob, AntigravityAuth};
use quota_dash::models::ProviderStatus;
use quota_dash::state::ProviderApis;
use quota_dash::{app, AppState, Config};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetch_with_auth_parses_mock_models() {
    let server = MockServer::start().await;
    let api_base = format!("{}/v1internal", server.uri());

    Mock::given(method("POST"))
        .and(path("/v1internal:loadCodeAssist"))
        .and(header("authorization", "Bearer fake-access"))
        .and(header("user-agent", "antigravity"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "paidTier": { "name": "Pro" },
            "cloudaicompanionProject": "proj-1"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1internal:fetchAvailableModels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "models": {
                "gemini-2.5-pro": {
                    "displayName": "Gemini 2.5 Pro",
                    "quotaInfo": {
                        "remainingFraction": 0.6,
                        "resetTime": "2099-06-01T12:00:00Z"
                    }
                },
                "claude-sonnet-4": {
                    "displayName": "Claude Sonnet 4",
                    "quotaInfo": { "remainingFraction": 0.8 }
                }
            }
        })))
        .mount(&server)
        .await;

    let auth = AntigravityAuth {
        access_token: "fake-access".into(),
        email: Some("you@gmail.com".into()),
        refresh_token: None,
        expiry: None,
        auth_method: None,
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &api_base, &auth).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "antigravity");
    assert_eq!(snap.id, "antigravity:you@gmail.com");
    assert_eq!(snap.plan.as_deref(), Some("Pro"));
    assert!(snap.windows.iter().any(|w| w.key == "gemini-pro"));
    assert!(snap.windows.iter().any(|w| w.key == "claude"));
    let pro = snap.windows.iter().find(|w| w.key == "gemini-pro").unwrap();
    assert!((pro.used_percent - 40.0).abs() < 0.01);
}

#[tokio::test]
async fn summary_windows_come_from_the_t3_quota_endpoint() {
    let server = MockServer::start().await;
    let api_base = format!("{}/v1internal", server.uri());

    Mock::given(method("POST"))
        .and(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "cloudaicompanionProject": "aicode-consumers"
        })))
        .mount(&server)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1internal:retrieveUserQuotaSummary"))
        .and(header("authorization", "Bearer fake-access"))
        .and(wiremock::matchers::body_partial_json(json!({
            "project": "aicode-consumers"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "groups": [{
                "buckets": [{
                    "bucketId": "gemini-pro-5h",
                    "window": "5h",
                    "remainingFraction": 0.26,
                    "resetTime": "2099-06-01T12:00:00Z"
                }]
            }]
        })))
        .mount(&server)
        .await;

    let auth = AntigravityAuth {
        access_token: "fake-access".into(),
        email: Some("you@gmail.com".into()),
        refresh_token: None,
        expiry: None,
        auth_method: Some("consumer".into()),
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &api_base, &auth).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.plan, None);
    assert_eq!(snap.windows.len(), 1);
    assert_eq!(snap.windows[0].label, "Gemini · 5-hour");
    assert!((snap.windows[0].used_percent - 74.0).abs() < 0.01);
}

#[tokio::test]
async fn unauthorized_is_needs_login() {
    let server = MockServer::start().await;
    let api_base = format!("{}/v1internal", server.uri());

    Mock::given(method("POST"))
        .and(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let auth = AntigravityAuth {
        access_token: "bad".into(),
        email: Some("a@b.com".into()),
        refresh_token: None,
        expiry: None,
        auth_method: None,
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &api_base, &auth).await;
    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert_eq!(snap.id, "antigravity:a@b.com");
    assert!(snap
        .message
        .as_ref()
        .unwrap()
        .to_lowercase()
        .contains("agy"));
}

#[test]
fn fake_token_blob_parses_like_wincred() {
    let blob = json!({
        "id_token_email": "ci@example.com",
        "token": {
            "access_token": "inject-me",
            "refresh_token": "rt",
            "expiry": "2099-01-01T00:00:00Z"
        }
    })
    .to_string();
    let auth = parse_cred_blob(&blob).unwrap();
    assert_eq!(auth.access_token, "inject-me");
    assert_eq!(auth.email.as_deref(), Some("ci@example.com"));
}

#[tokio::test]
async fn quotas_antigravity_needs_login_without_wincred_blob() {
    // Without a gemini:antigravity WinCred entry, fetch returns needs_login (not blank).
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("quota-dash.toml");
    let mut cfg = Config::default();
    cfg.enabled_providers = vec!["antigravity".into()];
    cfg.save(&cfg_path).unwrap();

    let mut apis = ProviderApis::default();
    // Point at a dead base so we never hit the real Google API if auth somehow exists.
    apis.antigravity = "http://127.0.0.1:9/v1internal".into();
    let state = AppState::new(cfg_path, cfg).with_apis(apis);

    let response = app(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/quotas/antigravity")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["providers"][0]["kind"], "antigravity");
    // Either needs_login (no cred) or ok/error if this machine has a real agy login.
    let status = json["providers"][0]["status"].as_str().unwrap();
    assert!(
        status == "needs_login" || status == "stale" || status == "ok" || status == "error",
        "unexpected status {status}"
    );
    if status == "needs_login" {
        let msg = json["providers"][0]["message"].as_str().unwrap_or("");
        assert!(msg.to_lowercase().contains("agy"));
    }
}
