//! OpenCode live path with mock API + fake local credentials (no real network/login).

use quota_dash::models::ProviderStatus;
use quota_dash::providers::opencode::{discover_keys, fetch_from, fetch_with_key, OpenCodeKey};
use rusqlite::Connection;
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn usage_body() -> serde_json::Value {
    json!({
        "usage": {
            "rolling": {
                "percent": 5.0,
                "resetsAt": "2026-12-01T12:00:00Z",
                "status": "ok"
            },
            "weekly": {
                "percent": 15.5,
                "resetsAt": "2026-12-07T00:00:00Z"
            },
            "monthly": {
                "percent": 40.0,
                "resetsAt": "2027-01-01T00:00:00Z"
            }
        }
    })
}

#[tokio::test]
async fn fetch_with_key_parses_mock_zen_usage() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/zen/go/v1/usage"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage_body()))
        .mount(&server)
        .await;

    let key = OpenCodeKey {
        id: "k1".into(),
        label: "Go Main".into(),
        key: "sk-test".into(),
        active: true,
    };
    let client = reqwest::Client::new();
    let snap = fetch_with_key(&client, &server.uri(), &key).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "opencode");
    assert_eq!(snap.display_name, "OpenCode");
    assert_eq!(snap.plan.as_deref(), Some("Go"));
    assert_eq!(snap.account.as_deref(), Some("Go Main"));
    let weekly = snap.windows.iter().find(|w| w.key == "weekly").unwrap();
    assert!((weekly.used_percent - 15.5).abs() < 0.001);
    assert!(weekly.resets_at.is_some());
    assert!(snap.windows.iter().any(|w| w.key == "rolling"));
    assert!(snap.windows.iter().any(|w| w.key == "monthly"));
}

#[tokio::test]
async fn fetch_from_discovers_db_key_and_hits_mock() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/zen/go/v1/usage"))
        .respond_with(ResponseTemplate::new(200).set_body_json(usage_body()))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("opencode.db");
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE credential (
                id TEXT PRIMARY KEY,
                label TEXT,
                value TEXT,
                active INTEGER,
                integration_id TEXT
             );
             INSERT INTO credential VALUES (
                'db1', 'From DB', '{\"key\":\"sk-db\"}', 1, 'opencode-go'
             );",
        )
        .unwrap();
    }

    let keys = discover_keys(dir.path());
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].key, "sk-db");

    let client = reqwest::Client::new();
    let snap = fetch_from(&client, &server.uri(), Some(dir.path()), None).await;
    assert_eq!(snap.status, ProviderStatus::Ok);
    assert_eq!(snap.kind, "opencode");
    assert!(snap.windows.len() >= 2);
}

#[tokio::test]
async fn missing_credentials_needs_login_without_spam() {
    let dir = tempfile::tempdir().unwrap();
    let client = reqwest::Client::new();
    let snap = fetch_from(&client, "http://127.0.0.1:1", Some(dir.path()), None).await;
    assert_eq!(snap.status, ProviderStatus::NeedsLogin);
    assert!(snap.windows.is_empty());
    let msg = snap.message.unwrap_or_default().to_lowercase();
    assert!(msg.contains("no opencode key"));
}
