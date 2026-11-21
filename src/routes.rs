use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::catalog;
use crate::i18n::Lang;
use crate::models::UsageReport;
use crate::providers::antigravity::{self, login};
use crate::refresh;
use crate::state::AppState;

const INDEX_HTML: &str = include_str!("../static/index.html");
const I18N_BOOT_JS: &str = include_str!("../static/i18n-boot.js");
const UI_FALLBACK_EN_JS: &str = include_str!("../static/ui-fallback-en.js");
const ICON_CURSOR: &str = include_str!("../static/icons/cursor.svg");
const ICON_ANTIGRAVITY: &str = include_str!("../static/icons/antigravity.svg");
const ICON_CLAUDE: &str = include_str!("../static/icons/claude-code.svg");
const ICON_CODEX: &str = include_str!("../static/icons/codex.svg");
const ICON_OPENCODE: &str = include_str!("../static/icons/opencode.svg");
const ICON_OPENROUTER: &str = include_str!("../static/icons/openrouter.svg");
const ICON_T3: &str = include_str!("../static/icons/t3.svg");

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/", get(ui_index))
        .route("/index.html", get(ui_index))
        .route("/static/i18n-boot.js", get(ui_i18n_boot))
        .route("/static/ui-fallback-en.js", get(ui_fallback_en))
        .route("/icons/{name}", get(ui_icon))
        .route("/api/health", get(health))
        .route("/api/quotas", get(quotas))
        .route("/api/quotas/{kind}", get(quotas_one))
        .route("/api/providers/catalog", get(providers_catalog))
        .route("/api/locales/{lang}", get(locale_bundle))
        .route("/api/system-locale", get(system_locale))
        .route("/api/settings", get(settings_get).put(settings_put))
        .route("/api/providers", get(providers_list).post(providers_add))
        .route("/api/providers/{kind}", delete(providers_remove))
        .route("/api/secrets/{slot}", put(put_secret))
        .route("/api/auth/antigravity/start", post(antigravity_login_start))
        .route("/api/auth/antigravity/code", post(antigravity_login_code))
        .route(
            "/api/auth/antigravity/cancel",
            post(antigravity_login_cancel),
        )
        .route(
            "/api/auth/antigravity/status",
            get(antigravity_login_status),
        )
        .route(
            "/api/providers/antigravity/accounts",
            get(antigravity_accounts_list),
        )
        .route(
            "/api/providers/antigravity/accounts/switch",
            post(antigravity_accounts_switch),
        )
        .route(
            "/api/providers/antigravity/accounts/{email}",
            delete(antigravity_accounts_remove),
        )
        .with_state(state)
}

async fn ui_index() -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")], Html(INDEX_HTML))
}

async fn ui_i18n_boot() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        I18N_BOOT_JS,
    )
}

async fn ui_fallback_en() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        UI_FALLBACK_EN_JS,
    )
}

async fn ui_icon(Path(name): Path<String>) -> Result<impl IntoResponse, StatusCode> {
    let body = match name.as_str() {
        "cursor.svg" => ICON_CURSOR,
        "antigravity.svg" => ICON_ANTIGRAVITY,
        "claude-code.svg" => ICON_CLAUDE,
        "codex.svg" => ICON_CODEX,
        "opencode.svg" => ICON_OPENCODE,
        "openrouter.svg" => ICON_OPENROUTER,
        "t3.svg" => ICON_T3,
        _ => return Err(StatusCode::NOT_FOUND),
    };
    Ok((
        [
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    ))
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}

async fn quotas(State(state): State<AppState>) -> Json<UsageReport> {
    Json(refresh::collect_report(&state).await)
}

async fn quotas_one(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<Json<UsageReport>, (StatusCode, Json<Value>)> {
    let Some(canonical) = catalog::canonical_kind(&kind) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown provider kind: {kind}") })),
        ));
    };
    let enabled = {
        let cfg = state.config.lock().expect("config lock");
        cfg.enabled_providers
            .iter()
            .any(|k| k.eq_ignore_ascii_case(canonical))
    };
    if !enabled {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("provider not enabled: {canonical}") })),
        ));
    }
    // Full report then filter — preserves multi-account kinds (Antigravity).
    let report = refresh::collect_report(&state).await;
    let providers: Vec<_> = report
        .providers
        .into_iter()
        .filter(|p| p.kind.eq_ignore_ascii_case(canonical))
        .collect();
    if providers.is_empty() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "fetch failed" })),
        ));
    }
    Ok(Json(UsageReport {
        generated_at: Utc::now(),
        providers,
    }))
}

#[derive(Debug, Deserialize)]
struct LangQuery {
    #[serde(default)]
    lang: Option<String>,
}

async fn providers_catalog(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<LangQuery>,
) -> Json<Value> {
    let lang = Lang::resolve(&headers, query.lang.as_deref());
    Json(json!(catalog::catalog_for_lang(lang)))
}

async fn locale_bundle(Path(tag): Path<String>) -> Result<Json<Value>, StatusCode> {
    if !crate::i18n::is_known_locale(&tag) {
        return Err(StatusCode::NOT_FOUND);
    }
    let lang = Lang::parse(&tag);
    Ok(Json(json!({
        "lang": lang.as_tag(),
        "dir": crate::i18n::text_dir(lang),
        "ui": crate::i18n::ui_bundle(lang),
    })))
}

async fn system_locale() -> Json<Value> {
    let lang = crate::i18n::system_lang();
    Json(json!({
        "lang": lang.as_tag(),
        "dir": crate::i18n::text_dir(lang),
    }))
}

async fn settings_get(State(state): State<AppState>) -> Json<Value> {
    let cfg = state.config.lock().expect("config lock");
    Json(json!({
        "languages": crate::i18n::available_languages(),
        "config": {
            "refresh_interval_secs": cfg.refresh_interval_secs,
            "provider_timeout_secs": cfg.provider_timeout_secs,
            "cache_ttl_secs": cfg.cache_ttl_secs,
        },
        "server": {
            "bind": cfg.bind.clone(),
            "port": cfg.port,
            "config_path": state.config_path.to_string_lossy(),
        },
    }))
}

#[derive(Debug, Deserialize)]
struct SettingsUpdate {
    #[serde(default)]
    refresh_interval_secs: Option<u64>,
    #[serde(default)]
    provider_timeout_secs: Option<u64>,
    #[serde(default)]
    cache_ttl_secs: Option<u64>,
}

async fn settings_put(
    State(state): State<AppState>,
    Json(body): Json<SettingsUpdate>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    {
        let mut cfg = state.config.lock().expect("config lock");
        if let Some(v) = body.refresh_interval_secs {
            if v > 0 {
                cfg.refresh_interval_secs = v;
            }
        }
        if let Some(v) = body.provider_timeout_secs {
            if v > 0 {
                cfg.provider_timeout_secs = v;
            }
        }
        if let Some(v) = body.cache_ttl_secs {
            if v > 0 {
                cfg.cache_ttl_secs = v;
            }
        }
    }
    state.bump_cache();
    state.persist().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
    })?;
    Ok(Json(json!({ "ok": true })))
}

async fn providers_list(State(state): State<AppState>) -> Json<Value> {
    let enabled = state
        .config
        .lock()
        .expect("config lock")
        .enabled_providers
        .clone();
    Json(json!({ "enabled": enabled }))
}

#[derive(Debug, Deserialize)]
struct AddProviderBody {
    kind: String,
}

async fn providers_add(
    State(state): State<AppState>,
    Json(body): Json<AddProviderBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let Some(canonical) = catalog::canonical_kind(&body.kind) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown provider kind: {}", body.kind) })),
        ));
    };

    {
        let mut cfg = state.config.lock().expect("config lock");
        if !cfg
            .enabled_providers
            .iter()
            .any(|k| k.eq_ignore_ascii_case(canonical))
        {
            cfg.enabled_providers.push(canonical.to_string());
        }
    }

    state.persist().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
    })?;

    Ok((
        StatusCode::OK,
        Json(json!({ "enabled": true, "kind": canonical })),
    ))
}

async fn providers_remove(
    State(state): State<AppState>,
    Path(kind): Path<String>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let Some(canonical) = catalog::canonical_kind(&kind) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown provider kind: {kind}") })),
        ));
    };

    {
        let mut cfg = state.config.lock().expect("config lock");
        cfg.enabled_providers
            .retain(|k| !k.eq_ignore_ascii_case(canonical));
    }

    state.persist().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
    })?;

    Ok((
        StatusCode::OK,
        Json(json!({ "enabled": false, "kind": canonical })),
    ))
}

#[derive(Debug, Deserialize)]
struct SecretBody {
    key: String,
}

async fn put_secret(
    State(state): State<AppState>,
    Path(slot): Path<String>,
    Json(body): Json<SecretBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let key = body.key.trim().to_string();
    if key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "key is empty" })),
        ));
    }

    let kind = match slot.as_str() {
        "openrouter" => "openrouter",
        "opencode" => "opencode",
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("unknown secret slot: {other}") })),
            ));
        }
    };

    {
        let mut cfg = state.config.lock().expect("config lock");
        match kind {
            "openrouter" => cfg.openrouter_api_key = key,
            "opencode" => cfg.opencode_api_key = key,
            _ => unreachable!(),
        }
        if !cfg
            .enabled_providers
            .iter()
            .any(|k| k.eq_ignore_ascii_case(kind))
        {
            cfg.enabled_providers.push(kind.into());
        }
    }
    state.persist().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
    })?;
    state.bump_cache();
    Ok((
        StatusCode::OK,
        Json(json!({ "ok": true, "kind": kind, "enabled": true })),
    ))
}

async fn antigravity_login_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    {
        let mut slot = state.antigravity_login.lock().await;
        if let Some(mut old) = slot.take() {
            login::cancel(&mut old).await;
        }
    }

    // Vault current WinCred account, then clear so a new Google login can be added.
    let previous = antigravity::prepare_add_account();

    let lang = Lang::from_headers(&headers);
    match login::start(lang).await {
        Ok(pending) => {
            let url = pending.auth_url.clone();
            *state.antigravity_login.lock().await = Some(pending);
            enable_provider(&state, "antigravity")?;
            Ok((
                StatusCode::OK,
                Json(json!({
                    "status": "awaiting_code",
                    "auth_url": url,
                    "agy_found": true,
                    "previous": previous,
                })),
            ))
        }
        Err(e) => Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": e.message(lang),
                "agy_found": login::find_agy().is_some()
            })),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct AuthCodeBody {
    code: String,
}

async fn antigravity_login_code(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AuthCodeBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let lang = Lang::from_headers(&headers);
    let mut slot = state.antigravity_login.lock().await;
    let Some(mut pending) = slot.take() else {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({
                "error": crate::i18n::error(lang, "antigravity.no_active_session"),
                "status": "idle"
            })),
        ));
    };

    match login::submit_code(&mut pending, &body.code).await {
        Ok(auth) => {
            let tracked = antigravity::track_active();
            enable_provider(&state, "antigravity")?;
            state.bump_cache();
            Ok((
                StatusCode::OK,
                Json(json!({
                    "status": "ok",
                    "kind": "antigravity",
                    "email": tracked.or(auth.email),
                    "enabled": true,
                    "accounts": antigravity::list_accounts(),
                })),
            ))
        }
        Err(e) => {
            // Keep idle; user can start again.
            Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": e.message(lang), "status": "idle" })),
            ))
        }
    }
}

async fn antigravity_login_cancel(State(state): State<AppState>) -> Json<Value> {
    let mut slot = state.antigravity_login.lock().await;
    if let Some(mut pending) = slot.take() {
        login::cancel(&mut pending).await;
    }
    Json(json!({ "status": "idle" }))
}

async fn antigravity_login_status(State(state): State<AppState>) -> Json<Value> {
    let slot = state.antigravity_login.lock().await;
    if let Some(p) = slot.as_ref() {
        Json(json!({
            "status": "awaiting_code",
            "auth_url": p.auth_url,
            "agy_found": login::find_agy().is_some()
        }))
    } else {
        Json(json!({
            "status": "idle",
            "agy_found": login::find_agy().is_some()
        }))
    }
}

async fn antigravity_accounts_list() -> Json<Value> {
    Json(json!({ "accounts": antigravity::list_accounts() }))
}

#[derive(Debug, Deserialize)]
struct SwitchBody {
    email: String,
}

async fn antigravity_accounts_switch(
    State(state): State<AppState>,
    Json(body): Json<SwitchBody>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    antigravity::switch(&body.email)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))))?;
    state.bump_cache();
    Ok((
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "active": body.email,
            "accounts": antigravity::list_accounts()
        })),
    ))
}

async fn antigravity_accounts_remove(
    State(state): State<AppState>,
    Path(email): Path<String>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    let email = urlencoding_decode(&email);
    antigravity::remove_account(&email)
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))))?;
    state.bump_cache();
    Ok((
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "accounts": antigravity::list_accounts()
        })),
    ))
}

fn urlencoding_decode(s: &str) -> String {
    // Path extractor already percent-decodes once; keep + as space only if present.
    s.replace('+', " ")
}

fn enable_provider(state: &AppState, kind: &str) -> Result<(), (StatusCode, Json<Value>)> {
    {
        let mut cfg = state.config.lock().expect("config lock");
        if !cfg
            .enabled_providers
            .iter()
            .any(|k| k.eq_ignore_ascii_case(kind))
        {
            cfg.enabled_providers.push(kind.into());
        }
    }
    state.persist().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        )
    })
}
