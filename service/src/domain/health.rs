//! 每日健康快照领域模型（M11）。

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 快照总体状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverallHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

impl OverallHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "unhealthy" => Ok(Self::Unhealthy),
            other => Err(format!("未知 overall_status: {other}")),
        }
    }
}

/// 组件类型（与迁移 CHECK 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthComponentType {
    Collector,
    Connector,
    FixedEgress,
    DynamicEgress,
    Worker,
    ObjectStore,
}

impl HealthComponentType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Collector => "collector",
            Self::Connector => "connector",
            Self::FixedEgress => "fixed_egress",
            Self::DynamicEgress => "dynamic_egress",
            Self::Worker => "worker",
            Self::ObjectStore => "object_store",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "collector" => Ok(Self::Collector),
            "connector" => Ok(Self::Connector),
            "fixed_egress" => Ok(Self::FixedEgress),
            "dynamic_egress" => Ok(Self::DynamicEgress),
            "worker" => Ok(Self::Worker),
            "object_store" => Ok(Self::ObjectStore),
            other => Err(format!("未知 component_type: {other}")),
        }
    }

    /// 核心依赖：不可用则 overall=unhealthy。
    pub fn is_core(self) -> bool {
        matches!(self, Self::Worker | Self::ObjectStore)
    }
}

/// 组件状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthComponentStatus {
    Healthy,
    Degraded,
    Unhealthy,
    NotConfigured,
}

impl HealthComponentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Unhealthy => "unhealthy",
            Self::NotConfigured => "not_configured",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "healthy" => Ok(Self::Healthy),
            "degraded" => Ok(Self::Degraded),
            "unhealthy" => Ok(Self::Unhealthy),
            "not_configured" => Ok(Self::NotConfigured),
            other => Err(format!("未知 component status: {other}")),
        }
    }

    pub fn is_ok(self) -> bool {
        matches!(self, Self::Healthy | Self::NotConfigured)
    }
}

/// 不可变日健康快照。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealthSnapshot {
    pub id: Uuid,
    pub snapshot_date: NaiveDate,
    pub generated_at: DateTime<Utc>,
    pub overall_status: OverallHealthStatus,
    pub yesterday_plan_success_count: i32,
    pub yesterday_plan_failure_count: i32,
    pub yesterday_article_failure_count: i32,
    pub today_eligible_source_count: i32,
    pub today_blocked_source_count: i32,
    pub pending_action_count: i32,
    pub summary_json: Value,
}

/// 快照内组件行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceHealthComponent {
    pub id: Uuid,
    pub snapshot_id: Uuid,
    pub component_type: HealthComponentType,
    pub component_id: Option<Uuid>,
    pub status: HealthComponentStatus,
    pub reason_code: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub safe_details_json: Value,
}

/// 管理员「立即检查」独立运行记录（不写入/不覆盖快照）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckRun {
    pub id: Uuid,
    pub job_id: Option<Uuid>,
    pub target_type: String,
    pub target_id: Option<Uuid>,
    pub status: HealthComponentStatus,
    pub reason_code: Option<String>,
    pub safe_details_json: Value,
    pub checked_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// 由组件列表归纳 overall。
pub fn aggregate_overall(components: &[ServiceHealthComponent]) -> OverallHealthStatus {
    let mut any_core_unhealthy = false;
    let mut any_partial_issue = false;

    for c in components {
        match c.status {
            HealthComponentStatus::Unhealthy => {
                if c.component_type.is_core() {
                    any_core_unhealthy = true;
                } else {
                    any_partial_issue = true;
                }
            }
            HealthComponentStatus::Degraded => {
                any_partial_issue = true;
            }
            HealthComponentStatus::Healthy | HealthComponentStatus::NotConfigured => {}
        }
    }

    if any_core_unhealthy {
        OverallHealthStatus::Unhealthy
    } else if any_partial_issue {
        OverallHealthStatus::Degraded
    } else {
        OverallHealthStatus::Healthy
    }
}

/// 脱敏：剔除 cookie/token/代理密码等敏感键，保留错误码与计数。
pub fn sanitize_safe_details(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                if is_sensitive_key(k) {
                    continue;
                }
                out.insert(k.clone(), sanitize_safe_details(v));
            }
            Value::Object(out)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sanitize_safe_details).collect()),
        Value::String(s) if looks_like_secret(s) => Value::String("[redacted]".into()),
        other => other.clone(),
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    [
        "cookie",
        "cookies",
        "token",
        "access_token",
        "refresh_token",
        "authorization",
        "password",
        "secret",
        "proxy",
        "proxy_url",
        "proxy_password",
        "endpoint",
        "credential",
        "credentials",
        "raw_error",
        "error_body",
        "body",
        "html",
        "qr_url",
        "qr_content",
        "qr_ticket",
    ]
    .iter()
    .any(|d| lower == *d || lower.contains(d))
}

fn looks_like_secret(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.contains("cookie=")
        || lower.contains("bearer ")
        || lower.contains("://") && (lower.contains("proxy") || lower.contains("@"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregate_core_unhealthy() {
        let comps = vec![ServiceHealthComponent {
            id: Uuid::new_v4(),
            snapshot_id: Uuid::new_v4(),
            component_type: HealthComponentType::Worker,
            component_id: None,
            status: HealthComponentStatus::Unhealthy,
            reason_code: Some("worker.unavailable".into()),
            checked_at: Utc::now(),
            safe_details_json: json!({}),
        }];
        assert_eq!(aggregate_overall(&comps), OverallHealthStatus::Unhealthy);
    }

    #[test]
    fn aggregate_partial_degraded() {
        let comps = vec![
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({}),
            },
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: Uuid::new_v4(),
                component_type: HealthComponentType::Collector,
                component_id: Some(Uuid::new_v4()),
                status: HealthComponentStatus::Unhealthy,
                reason_code: Some("collector.needs_relogin".into()),
                checked_at: Utc::now(),
                safe_details_json: json!({}),
            },
        ];
        assert_eq!(aggregate_overall(&comps), OverallHealthStatus::Degraded);
    }

    #[test]
    fn sanitize_strips_cookie_and_proxy() {
        let v = json!({
            "error_code": "egress.network_unreachable",
            "cookie": "sid=abc",
            "proxy_url": "http://user:pass@1.2.3.4:8080",
            "counts": { "ok": 1 }
        });
        let s = sanitize_safe_details(&v);
        assert_eq!(s["error_code"], "egress.network_unreachable");
        assert!(s.get("cookie").is_none());
        assert!(s.get("proxy_url").is_none());
        assert_eq!(s["counts"]["ok"], 1);
    }
}
