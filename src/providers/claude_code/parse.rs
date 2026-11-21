use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

const KNOWN: &[(&str, &str)] = &[
    ("five_hour", "Session"),
    ("seven_day", "Weekly"),
    ("seven_day_opus", "Weekly · Opus"),
    ("seven_day_sonnet", "Weekly · Sonnet"),
    ("seven_day_oauth_apps", "Weekly · Apps"),
];

/// Maps Anthropic OAuth usage utilization windows into a snapshot.
pub fn parse_usage(u: &Value, plan: Option<&str>) -> ProviderSnapshot {
    let mut windows = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for &(key, label) in KNOWN {
        if let Some(w) = window_from(u.get(key), key, label) {
            seen.insert(key.to_string());
            windows.push(w);
        }
    }

    if let Some(obj) = u.as_object() {
        for (name, val) in obj {
            if name == "extra_usage" || seen.contains(name) {
                continue;
            }
            if !val.is_object() || utilization(val).is_none() {
                continue;
            }
            let label = title_case_key(name);
            if let Some(w) = window_from(Some(val), name, &label) {
                windows.push(w);
            }
        }
    }

    let mut message = None;
    if let Some(ex) = u.get("extra_usage") {
        if ex.get("is_enabled").and_then(|v| v.as_bool()) == Some(true) {
            let used = num(ex, "used_credits").unwrap_or(0.0) / 100.0;
            let limit = num(ex, "monthly_limit").unwrap_or(0.0) / 100.0;
            message = Some(if limit > 0.0 {
                format!("Extra usage: ${used:.2} of ${limit:.0}")
            } else {
                format!("Extra usage: ${used:.2}")
            });
        }
    }

    if windows.is_empty() {
        return fail(
            ProviderStatus::Error,
            "Usage response had no utilization windows.",
        );
    }

    ProviderSnapshot {
        id: "claude-code".into(),
        kind: "claude-code".into(),
        display_name: "Claude Code".into(),
        account: None,
        plan: plan.map(|s| s.to_string()),
        status: ProviderStatus::Ok,
        message,
        windows,
        fetched_at: Utc::now(),
    }
}

fn window_from(val: Option<&Value>, key: &str, label: &str) -> Option<UsageWindow> {
    let val = val?;
    let used = utilization(val)?;
    Some(UsageWindow {
        key: key.to_string(),
        label: label.to_string(),
        used_percent: used.clamp(0.0, 100.0),
        resets_at: parse_resets_at(val.get("resets_at")),
        detail: Some(format!("{:.1}% used", used.clamp(0.0, 100.0))),
    })
}

fn utilization(val: &Value) -> Option<f64> {
    num(val, "utilization")
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
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
    }
    // Some payloads may use epoch ms/seconds.
    let n = v
        .as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_u64().map(|i| i as f64))?;
    if n <= 0.0 {
        return None;
    }
    let ms = if n > 1e12 {
        n as i64
    } else {
        (n * 1000.0) as i64
    };
    DateTime::from_timestamp_millis(ms)
}

fn title_case_key(name: &str) -> String {
    name.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn plan_name(tier: Option<&str>, fallback: Option<&str>) -> Option<String> {
    let t = format!("{} {}", tier.unwrap_or(""), fallback.unwrap_or("")).to_lowercase();
    if t.contains("max_20x") {
        return Some("Max 20x".into());
    }
    if t.contains("max_5x") {
        return Some("Max 5x".into());
    }
    if t.contains("max") {
        return Some("Max".into());
    }
    if t.contains("pro") {
        return Some("Pro".into());
    }
    if t.contains("team") {
        return Some("Team".into());
    }
    if t.contains("enterprise") {
        return Some("Enterprise".into());
    }
    if t.trim().is_empty() {
        None
    } else {
        Some("Free".into())
    }
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "claude-code".into(),
        kind: "claude-code".into(),
        display_name: "Claude Code".into(),
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
    fn maps_known_utilization_windows() {
        let sample = json!({
            "five_hour": { "utilization": 34.5, "resets_at": "2026-11-21T20:00:00Z" },
            "seven_day": { "utilization": 58.0, "resets_at": "2026-11-28T00:00:00Z" },
            "seven_day_opus": { "utilization": 10.0, "resets_at": "2026-11-28T00:00:00Z" },
            "extra_usage": { "is_enabled": true, "used_credits": 500, "monthly_limit": 10000 }
        });
        let snap = parse_usage(&sample, Some("Max 5x"));
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.kind, "claude-code");
        assert_eq!(snap.display_name, "Claude Code");
        assert_eq!(snap.plan.as_deref(), Some("Max 5x"));
        let session = snap.windows.iter().find(|w| w.key == "five_hour").unwrap();
        assert!((session.used_percent - 34.5).abs() < 0.001);
        assert!(session.resets_at.is_some());
        assert!(snap.message.as_ref().unwrap().contains("Extra usage"));
        assert!(snap.windows.iter().any(|w| w.key == "seven_day_opus"));
    }

    #[test]
    fn plan_name_from_tier() {
        assert_eq!(
            plan_name(Some("default_claude_max_5x"), Some("max")).as_deref(),
            Some("Max 5x")
        );
        assert_eq!(plan_name(Some("pro"), None).as_deref(), Some("Pro"));
    }
}
