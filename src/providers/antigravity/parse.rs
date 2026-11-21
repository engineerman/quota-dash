use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::{ProviderSnapshot, ProviderStatus, UsageWindow};

type FamilyMatch = fn(&str) -> bool;

const FAMILIES: &[(&str, &str, FamilyMatch)] = &[
    ("gemini-pro", "Gemini Pro", |id| {
        id.starts_with("gemini") && id.contains("pro")
    }),
    ("gemini-flash", "Gemini Flash", |id| {
        id.starts_with("gemini") && id.contains("flash") && !id.contains("image")
    }),
    ("claude", "Claude", |id| id.starts_with("claude")),
    ("gpt", "GPT-OSS", |id| id.starts_with("gpt")),
    ("image", "Image", |id| id.contains("image")),
];

/// Maps `fetchAvailableModels` `models.*.quotaInfo.remainingFraction` into family windows.
pub fn parse_models(
    models_json: &Value,
    email: Option<&str>,
    plan: Option<&str>,
) -> ProviderSnapshot {
    let account = email.map(|s| s.to_string());
    let id = match email {
        Some(e) => format!("antigravity:{e}"),
        None => "antigravity".into(),
    };

    let Some(models) = models_json.get("models").and_then(|m| m.as_object()) else {
        return ProviderSnapshot {
            id,
            kind: "antigravity".into(),
            display_name: "Antigravity".into(),
            account,
            plan: plan.map(|s| s.to_string()),
            status: ProviderStatus::Ok,
            message: None,
            windows: Vec::new(),
            fetched_at: Utc::now(),
        };
    };

    struct Bucket {
        used: f64,
        reset: Option<DateTime<Utc>>,
        names: Vec<String>,
    }

    let mut buckets: Vec<(String, Bucket)> = Vec::new();

    for (model_id, model) in models {
        let id_l = model_id.to_lowercase();
        let Some(display) = model
            .get("displayName")
            .and_then(|d| d.as_str())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(q) = model.get("quotaInfo") else {
            continue;
        };
        let Some((key, _label, _)) = FAMILIES.iter().find(|(_, _, m)| m(&id_l)) else {
            continue;
        };

        let remaining = q
            .get("remainingFraction")
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_i64().map(|i| i as f64))
                    .or_else(|| v.as_u64().map(|u| u as f64))
            })
            .unwrap_or(1.0);
        let used = (1.0 - remaining) * 100.0;
        let reset = q
            .get("resetTime")
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc));

        if let Some((_, b)) = buckets.iter_mut().find(|(k, _)| k == *key) {
            if !b.names.iter().any(|n| n == display) {
                b.names.push(display.to_string());
            }
            b.used = b.used.max(used);
            if let Some(r) = reset {
                b.reset = Some(match b.reset {
                    Some(cur) if cur <= r => cur,
                    _ => r,
                });
            }
        } else {
            buckets.push((
                (*key).to_string(),
                Bucket {
                    used,
                    reset,
                    names: vec![display.to_string()],
                },
            ));
        }
    }

    let mut windows = Vec::new();
    for (key, label, _) in FAMILIES {
        let Some((_, b)) = buckets.iter().find(|(k, _)| k == *key) else {
            continue;
        };
        let detail = if b.names.len() > 4 {
            format!(
                "{} +{}",
                b.names
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                b.names.len() - 4
            )
        } else {
            b.names.join(", ")
        };
        windows.push(UsageWindow {
            key: (*key).to_string(),
            label: (*label).to_string(),
            used_percent: (b.used * 10.0).round() / 10.0,
            resets_at: b.reset,
            detail: Some(detail),
        });
    }

    ProviderSnapshot {
        id,
        kind: "antigravity".into(),
        display_name: "Antigravity".into(),
        account,
        plan: plan.map(|s| s.to_string()),
        status: ProviderStatus::Ok,
        message: None,
        windows,
        fetched_at: Utc::now(),
    }
}

/// Windows from `retrieveUserQuotaSummary`, labeled the way T3 labels them.
/// The first host's reading of a bucket id wins; callers merge hosts themselves.
pub fn parse_quota_summary(body: &Value, requested_at: DateTime<Utc>) -> Vec<UsageWindow> {
    let groups = match body.get("groups").and_then(|g| g.as_array()) {
        Some(groups) if groups.iter().any(|g| bucket_list(g).is_some_and(|b| !b.is_empty())) => {
            groups.to_vec()
        }
        _ => vec![serde_json::json!({ "buckets": body.get("buckets").cloned() })],
    };

    let mut windows = Vec::new();
    for group in groups {
        let group_name = group
            .get("displayName")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(buckets) = bucket_list(&group) else {
            continue;
        };
        for bucket in buckets {
            if bucket.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let Some(bucket_id) = bucket.get("bucketId").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(remaining) = bucket.get("remainingFraction").and_then(as_fraction) else {
                continue;
            };
            if !remaining.is_finite() {
                continue;
            }
            let window = bucket.get("window").and_then(|v| v.as_str()).unwrap_or("");
            let family = summary_family(bucket_id, group_name);
            let period = match window {
                "weekly" => "Weekly",
                "5h" => "5-hour",
                _ => bucket
                    .get("displayName")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Quota"),
            };
            let used = (1.0 - remaining.clamp(0.0, 1.0)) * 100.0;
            let resets_at = summary_reset(bucket, window, remaining, requested_at);
            if windows.iter().any(|have: &UsageWindow| have.key == bucket_id) {
                continue;
            }
            windows.push(UsageWindow {
                key: bucket_id.to_string(),
                label: format!("{family} · {period}"),
                used_percent: (used * 10.0).round() / 10.0,
                resets_at,
                detail: None,
            });
        }
    }
    windows
}

fn bucket_list(group: &Value) -> Option<&Vec<Value>> {
    group.get("buckets").and_then(|v| v.as_array())
}

fn as_fraction(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|i| i as f64))
        .or_else(|| value.as_u64().map(|u| u as f64))
}

fn summary_family(bucket_id: &str, group_name: Option<&str>) -> String {
    if bucket_id.starts_with("gemini-argon-") {
        group_name.unwrap_or("Gemini Argon").to_string()
    } else if bucket_id.starts_with("gemini-carbon-") {
        group_name.unwrap_or("Gemini Carbon").to_string()
    } else if bucket_id.starts_with("gemini-") {
        "Gemini".into()
    } else if bucket_id.starts_with("3p-") {
        "Claude & GPT (shared)".into()
    } else {
        group_name.unwrap_or(bucket_id).to_string()
    }
}

/// An untouched bucket reports a reset one full window ahead. That is not a real reset.
fn summary_reset(
    bucket: &Value,
    window: &str,
    remaining: f64,
    requested_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let reset = bucket
        .get("resetTime")
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc));
    let duration_mins = match window {
        "5h" => Some(5 * 60),
        "weekly" => Some(7 * 24 * 60),
        _ => None,
    };
    let untouched = (remaining - 1.0).abs() < f64::EPSILON
        && bucket.get("description").and_then(|v| v.as_str()).is_none()
        && duration_mins.is_some();
    if untouched {
        if let (Some(reset), Some(mins)) = (reset, duration_mins) {
            let expected = requested_at + chrono::Duration::minutes(mins);
            if (reset - expected).num_seconds().abs() <= 60 {
                return None;
            }
        }
    }
    reset
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_family_windows_from_remaining_fraction() {
        let sample = json!({
            "models": {
                "gemini-2.5-pro": {
                    "displayName": "Gemini 2.5 Pro",
                    "quotaInfo": {
                        "remainingFraction": 0.6,
                        "resetTime": "2099-06-01T12:00:00Z"
                    }
                },
                "gemini-2.5-flash": {
                    "displayName": "Gemini 2.5 Flash",
                    "quotaInfo": {
                        "remainingFraction": 0.25,
                        "resetTime": "2099-06-02T12:00:00Z"
                    }
                },
                "claude-sonnet-4": {
                    "displayName": "Claude Sonnet 4",
                    "quotaInfo": { "remainingFraction": 0.9 }
                },
                "gpt-oss": {
                    "displayName": "GPT-OSS",
                    "quotaInfo": { "remainingFraction": 1.0 }
                },
                "gemini-2.0-flash-preview-image-generation": {
                    "displayName": "Image Gen",
                    "quotaInfo": { "remainingFraction": 0.5 }
                }
            }
        });
        let snap = parse_models(&sample, Some("you@gmail.com"), Some("Pro"));
        assert_eq!(snap.status, ProviderStatus::Ok);
        assert_eq!(snap.id, "antigravity:you@gmail.com");
        assert_eq!(snap.plan.as_deref(), Some("Pro"));

        let pro = snap.windows.iter().find(|w| w.key == "gemini-pro").unwrap();
        assert!((pro.used_percent - 40.0).abs() < 0.01);
        assert!(pro.resets_at.is_some());

        let flash = snap
            .windows
            .iter()
            .find(|w| w.key == "gemini-flash")
            .unwrap();
        assert!((flash.used_percent - 75.0).abs() < 0.01);

        assert!(snap.windows.iter().any(|w| w.key == "claude"));
        assert!(snap.windows.iter().any(|w| w.key == "gpt"));
        let image = snap.windows.iter().find(|w| w.key == "image").unwrap();
        assert!((image.used_percent - 50.0).abs() < 0.01);
        assert_eq!(image.detail.as_deref(), Some("Image Gen"));
    }

    #[test]
    fn max_used_within_family() {
        let sample = json!({
            "models": {
                "gemini-1.5-pro": {
                    "displayName": "1.5 Pro",
                    "quotaInfo": { "remainingFraction": 0.8 }
                },
                "gemini-2.5-pro": {
                    "displayName": "2.5 Pro",
                    "quotaInfo": { "remainingFraction": 0.3 }
                }
            }
        });
        let snap = parse_models(&sample, None, None);
        let pro = snap.windows.iter().find(|w| w.key == "gemini-pro").unwrap();
        assert!((pro.used_percent - 70.0).abs() < 0.01);
        assert!(pro.detail.as_ref().unwrap().contains("1.5 Pro"));
        assert!(pro.detail.as_ref().unwrap().contains("2.5 Pro"));
    }

    #[test]
    fn summary_buckets_match_t3_labels() {
        let sample = json!({
            "groups": [{
                "buckets": [
                    {
                        "bucketId": "gemini-pro-5h",
                        "window": "5h",
                        "remainingFraction": 0.26,
                        "resetTime": "2099-06-01T12:00:00Z"
                    },
                    {
                        "bucketId": "gemini-weekly",
                        "window": "weekly",
                        "remainingFraction": 0.47
                    },
                    {
                        "bucketId": "3p-shared-weekly",
                        "window": "weekly",
                        "remainingFraction": 0.9
                    },
                    {
                        "bucketId": "gemini-off",
                        "window": "5h",
                        "remainingFraction": 0.1,
                        "disabled": true
                    }
                ]
            }]
        });
        let windows = parse_quota_summary(&sample, Utc::now());
        assert_eq!(windows.len(), 3);
        let five = windows.iter().find(|w| w.key == "gemini-pro-5h").unwrap();
        assert_eq!(five.label, "Gemini · 5-hour");
        assert!((five.used_percent - 74.0).abs() < 0.01);
        assert!(five.resets_at.is_some());
        let weekly = windows.iter().find(|w| w.key == "gemini-weekly").unwrap();
        assert_eq!(weekly.label, "Gemini · Weekly");
        assert!((weekly.used_percent - 53.0).abs() < 0.01);
        let shared = windows
            .iter()
            .find(|w| w.key == "3p-shared-weekly")
            .unwrap();
        assert_eq!(shared.label, "Claude & GPT (shared) · Weekly");
        assert!((shared.used_percent - 10.0).abs() < 0.01);
    }
}
