//! Primary seam: HTTP API behaviour.

use axum::body::Body;
use http_body_util::BodyExt;
use quota_dash::{app, AppState, Config};
use serde_json::{json, Value};
use tower::ServiceExt;

fn test_state() -> AppState {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota-dash.toml");
    // Keep tempdir alive by leaking — fine for tests.
    std::mem::forget(dir);
    let cfg = Config::default();
    cfg.save(&path).unwrap();
    AppState::new(path, cfg)
}

async fn request(
    state: AppState,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (axum::http::StatusCode, Value) {
    request_with_lang(state, method, path, body, None).await
}

async fn request_with_lang(
    state: AppState,
    method: &str,
    path: &str,
    body: Option<Value>,
    accept_language: Option<&str>,
) -> (axum::http::StatusCode, Value) {
    let mut builder = axum::http::Request::builder().method(method).uri(path);
    if let Some(lang) = accept_language {
        builder = builder.header("accept-language", lang);
    }
    let req = if let Some(b) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let response = app(state).oneshot(req).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("response JSON")
    };
    (status, json)
}

#[tokio::test]
async fn ui_index_serves_add_provider_page() {
    let response = app(test_state())
        .oneshot(
            axum::http::Request::builder()
                .uri("/")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let html = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(html.contains("page-title"));
    assert!(html.contains("/api/quotas"));
    assert!(html.contains("/static/i18n-boot.js"));
    assert!(html.contains("/static/ui-fallback-en.js"));
    assert!(html.contains("settings-open"));
    assert!(html.contains("theme-toggle"));
    assert!(html.contains("qb-theme"));
    assert!(html.contains("/api/auth/antigravity/start"));
    assert!(html.contains("/api/secrets/"));
}

#[tokio::test]
async fn locales_tr_serves_ui_bundle() {
    let (status, json) = request(test_state(), "GET", "/api/locales/tr", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["lang"], "tr");
    assert_eq!(json["ui"]["page_title"], "quota-dash");
    assert_eq!(json["ui"]["theme_to_dark"], "Koyu");
}

#[tokio::test]
async fn locales_en_serves_ui_bundle() {
    let (status, json) = request(test_state(), "GET", "/api/locales/en", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["lang"], "en");
    assert_eq!(json["ui"]["page_title"], "quota-dash");
    assert_eq!(json["ui"]["theme_to_dark"], "Dark");
    assert_eq!(json["dir"], "ltr");
}

#[tokio::test]
async fn locales_ar_is_rtl_and_unknown_is_missing() {
    let (status, json) = request(test_state(), "GET", "/api/locales/ar", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["lang"], "ar");
    assert_eq!(json["dir"], "rtl");
    assert_eq!(json["ui"]["page_title"], "quota-dash");

    let (status, _) = request(test_state(), "GET", "/api/locales/sv", None).await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn system_locale_is_a_shipped_language() {
    let (status, json) = request(test_state(), "GET", "/api/system-locale", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let lang = json["lang"].as_str().unwrap();
    let (settings_status, settings) = request(test_state(), "GET", "/api/settings", None).await;
    assert_eq!(settings_status, axum::http::StatusCode::OK);
    let codes: Vec<&str> = settings["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["code"].as_str().unwrap())
        .collect();
    assert!(codes.contains(&lang), "{lang} not in {codes:?}");
    let dir = json["dir"].as_str().unwrap();
    assert!(dir == "ltr" || dir == "rtl");
}

#[tokio::test]
async fn catalog_defaults_to_english_copy() {
    let (status, json) = request(test_state(), "GET", "/api/providers/catalog", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let cursor = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "cursor")
        .unwrap();
    assert!(cursor["summary"].as_str().unwrap().contains("browser"));
}

#[tokio::test]
async fn catalog_honors_accept_language() {
    let (status, json) = request_with_lang(
        test_state(),
        "GET",
        "/api/providers/catalog",
        None,
        Some("tr-TR,tr;q=0.9,en;q=0.8"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let cursor = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "cursor")
        .unwrap();
    assert!(cursor["summary"].as_str().unwrap().contains("tarayıcı"));

    let (status, json) = request_with_lang(
        test_state(),
        "GET",
        "/api/providers/catalog",
        None,
        Some("de-DE,de;q=0.9"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let cursor = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "cursor")
        .unwrap();
    assert!(cursor["summary"].as_str().unwrap().contains("Sitzung"));
}

#[tokio::test]
async fn settings_api_returns_languages_and_config() {
    let (status, json) = request(test_state(), "GET", "/api/settings", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["languages"].as_array().unwrap().len(), 18);
    assert!(json["config"]["refresh_interval_secs"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn catalog_honors_lang_query_en() {
    let (status, json) = request(test_state(), "GET", "/api/providers/catalog?lang=en", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let cursor = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "cursor")
        .unwrap();
    assert!(cursor["summary"].as_str().unwrap().contains("browser"));
}

#[tokio::test]
async fn catalog_includes_setup_guidance() {
    let (status, json) = request(test_state(), "GET", "/api/providers/catalog", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let antigravity = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "antigravity")
        .unwrap();
    assert_eq!(antigravity["setup"], "oauth_code");
    assert_eq!(antigravity["command"], "agy acp --login");
    let openrouter = json
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "openrouter")
        .unwrap();
    assert_eq!(openrouter["setup"], "api_key");
    assert_eq!(openrouter["secret_slot"], "openrouter");
}

#[tokio::test]
async fn antigravity_auth_status_idle() {
    let (status, json) = request(test_state(), "GET", "/api/auth/antigravity/status", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["status"], "idle");
}

#[tokio::test]
async fn save_opencode_key_enables_provider() {
    let state = test_state();
    let (status, json) = request(
        state.clone(),
        "PUT",
        "/api/secrets/opencode",
        Some(json!({ "key": "sk-oc-test" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["kind"], "opencode");
    let (status, json) = request(state, "GET", "/api/providers", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(json["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == "opencode"));
}

#[tokio::test]
async fn save_openrouter_key_enables_provider() {
    let state = test_state();
    let (status, json) = request(
        state.clone(),
        "PUT",
        "/api/secrets/openrouter",
        Some(json!({ "key": "sk-or-test-key" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["ok"], true);
    let (status, json) = request(state, "GET", "/api/providers", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(json["enabled"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k == "openrouter"));
}

#[tokio::test]
async fn health_returns_ok() {
    let (status, json) = request(test_state(), "GET", "/api/health", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["ok"], true);
}

#[tokio::test]
async fn quotas_returns_empty_when_none_enabled() {
    let (status, json) = request(test_state(), "GET", "/api/quotas", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(json.get("generated_at").is_some());
    assert_eq!(json["providers"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn catalog_lists_seven_providers() {
    let (status, json) = request(test_state(), "GET", "/api/providers/catalog", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 7);
    let kinds: Vec<&str> = arr.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(
        kinds,
        vec![
            "cursor",
            "antigravity",
            "claude-code",
            "codex",
            "opencode",
            "openrouter",
            "t3"
        ]
    );
    assert_eq!(arr[0]["display_name"], "Cursor");
}

#[tokio::test]
async fn add_remove_provider_and_quotas_stub() {
    let state = test_state();

    let (status, json) = request(
        state.clone(),
        "POST",
        "/api/providers",
        Some(json!({ "kind": "opencode" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["kind"], "opencode");

    let (status, json) = request(state.clone(), "GET", "/api/providers", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["enabled"], json!(["opencode"]));

    let (status, json) = request(state.clone(), "GET", "/api/quotas", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let providers = json["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0]["kind"], "opencode");
    // No local OpenCode keys in the test env → honest needs_login (live path, not stub).
    assert_eq!(providers[0]["status"], "needs_login");
    assert!(providers[0]["message"].as_str().is_some());

    let (status, _) = request(state.clone(), "DELETE", "/api/providers/opencode", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let (status, json) = request(state, "GET", "/api/providers", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["enabled"], json!([]));
}

#[tokio::test]
async fn reject_unknown_provider_kind() {
    let (status, json) = request(
        test_state(),
        "POST",
        "/api/providers",
        Some(json!({ "kind": "not-a-provider" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::BAD_REQUEST);
    assert!(json["error"].as_str().unwrap().contains("unknown"));
}

#[tokio::test]
async fn add_provider_persists_across_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quota-dash.toml");
    Config::default().save(&path).unwrap();

    let state = AppState::load(path.clone());
    let (status, _) = request(
        state,
        "POST",
        "/api/providers",
        Some(json!({ "kind": "codex" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);

    let reloaded = Config::load_or_default(&path);
    assert_eq!(reloaded.enabled_providers, vec!["codex".to_string()]);

    let state2 = AppState::load(path);
    let (status, json) = request(state2, "GET", "/api/providers", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["enabled"], json!(["codex"]));
}
