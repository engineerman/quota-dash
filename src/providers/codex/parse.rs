use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

/// Maps ChatGPT/Codex `wham/usage` JSON into a snapshot.
pub fn parse_usage(u: &Value) -> ProviderSnapshot {
    let plan = u
        .get("plan_type")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(title_case);
    let account = u
        .get("email")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string());

    let mut windows = Vec::new();
    if let Some(rl) = u.get("rate_limit") {
        add_windows(&mut windows, rl, "", "");
    }

    if let Some(extra) = u.get("additional_rate_limits").and_then(|v| v.as_array()) {
        for item in extra {
            let name = item
                .get("limit_name")
                .or_else(|| item.get("name"))
                .or_else(|| item.get("metered_feature"))
                .and_then(|v| v.as_str())
                .unwrap_or("Extra");
            let key_prefix = format!("{}:", name.to_lowercase().replace(' ', "_"));
            let label_prefix = format!("{name} ");
            let inner = item.get("rate_limit").unwrap_or(item);
            add_windows(&mut windows, inner, &label_prefix, &key_prefix);
        }
    }

    if windows.is_empty() {
        return fail(
            ProviderStatus::Error,
            "Usage response had no rate-limit windows.",
        );
    }

    let message = if u
        .pointer("/rate_limit/limit_reached")
        .and_then(|v| v.as_bool())
        == Some(true)
    {
        Some("Limit reached".to_string())
    } else {
        None
    };

    ProviderSnapshot {
        id: "codex".into(),
        kind: "codex".into(),
        display_name: "Codex".into(),
        account,
        plan,
        status: ProviderStatus::Ok,
        message,
        windows,
        fetched_at: Utc::now(),
    }
}

fn add_windows(windows: &mut Vec<UsageWindow>, rl: &Value, label_prefix: &str, key_prefix: &str) {
    for (prop, fallback) in [
        ("primary_window", "Session"),
        ("secondary_window", "Weekly"),
    ] {
        let Some(w) = rl.get(prop) else { continue };
        let len = num(w, "limit_window_seconds").map(|n| n as i64);
        let resets_at = reset_at(w);
        let used = num(w, "used_percent").unwrap_or(0.0).clamp(0.0, 100.0);
        let key_suffix = if prop == "primary_window" {
            "primary"
        } else {
            "secondary"
        };
        let label = format!("{}{}", label_prefix, window_name(len, fallback))
            .trim()
            .to_string();
        windows.push(UsageWindow {
            key: format!("{key_prefix}{key_suffix}"),
            label,
            used_percent: used,
            resets_at,
            detail: Some(format!("{used:.1}% used")),
        });
    }
}

fn reset_at(w: &Value) -> Option<DateTime<Utc>> {
    if let Some(ra) = num(w, "reset_at") {
        if ra > 0.0 {
            return Utc.timestamp_opt(ra as i64, 0).single();
        }
    }
    if let Some(rs) = num(w, "reset_after_seconds") {
        if rs > 0.0 {
            return Some(Utc::now() + chrono::Duration::seconds(rs as i64));
        }
    }
    None
}

fn window_name(seconds: Option<i64>, fallback: &str) -> String {
    match seconds {
        Some(18_000) => "Session".into(),
        Some(604_800) => "Weekly".into(),
        Some(86_400) => "Daily".into(),
        Some(x) if x > 0 && x % 3600 == 0 && x < 86_400 => format!("{}-hour", x / 3600),
        _ => fallback.to_string(),
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
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

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "codex".into(),
        kind: "codex".into(),
        display_name: "Codex".into(),
        account: None,
        plan: None,
        status,
        message: Some(message.to_string()),
        windows: Vec::new(),
        fetched_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_primary_and_secondary_windows() {
        let sample = json!({
            "email": "you@example.com",
            "plan_type": "plus",
            "rate_limit": {
                "limit_reached": false,
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
        });
        let snap = parse_usage(&sample);
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.kind, "codex");
        assert_eq!(snap.account.as_deref(), Some("you@example.com"));
        assert_eq!(snap.plan.as_deref(), Some("Plus"));
        let primary = snap.windows.iter().find(|w| w.key == "primary").unwrap();
        assert!((primary.used_percent - 42.5).abs() < 0.001);
        assert_eq!(primary.label, "Session");
        assert!(primary.resets_at.is_some());
        let weekly = snap.windows.iter().find(|w| w.key == "secondary").unwrap();
        assert_eq!(weekly.label, "Weekly");
        assert!((weekly.used_percent - 10.0).abs() < 0.001);
    }

    #[test]
    fn empty_rate_limit_errors() {
        let sample = json!({"email": "a@b.com", "plan_type": "plus"});
        let snap = parse_usage(&sample);
        assert_eq!(snap.status, ProviderStatus::Error);
        assert!(snap.message.unwrap().to_lowercase().contains("window"));
    }

    #[test]
    fn limit_reached_sets_message() {
        let sample = json!({
            "rate_limit": {
                "limit_reached": true,
                "primary_window": { "used_percent": 100.0, "limit_window_seconds": 18000 }
            }
        });
        let snap = parse_usage(&sample);
        assert_eq!(snap.message.as_deref(), Some("Limit reached"));
    }
}
