//! Concurrent refresh, per-provider timeout, and last-good cache.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use futures::future::join_all;

use crate::catalog;
use crate::models::{ProviderSnapshot, ProviderStatus, UsageReport};
use crate::providers;
use crate::state::AppState;

#[derive(Debug, Default)]
pub struct QuotaCache {
    /// Last successful (`Ok`) snapshot by provider id.
    pub last_ok: HashMap<String, ProviderSnapshot>,
    /// Last full report + when it was produced (for refresh interval).
    pub last_full: Option<(Instant, UsageReport)>,
}

/// Prefer live ok / needs_login; on error reuse recent last-good with a Cached hint.
///
/// An expired Antigravity token cannot be re-read until the next turn renews it.
/// The last Google reading stays, marked stale, instead of disappearing.
pub fn resolve_after_fetch(
    live: ProviderSnapshot,
    last_ok: Option<&ProviderSnapshot>,
    now: DateTime<Utc>,
    cache_ttl: Duration,
) -> ProviderSnapshot {
    match live.status {
        ProviderStatus::NeedsLogin
            if live.kind == "antigravity"
                && last_ok.is_some_and(|prev| {
                    prev.status == ProviderStatus::Ok && !prev.windows.is_empty()
                }) =>
        {
            stale_antigravity(last_ok.expect("guard checked"))
        }
        ProviderStatus::Ok
        | ProviderStatus::NeedsLogin
        | ProviderStatus::Stale
        | ProviderStatus::Disabled => live,
        ProviderStatus::Error => {
            if let Some(prev) = last_ok {
                if prev.status == ProviderStatus::Ok
                    && now
                        .signed_duration_since(prev.fetched_at)
                        .to_std()
                        .ok()
                        .is_some_and(|d| d <= cache_ttl)
                {
                    return cached_from(prev);
                }
            }
            live
        }
    }
}

fn stale_antigravity(prev: &ProviderSnapshot) -> ProviderSnapshot {
    let mut snap = prev.clone();
    snap.status = ProviderStatus::Stale;
    let local = prev.fetched_at.with_timezone(&chrono::Local);
    let time = local.format("%H:%M").to_string();
    let reset_passed = {
        let resets: Vec<_> = prev.windows.iter().filter_map(|w| w.resets_at).collect();
        !resets.is_empty() && resets.iter().all(|t| *t <= Utc::now())
    };
    snap.message = Some(if reset_passed {
        format!(
            "Last reading at {time} is out of date: its reset time has passed. The next Antigravity turn on this account reads it again."
        )
    } else {
        format!(
            "Last reading at {time}. Not the current quota. The next Antigravity turn on this account refreshes it."
        )
    });
    snap
}

fn cached_from(prev: &ProviderSnapshot) -> ProviderSnapshot {
    let mut s = prev.clone();
    let local = prev.fetched_at.with_timezone(&chrono::Local);
    let hint = format!("Cached from {}", local.format("%H:%M"));
    s.message = Some(match s.message {
        Some(m) if !m.is_empty() => format!("{m} ({hint})"),
        _ => hint,
    });
    s
}

fn timeout_snapshot(kind: &str) -> ProviderSnapshot {
    let name = catalog::display_name(kind).unwrap_or(kind);
    ProviderSnapshot {
        id: kind.to_string(),
        kind: kind.to_string(),
        display_name: name.to_string(),
        account: None,
        plan: None,
        status: ProviderStatus::Error,
        message: Some("Provider fetch timed out".into()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

/// Collect quotas for all enabled providers.
///
/// Within `refresh_interval_secs`, returns the previous report without re-fetching.
/// Otherwise fetches all enabled providers concurrently with per-provider timeouts.
pub async fn collect_report(state: &AppState) -> UsageReport {
    let (enabled, refresh_secs, timeout_secs, cache_ttl_secs) = {
        let cfg = state.config.lock().expect("config lock");
        (
            cfg.enabled_providers.clone(),
            cfg.refresh_interval_secs,
            cfg.provider_timeout_secs,
            cfg.cache_ttl_secs,
        )
    };

    {
        let cache = state.cache.lock().expect("cache lock");
        if let Some((at, report)) = &cache.last_full {
            if at.elapsed() < Duration::from_secs(refresh_secs) {
                return report.clone();
            }
        }
    }

    let timeout = Duration::from_secs(timeout_secs.max(1));
    let cache_ttl = Duration::from_secs(cache_ttl_secs.max(1));

    let futs = enabled.into_iter().map(|kind| {
        let state = state.clone();
        async move {
            match tokio::time::timeout(timeout, providers::fetch_kind(&kind, &state)).await {
                Ok(snaps) if !snaps.is_empty() => snaps,
                Ok(_) => Vec::new(),
                Err(_) => vec![timeout_snapshot(&kind)],
            }
        }
    });

    let raw: Vec<ProviderSnapshot> = join_all(futs).await.into_iter().flatten().collect();
    let now = Utc::now();

    let mut providers_out = Vec::with_capacity(raw.len());
    let mut cache = state.cache.lock().expect("cache lock");
    for live in raw {
        let id = live.id.clone();
        let prev = cache.last_ok.get(&id).cloned();
        let resolved = resolve_after_fetch(live.clone(), prev.as_ref(), now, cache_ttl);
        if live.status == ProviderStatus::Ok {
            cache.last_ok.insert(id, live);
        }
        providers_out.push(resolved);
    }
    store_antigravity_last(&cache);

    let report = UsageReport {
        generated_at: now,
        providers: providers_out,
    };
    cache.last_full = Some((Instant::now(), report.clone()));
    report
}

/// Collect a single enabled kind (always fetches; still applies timeout + last-good).
pub async fn collect_one(state: &AppState, kind: &str) -> Option<ProviderSnapshot> {
    let (timeout_secs, cache_ttl_secs) = {
        let cfg = state.config.lock().expect("config lock");
        (cfg.provider_timeout_secs, cfg.cache_ttl_secs)
    };
    let timeout = Duration::from_secs(timeout_secs.max(1));
    let cache_ttl = Duration::from_secs(cache_ttl_secs.max(1));

    let lives = match tokio::time::timeout(timeout, providers::fetch_kind(kind, state)).await {
        Ok(snaps) if !snaps.is_empty() => snaps,
        Ok(_) => return None,
        Err(_) => vec![timeout_snapshot(kind)],
    };

    let now = Utc::now();
    let mut cache = state.cache.lock().expect("cache lock");
    let mut last = None;
    for live in lives {
        let prev = cache.last_ok.get(&live.id).cloned();
        let resolved = resolve_after_fetch(live.clone(), prev.as_ref(), now, cache_ttl);
        if live.status == ProviderStatus::Ok {
            cache.last_ok.insert(live.id.clone(), live);
        }
        last = Some(resolved);
    }
    store_antigravity_last(&cache);
    // Invalidate full-report cache so next /api/quotas refreshes.
    cache.last_full = None;
    last
}

fn antigravity_last_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("HOME"))?;
    Some(
        std::path::PathBuf::from(base)
            .join("quota-dash")
            .join("antigravity-last.json"),
    )
}

/// Last successful Antigravity readings, so an expired token still has a figure to show.
pub fn load_antigravity_last(cache: &mut QuotaCache) {
    let Some(path) = antigravity_last_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(snaps) = serde_json::from_str::<Vec<ProviderSnapshot>>(&text) else {
        return;
    };
    for snap in snaps {
        if snap.kind == "antigravity"
            && snap.status == ProviderStatus::Ok
            && !snap.windows.is_empty()
        {
            cache.last_ok.entry(snap.id.clone()).or_insert(snap);
        }
    }
}

fn store_antigravity_last(cache: &QuotaCache) {
    let Some(path) = antigravity_last_path() else {
        return;
    };
    let snaps: Vec<_> = cache
        .last_ok
        .values()
        .filter(|snap| snap.kind == "antigravity" && snap.status == ProviderStatus::Ok)
        .cloned()
        .collect();
    if snaps.is_empty() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string(&snaps) {
        let _ = std::fs::write(path, body);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageWindow;

    fn ok_snap(id: &str, fetched_at: DateTime<Utc>) -> ProviderSnapshot {
        ProviderSnapshot {
            id: id.into(),
            kind: id.into(),
            display_name: id.into(),
            account: None,
            plan: None,
            status: ProviderStatus::Ok,
            message: None,
            windows: vec![UsageWindow {
                key: "primary".into(),
                label: "Primary".into(),
                used_percent: 10.0,
                resets_at: None,
                detail: None,
            }],
            fetched_at,
        }
    }

    fn err_snap(id: &str) -> ProviderSnapshot {
        ProviderSnapshot {
            id: id.into(),
            kind: id.into(),
            display_name: id.into(),
            account: None,
            plan: None,
            status: ProviderStatus::Error,
            message: Some("boom".into()),
            windows: Vec::new(),
            fetched_at: Utc::now(),
        }
    }

    #[test]
    fn cache_fallback_on_error_within_ttl() {
        let now = Utc::now();
        let prev = ok_snap("cursor", now - chrono::Duration::minutes(5));
        let live = err_snap("cursor");
        let out = resolve_after_fetch(live, Some(&prev), now, Duration::from_secs(1800));
        assert_eq!(out.status, ProviderStatus::Ok);
        assert!(out.message.as_ref().unwrap().contains("Cached from"));
        assert_eq!(out.windows.len(), 1);
    }

    #[test]
    fn no_cache_when_ttl_expired() {
        let now = Utc::now();
        let prev = ok_snap("cursor", now - chrono::Duration::hours(2));
        let live = err_snap("cursor");
        let out = resolve_after_fetch(live.clone(), Some(&prev), now, Duration::from_secs(1800));
        assert_eq!(out.status, ProviderStatus::Error);
        assert_eq!(out.message.as_deref(), Some("boom"));
    }

    #[test]
    fn antigravity_expired_token_keeps_last_reading() {
        let now = Utc::now();
        let mut prev = ok_snap("antigravity:a@b.c", now - chrono::Duration::minutes(40));
        prev.kind = "antigravity".into();
        let mut live = err_snap("antigravity:a@b.c");
        live.kind = "antigravity".into();
        live.status = ProviderStatus::NeedsLogin;
        live.message = Some("Antigravity access token expired.".into());
        let out = resolve_after_fetch(live, Some(&prev), now, Duration::from_secs(1800));
        assert_eq!(out.status, ProviderStatus::Stale);
        assert_eq!(out.windows.len(), 1);
        assert_eq!(out.fetched_at, prev.fetched_at);
        assert!(out.message.unwrap().contains("Not the current quota"));
    }

    #[test]
    fn needs_login_not_replaced_by_cache() {
        let now = Utc::now();
        let prev = ok_snap("cursor", now);
        let mut live = err_snap("cursor");
        live.status = ProviderStatus::NeedsLogin;
        live.message = Some("sign in".into());
        let out = resolve_after_fetch(live, Some(&prev), now, Duration::from_secs(1800));
        assert_eq!(out.status, ProviderStatus::NeedsLogin);
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_returns_error_snapshot() {
        let slow = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Some(ok_snap("x", Utc::now()))
        };
        let result = tokio::time::timeout(Duration::from_secs(1), slow).await;
        assert!(result.is_err());
        let snap = timeout_snapshot("codex");
        assert_eq!(snap.status, ProviderStatus::Error);
        assert!(snap.message.unwrap().contains("timed out"));
    }
}
