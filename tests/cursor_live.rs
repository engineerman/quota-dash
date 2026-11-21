//! Cursor live path with mock API + fake sqlite auth (no real network/login).

use axum::body::Body;
use http_body_util::BodyExt;
use quota_dash::providers::cursor::{fetch_with_auth, CursorAuth};
use quota_dash::{app, AppState, Config};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn fetch_with_auth_parses_mock_dashboard() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aiserver.v1.DashboardService/GetCurrentPeriodUsage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "billingCycleStart": "1792695957000",
            "billingCycleEnd": "1795374357000",
            "planUsage": {
                "autoPercentUsed": 42.5,
                "apiPercentUsed": 10.25,
                "totalPercentUsed": 35.0
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aiserver.v1.DashboardService/GetPlanInfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "planInfo": { "planName": "Pro" }
        })))
        .mount(&server)
        .await;

    let auth = CursorAuth {
        token: "fake".into(),
        email: Some("you@example.com".into()),
        membership: Some("pro".into()),
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_auth(&client, &server.uri(), &auth).await;
    assert_eq!(snap.status, quota_dash::models::ProviderStatus::Ok);
    assert_eq!(snap.plan.as_deref(), Some("Pro"));
    assert!(snap.windows.iter().any(|w| w.key == "cursor_models"));
}

#[tokio::test]
async fn quotas_cursor_endpoint_with_mock_and_temp_db() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/aiserver.v1.DashboardService/GetCurrentPeriodUsage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "billingCycleStart": "1792695957000",
            "billingCycleEnd": "1795374357000",
            "planUsage": {
                "autoPercentUsed": 10.0,
                "apiPercentUsed": 5.0,
                "totalPercentUsed": 8.0
            }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/aiserver.v1.DashboardService/GetPlanInfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "planInfo": { "planName": "Pro" }
        })))
        .mount(&server)
        .await;

    // Point APPDATA at a temp tree with plaintext Cursor token.
    let appdata = tempfile::tempdir().unwrap();
    let db_dir = appdata
        .path()
        .join("Cursor")
        .join("User")
        .join("globalStorage");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = db_dir.join("state.vscdb");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ItemTable VALUES ('cursorAuth/accessToken', 'mock-token');
             INSERT INTO ItemTable VALUES ('cursorAuth/cachedEmail', 'mock@example.com');
             INSERT INTO ItemTable VALUES ('cursorAuth/stripeMembershipType', 'pro');",
        )
        .unwrap();
    }
    std::env::set_var("APPDATA", appdata.path());

    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("quota-dash.toml");
    let mut cfg = Config::default();
    cfg.enabled_providers = vec!["cursor".into()];
    cfg.save(&cfg_path).unwrap();
    let state = AppState::new(cfg_path, cfg).with_cursor_api_base(server.uri());

    let response = app(state)
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/quotas/cursor")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["providers"][0]["kind"], "cursor");
    assert_eq!(json["providers"][0]["status"], "ok");
    assert!(json["providers"][0]["windows"].as_array().unwrap().len() >= 1);

    // Avoid leaking APPDATA mutation into other tests as much as possible.
    std::env::remove_var("APPDATA");
    std::mem::forget(appdata);
}
