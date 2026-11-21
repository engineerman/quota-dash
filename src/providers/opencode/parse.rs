use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

use super::keys::OpenCodeKey;

const WINDOWS: &[(&str, &str)] = &[
    ("rolling", "Session"),
    ("weekly", "Weekly"),
    ("monthly", "Monthly"),
];

/// Maps OpenCode Go `/zen/go/v1/usage` JSON into a snapshot.
pub fn parse_usage(u: &Value, key: &OpenCodeKey) -> ProviderSnapshot {
    let usage = u.get("usage").unwrap_or(u);
    let mut windows = Vec::new();
    let mut message: Option<String> = None;

    for &(wkey, label) in WINDOWS {
        let Some(w) = usage.get(wkey) else { continue };
        let used = num(w, "percent").unwrap_or(0.0).clamp(0.0, 100.0);
        let resets_at = parse_resets_at(w.get("resetsAt"));
        if let Some(st) = w
            .get("status")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty() && *s != "ok")
        {
            message = Some(format!("{label}: {st}"));
        }
        windows.push(UsageWindow {
            key: wkey.to_string(),
            label: label.to_string(),
            used_percent: used,
            resets_at,
            detail: Some(format!("{used:.1}% used")),
        });
    }

    if windows.is_empty() {
        return fail(
            ProviderStatus::Error,
            "Usage response had no rolling/weekly/monthly windows.",
            Some(key),
        );
    }

    ProviderSnapshot {
        id: format!("opencode:{}", key.id),
        kind: "opencode".into(),
        display_name: "OpenCode".into(),
        account: Some(key.label.clone()),
        plan: Some("Go".into()),
        status: ProviderStatus::Ok,
        message,
        windows,
        fetched_at: Utc::now(),
    }
}

pub fn fail(status: ProviderStatus, message: &str, key: Option<&OpenCodeKey>) -> ProviderSnapshot {
    ProviderSnapshot {
        id: key
            .map(|k| format!("opencode:{}", k.id))
            .unwrap_or_else(|| "opencode".into()),
        kind: "opencode".into(),
        display_name: "OpenCode".into(),
        account: key.map(|k| k.label.clone()),
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

fn num(obj: &Value, key: &str) -> Option<f64> {
    obj.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_i64().map(|i| i as f64))
            .or_else(|| v.as_u64().map(|i| i as f64))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn parse_resets_at(v: Option<&Value>) -> Option<DateTime<Utc>> {
    let v = v?;
    if let Some(s) = v.as_str() {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ") {
            return Some(DateTime::from_naive_utc_and_offset(dt, Utc));
        }
    }
    if let Some(n) = v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)) {
        if n > 1_000_000_000_000 {
            return DateTime::from_timestamp_millis(n);
        }
        if n > 0 {
            return DateTime::from_timestamp(n, 0);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_key() -> OpenCodeKey {
        OpenCodeKey {
            id: "k1".into(),
            label: "Go Main".into(),
            key: "sk-test".into(),
            active: true,
        }
    }

    #[test]
    fn maps_rolling_weekly_monthly() {
        let sample = json!({
            "usage": {
                "rolling": {
                    "percent": 12.5,
                    "resetsAt": "2026-12-01T12:00:00Z",
                    "status": "ok"
                },
                "weekly": {
                    "percent": 30.0,
                    "resetsAt": "2026-12-07T00:00:00Z"
                },
                "monthly": {
                    "percent": 45.25,
                    "resetsAt": "2027-01-01T00:00:00Z"
                }
            }
        });
        let snap = parse_usage(&sample, &sample_key());
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.kind, "opencode");
        assert_eq!(snap.plan.as_deref(), Some("Go"));
        assert_eq!(snap.account.as_deref(), Some("Go Main"));
        assert_eq!(snap.windows.len(), 3);
        let rolling = snap.windows.iter().find(|w| w.key == "rolling").unwrap();
        assert!((rolling.used_percent - 12.5).abs() < 0.001);
        assert_eq!(rolling.label, "Session");
        assert!(rolling.resets_at.is_some());
        let monthly = snap.windows.iter().find(|w| w.key == "monthly").unwrap();
        assert!((monthly.used_percent - 45.25).abs() < 0.001);
    }

    #[test]
    fn non_ok_window_status_sets_message() {
        let sample = json!({
            "usage": {
                "rolling": { "percent": 100.0, "status": "exhausted" }
            }
        });
        let snap = parse_usage(&sample, &sample_key());
        assert_eq!(snap.message.as_deref(), Some("Session: exhausted"));
    }

    #[test]
    fn empty_usage_errors() {
        let sample = json!({ "usage": {} });
        let snap = parse_usage(&sample, &sample_key());
        assert_eq!(snap.status, ProviderStatus::Error);
    }
}
