//! ApiTokenService：签发（明文一次）、hash、scope、IP allowlist、过期与撤销。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::json;
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::audit::{ACTOR_SYSTEM_ADMINISTRATOR, AuditWriter};
use crate::domain::delivery::{
    ApiCallLog, ApiScope, ApiToken, ApiTokenPublic, DeliveryGroupStatus, IssuedApiToken,
    PlainApiToken, generate_api_token, hash_api_token, token_prefix,
};
use crate::domain::session::hash_client_hint;
use crate::repository::delivery::DeliveryRepository;

pub struct ApiTokenService {
    delivery: Arc<dyn DeliveryRepository>,
    clock: Arc<dyn Clock>,
    audit: Option<AuditWriter>,
}

#[derive(Debug, Clone)]
pub struct IssueTokenCommand {
    pub delivery_group_id: Uuid,
    pub name: String,
    pub scopes: Vec<ApiScope>,
    pub ip_allowlist: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct AuthenticatedApiToken {
    pub token: ApiToken,
    pub plaintext_prefix: String,
}

impl ApiTokenService {
    pub fn new(delivery: Arc<dyn DeliveryRepository>, clock: Arc<dyn Clock>) -> Self {
        Self {
            delivery,
            clock,
            audit: None,
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    pub async fn issue(&self, cmd: IssueTokenCommand) -> Result<IssuedApiToken, AppError> {
        let name = cmd.name.trim().to_string();
        if name.is_empty() {
            return Err(AppError::new("token.invalid_name", "Token 名称不能为空"));
        }
        if cmd.scopes.is_empty() {
            return Err(AppError::new("token.scopes_required", "至少需要一个 scope"));
        }
        let group = self
            .delivery
            .get_group(cmd.delivery_group_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("delivery.group_not_found", "分组不存在"))?;
        if group.status != DeliveryGroupStatus::Active {
            return Err(AppError::new(
                "delivery.group_disabled",
                "分组已停用，无法签发 Token",
            ));
        }
        let now = self.now();
        if let Some(exp) = cmd.expires_at
            && exp <= now
        {
            return Err(AppError::new(
                "token.expires_in_past",
                "有效期必须晚于当前时间",
            ));
        }

        let plaintext = generate_api_token();
        let hash = hash_api_token(plaintext.as_str());
        let prefix = token_prefix(plaintext.as_str());
        let row = ApiToken {
            id: Uuid::new_v4(),
            delivery_group_id: cmd.delivery_group_id,
            name,
            token_prefix: prefix,
            token_hash: hash,
            scopes: cmd.scopes,
            ip_allowlist: cmd.ip_allowlist,
            expires_at: cmd.expires_at,
            revoked_at: None,
            last_used_at: None,
            created_at: now,
        };
        let saved = self
            .delivery
            .insert_token(row)
            .await
            .map_err(AppError::internal)?;
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "delivery.token_changed",
                    Some("api_token"),
                    Some(&saved.id.to_string()),
                    json!({
                        "op": "issued",
                        "delivery_group_id": saved.delivery_group_id.to_string(),
                        "scope_count": saved.scopes.len(),
                        "token_prefix": saved.token_prefix,
                    }),
                    now,
                )
                .await;
        }
        Ok(IssuedApiToken {
            token: saved,
            plaintext,
        })
    }

    pub async fn revoke(&self, token_id: Uuid) -> Result<ApiTokenPublic, AppError> {
        let now = self.now();
        let t = self
            .delivery
            .revoke_token(token_id, now)
            .await
            .map_err(|e| {
                if e.starts_with("token.not_found") {
                    AppError::not_found("token.not_found", "Token 不存在")
                } else {
                    AppError::internal(e)
                }
            })?;
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "delivery.token_changed",
                    Some("api_token"),
                    Some(&t.id.to_string()),
                    json!({
                        "op": "revoked",
                        "delivery_group_id": t.delivery_group_id.to_string(),
                        "token_prefix": t.token_prefix,
                    }),
                    now,
                )
                .await;
        }
        Ok(ApiTokenPublic::from_token(&t, now))
    }

    pub async fn list_for_group(&self, group_id: Uuid) -> Result<Vec<ApiTokenPublic>, AppError> {
        let now = self.now();
        let rows = self
            .delivery
            .list_tokens_for_group(group_id)
            .await
            .map_err(AppError::internal)?;
        Ok(rows
            .iter()
            .map(|t| ApiTokenPublic::from_token(t, now))
            .collect())
    }

    /// 校验明文 Token；失败返回明确错误码。
    pub async fn authenticate(
        &self,
        plaintext: &str,
        client_ip: Option<&str>,
        required_scope: ApiScope,
    ) -> Result<AuthenticatedApiToken, AppError> {
        let hash = hash_api_token(plaintext);
        let token = self
            .delivery
            .find_token_by_hash(&hash)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::new("token.invalid", "无效的 API Token")
                    .with_status(axum::http::StatusCode::UNAUTHORIZED)
            })?;

        let now = self.now();
        if token.revoked_at.is_some() {
            return Err(AppError::new("token.revoked", "Token 已撤销")
                .with_status(axum::http::StatusCode::UNAUTHORIZED));
        }
        if let Some(exp) = token.expires_at
            && exp <= now
        {
            return Err(AppError::new("token.expired", "Token 已过期")
                .with_status(axum::http::StatusCode::UNAUTHORIZED));
        }

        let group = self
            .delivery
            .get_group(token.delivery_group_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("delivery.group_not_found", "分组不存在"))?;
        if group.status != DeliveryGroupStatus::Active {
            return Err(AppError::new("delivery.group_disabled", "分组已停用")
                .with_status(axum::http::StatusCode::FORBIDDEN));
        }

        if !token.ip_allowed(client_ip) {
            return Err(AppError::new("token.ip_denied", "调用方 IP 不在 allowlist")
                .with_status(axum::http::StatusCode::FORBIDDEN));
        }

        if !token.has_scope(required_scope) {
            return Err(AppError::new("token.scope_denied", "缺少所需 scope")
                .with_status(axum::http::StatusCode::FORBIDDEN)
                .with_detail("required_scope", required_scope.as_str()));
        }

        let _ = self.delivery.touch_token_used(token.id, now).await;
        Ok(AuthenticatedApiToken {
            plaintext_prefix: token.token_prefix.clone(),
            token,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_call(
        &self,
        auth: &AuthenticatedApiToken,
        method: &str,
        path: &str,
        scope_used: Option<&str>,
        status_code: i32,
        duration_ms: i32,
        redaction_count: i32,
        client_ip: Option<&str>,
    ) -> Result<(), AppError> {
        let log = ApiCallLog {
            id: Uuid::new_v4(),
            api_token_id: auth.token.id,
            delivery_group_id: auth.token.delivery_group_id,
            method: method.into(),
            path: path.into(),
            scope_used: scope_used.map(|s| s.to_string()),
            status_code,
            duration_ms,
            redaction_count,
            client_ip_hash: client_ip.map(hash_client_hint),
            created_at: self.now(),
        };
        self.delivery
            .insert_call_log(log)
            .await
            .map_err(AppError::internal)?;
        Ok(())
    }

    /// 仅用于测试断言：明文不会再次从仓储读出。
    pub fn reissue_plaintext_impossible(token: &ApiToken) -> Option<PlainApiToken> {
        let _ = token;
        None
    }
}
