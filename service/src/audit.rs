//! 统一审计写入（AuditWriter）与 PRD §12 动作覆盖对照。
//!
//! 业务服务应通过本模块写入审计；禁止在 API/adapter 层直接拼审计行。
//! 审计 details 只允许稳定 ID、错误码、状态与脱敏计数，禁止凭据/全文/代理密码。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use crate::repository::audit::{AuditLogRecord, AuditRepository};

/// 允许的 actor_type（与 `audit_logs` CHECK 一致）。
pub const ACTOR_SYSTEM_ADMINISTRATOR: &str = "system_administrator";
pub const ACTOR_API_TOKEN: &str = "api_token";
pub const ACTOR_SYSTEM: &str = "system";
pub const ACTOR_WORKER: &str = "worker";

/// PRD §12 要求必须审计的动作（规范名）。
/// 实现侧可用同义 action 字符串，但须在覆盖表中映射。
pub const PRD_REQUIRED_AUDIT_ACTIONS: &[&str] = &[
    "admin.username_changed",
    "admin.password_changed",
    "collector.qr_session_created",
    "collector.qr_session_cancelled",
    "collector.qr_session_confirmed",
    "collector.egress_binding_changed",
    "source.created",
    "source.rules_updated",
    "source.initial_validation",
    "daily_plan.rebuilt",
    "article.manual_fetch",
    "article.refetch",
    "article.reocr",
    "article.edited",
    "delivery.group_changed",
    "delivery.token_changed",
    "egress.proxy_changed",
    "admin.schedule_updated",
];

/// 当前代码库已实际写入的 action（与服务层字符串对齐）。
pub const IMPLEMENTED_AUDIT_ACTIONS: &[&str] = &[
    "admin.bootstrap",
    "admin.login",
    "admin.login_failed",
    "admin.logout",
    "admin.password_changed",
    "admin.username_changed",
    "admin.schedule_updated",
    "collector.qr_session_created",
    "collector.qr_session_cancelled",
    "collector.qr_session_confirmed",
    "collector.egress_binding_changed",
    "source.created",
    "source.rules_updated",
    "source.initial_validation",
    "daily_plan.rebuilt",
    "article.manual_fetch",
    "article.refetch",
    "article.reocr",
    "article.edited",
    "delivery.group_changed",
    "delivery.token_changed",
    "egress.proxy_provider_created",
    "egress.proxy_provider_updated",
    "egress.profile_created",
    "egress.profile_updated",
    "egress.profile_tested",
    "retention.cleanup_run",
];

/// 统一审计写入器。
#[derive(Clone)]
pub struct AuditWriter {
    repo: std::sync::Arc<dyn AuditRepository>,
}

impl AuditWriter {
    pub fn new(repo: std::sync::Arc<dyn AuditRepository>) -> Self {
        Self { repo }
    }

    pub fn repo(&self) -> &std::sync::Arc<dyn AuditRepository> {
        &self.repo
    }

    /// 写入一条成功/失败审计；details 经敏感键校验。
    #[allow(clippy::too_many_arguments)]
    pub async fn write(
        &self,
        actor_type: &str,
        actor_id: Option<&str>,
        action: &str,
        resource_type: Option<&str>,
        resource_id: Option<&str>,
        result: &str,
        details: Value,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        validate_actor_type(actor_type)?;
        validate_audit_details(&details)?;
        let record = AuditLogRecord {
            id: Uuid::new_v4(),
            actor_type: actor_type.to_string(),
            actor_id: actor_id.map(|s| s.to_string()),
            action: action.to_string(),
            resource_type: resource_type.map(|s| s.to_string()),
            resource_id: resource_id.map(|s| s.to_string()),
            result: result.to_string(),
            details_json: details,
            created_at: now,
        };
        self.repo.append(&record).await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn success(
        &self,
        actor_type: &str,
        actor_id: Option<&str>,
        action: &str,
        resource_type: Option<&str>,
        resource_id: Option<&str>,
        details: Value,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        self.write(
            actor_type,
            actor_id,
            action,
            resource_type,
            resource_id,
            "success",
            details,
            now,
        )
        .await
    }
}

/// 便于测试注入的 trait 别名。
#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn append_audit(&self, record: &AuditLogRecord) -> Result<(), String>;
}

#[async_trait]
impl AuditSink for AuditWriter {
    async fn append_audit(&self, record: &AuditLogRecord) -> Result<(), String> {
        validate_actor_type(&record.actor_type)?;
        validate_audit_details(&record.details_json)?;
        self.repo.append(record).await
    }
}

fn validate_actor_type(actor_type: &str) -> Result<(), String> {
    match actor_type {
        ACTOR_SYSTEM_ADMINISTRATOR | ACTOR_API_TOKEN | ACTOR_SYSTEM | ACTOR_WORKER => Ok(()),
        other => Err(format!("audit.invalid_actor_type: {other}")),
    }
}

/// 审计 details 禁止敏感键（允许 has_* / must_change_password 等标志）。
pub fn validate_audit_details(details: &Value) -> Result<(), String> {
    if let Some(obj) = details.as_object() {
        for key in obj.keys() {
            if is_forbidden_audit_key(key) {
                return Err(format!("audit details 禁止包含敏感键: {key}"));
            }
        }
    }
    Ok(())
}

fn is_forbidden_audit_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "has_auth_secret"
            | "has_endpoint_secret"
            | "must_change_password"
            | "redaction_count"
            | "error_code"
    ) {
        return false;
    }
    matches!(
        lower.as_str(),
        "password"
            | "new_password"
            | "current_password"
            | "old_password"
            | "token"
            | "session_token"
            | "secret"
            | "password_hash"
            | "cookie"
            | "cookies"
            | "proxy_password"
            | "proxy_url"
            | "auth_secret"
            | "endpoint_secret"
            | "qr_url"
            | "qr_content"
    ) || lower.contains("password") && !lower.starts_with("must_change")
        || (lower.contains("secret") && !lower.starts_with("has_"))
        || lower.contains("cookie")
}

/// PRD 要求动作中尚未由服务层写入的缺口（文档与测试共用）。
pub fn audit_coverage_gaps() -> Vec<&'static str> {
    PRD_REQUIRED_AUDIT_ACTIONS
        .iter()
        .copied()
        .filter(|req| {
            !IMPLEMENTED_AUDIT_ACTIONS.contains(req)
                && !IMPLEMENTED_AUDIT_ACTIONS.iter().any(|impl_a| {
                    // 代理变更由 egress.proxy_* / profile_* 覆盖
                    (*req == "egress.proxy_changed"
                        && (impl_a.starts_with("egress.proxy_")
                            || impl_a.starts_with("egress.profile_")))
                        || (*req == "admin.schedule_updated" && *impl_a == "admin.schedule_updated")
                })
        })
        .filter(|req| *req != "egress.proxy_changed" && *req != "admin.schedule_updated")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::memory::MemoryAdministratorStore;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn writer_rejects_sensitive_details() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let w = AuditWriter::new(store.clone() as Arc<dyn AuditRepository>);
        let err = w
            .success(
                ACTOR_SYSTEM,
                None,
                "admin.bootstrap",
                Some("system_administrator"),
                None,
                json!({ "password": "secret" }),
                Utc::now(),
            )
            .await
            .unwrap_err();
        assert!(err.contains("敏感键"));
    }

    #[tokio::test]
    async fn writer_accepts_safe_details() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let w = AuditWriter::new(store.clone() as Arc<dyn AuditRepository>);
        w.success(
            ACTOR_SYSTEM_ADMINISTRATOR,
            Some("admin"),
            "admin.password_changed",
            Some("system_administrator"),
            Some("admin"),
            json!({ "must_change_password": false }),
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!(store.audit_count().await, 1);
    }

    #[test]
    fn coverage_lists_known_gaps() {
        let gaps = audit_coverage_gaps();
        // PRD §12 必审动作已全部接线；缺口应为空
        assert!(gaps.is_empty(), "unexpected audit coverage gaps: {gaps:?}");
        // 已实现的不应在缺口
        assert!(!gaps.contains(&"admin.password_changed"));
        assert!(!gaps.contains(&"admin.username_changed"));
        assert!(!gaps.contains(&"collector.qr_session_created"));
        assert!(!gaps.contains(&"article.edited"));
        assert!(!gaps.contains(&"delivery.token_changed"));
    }
}
