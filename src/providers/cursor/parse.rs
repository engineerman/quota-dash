use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

/// Maps dashboard percentage pools (not dollar totalSpend vs limit).
pub fn parse_usage(
    u: &Value,
    email: Option<&str>,
    plan: Option<&str>,
    plan_name: Option<&str>,
) -> ProviderSnapshot {
    let display_plan = plan_name
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            plan.filter(|s| !s.trim().is_empty()).map(|s| {
                let mut c = s.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
        });

    let Some(pu) = u.get("planUsage") else {
        return fail(
            ProviderStatus::Error,
            "No personal plan usage on this account (team-billed seats are not supported yet).",
        );
    };

    let start = ms_epoch(json_num_or_str(u.get("billingCycleStart")));
    let end = ms_epoch(json_num_or_str(u.get("billingCycleEnd")));

    let mut windows = Vec::new();
    let mut push = |key: &str, label: &str, pct: Option<f64>, detail: Option<String>| {
        let Some(pct) = pct else { return };
        let used = pct.clamp(0.0, 100.0);
        let detail = detail.unwrap_or_else(|| format!("{used:.1}% used"));
        windows.push(UsageWindow {
            key: key.to_string(),
            label: label.to_string(),
            used_percent: used,
            resets_at: end,
            detail: Some(detail),
        });
    };

    push(
        "cursor_models",
        "Cursor Models",
        num(pu, "autoPercentUsed"),
        None,
    );
    push(
        "other_models",
        "Other Models",
        num(pu, "apiPercentUsed"),
        None,
    );
    push("total", "Total", num(pu, "totalPercentUsed"), None);

    if let Some(slu) = u.get("spendLimitUsage") {
        let limit = num(slu, "individualLimit");
        let remaining = num(slu, "individualRemaining");
        if let (Some(limit), Some(remaining)) = (limit, remaining) {
            if limit > 0.0 {
                let used_cents = limit - remaining;
                let pct = used_cents / limit * 100.0;
                push(
                    "on_demand",
                    "On-Demand",
                    Some(pct),
                    Some(format!(
                        "${:.2} of ${:.2}",
                        used_cents / 100.0,
                        limit / 100.0
                    )),
                );
            }
        }
    }

    if windows.is_empty() {
        return fail(
            ProviderStatus::Error,
            "Usage response had no percentage pools.",
        );
    }

    let _ = start;

    ProviderSnapshot {
        id: "cursor".into(),
        kind: "cursor".into(),
        display_name: "Cursor".into(),
        account: email.map(|s| s.to_string()),
        plan: display_plan,
        status: ProviderStatus::Ok,
        message: u
            .get("displayMessage")
            .and_then(|m| m.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string()),
        windows,
        fetched_at: Utc::now(),
    }
}

fn fail(status: ProviderStatus, message: &str) -> ProviderSnapshot {
    ProviderSnapshot {
        id: "cursor".into(),
        kind: "cursor".into(),
        display_name: "Cursor".into(),
        account: None,
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

fn json_num_or_str(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(n) = v.as_f64() {
        return Some(n.to_string());
    }
    if let Some(n) = v.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = v.as_u64() {
        return Some(n.to_string());
    }
    None
}

fn ms_epoch(raw: Option<String>) -> Option<DateTime<Utc>> {
    let raw = raw.filter(|s| !s.trim().is_empty())?;
    let n: f64 = raw.parse().ok()?;
    if n <= 0.0 {
        return None;
    }
    let ms = if n > 1e12 {
        n as i64
    } else {
        (n * 1000.0) as i64
    };
    Utc.timestamp_millis_opt(ms).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_dashboard_percentage_pools() {
        let sample = json!({
          "billingCycleStart": "1792695957000",
          "billingCycleEnd": "1795374357000",
          "planUsage": {
            "autoPercentUsed": 42.5,
            "apiPercentUsed": 10.25,
            "totalPercentUsed": 35.0
          },
          "spendLimitUsage": {
            "individualLimit": 10000,
            "individualRemaining": 7500
          },
          "displayMessage": "You've used 35% of your included usage"
        });
        let snap = parse_usage(&sample, Some("you@example.com"), Some("pro"), Some("Pro"));
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.account.as_deref(), Some("you@example.com"));
        assert_eq!(snap.plan.as_deref(), Some("Pro"));
        let auto = snap
            .windows
            .iter()
            .find(|w| w.key == "cursor_models")
            .unwrap();
        assert!((auto.used_percent - 42.5).abs() < 0.001);
        assert_eq!(auto.detail.as_deref(), Some("42.5% used"));
        let on_demand = snap.windows.iter().find(|w| w.key == "on_demand").unwrap();
        assert!((on_demand.used_percent - 25.0).abs() < 0.001);
        assert!(on_demand.detail.as_ref().unwrap().contains("$25.00"));
        assert!(on_demand.detail.as_ref().unwrap().contains("$100.00"));
        assert!(auto.resets_at.is_some());
    }

    #[test]
    fn team_seat_without_plan_usage_errors() {
        let sample = json!({"billingCycleStart":"1","billingCycleEnd":"1"});
        let snap = parse_usage(&sample, None, None, None);
        assert_eq!(snap.status, ProviderStatus::Error);
        assert!(snap.message.unwrap().to_lowercase().contains("plan"));
    }
}
