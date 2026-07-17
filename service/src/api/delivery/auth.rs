//! Delivery API Token 提取器。

use axum::extract::FromRequestParts;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use server_core::AppError;
use std::sync::Arc;

use crate::domain::delivery::ApiScope;
use crate::service::{
    ApiTokenService, ArticleCursorService, AuthenticatedApiToken, DeliveryGroupService,
    HealthDeliveryService, SourceValidationService,
};

use crate::api::admin::extract_bearer;

#[derive(Clone)]
pub struct DeliveryState {
    pub tokens: Arc<ApiTokenService>,
    pub groups: Arc<DeliveryGroupService>,
    pub articles: Arc<ArticleCursorService>,
    pub health: Arc<HealthDeliveryService>,
    pub sources: Option<Arc<SourceValidationService>>,
}

/// 已通过 API Token 鉴权的调用方（需指定 required scope）。
pub struct DeliveryAuth {
    pub auth: AuthenticatedApiToken,
    pub client_ip: Option<String>,
    pub scope: ApiScope,
}

pub fn extract_client_ip(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

/// 按 scope 构造 extractor 的辅助：在 handler 内调用 authenticate。
pub async fn require_scope(
    state: &DeliveryState,
    parts: &Parts,
    scope: ApiScope,
) -> Result<DeliveryAuth, AppError> {
    let token = extract_bearer(&parts.headers).ok_or_else(|| {
        AppError::new("token.missing", "缺少 Authorization Bearer API Token")
            .with_status(axum::http::StatusCode::UNAUTHORIZED)
    })?;
    let client_ip = extract_client_ip(&parts.headers);
    let auth = state
        .tokens
        .authenticate(&token, client_ip.as_deref(), scope)
        .await?;
    Ok(DeliveryAuth {
        auth,
        client_ip,
        scope,
    })
}

/// 用于 FromRequestParts 的 scope 标记类型。
pub struct RequireArticlesRead(pub DeliveryAuth);
pub struct RequireSourcesRead(pub DeliveryAuth);
pub struct RequireServiceHealthRead(pub DeliveryAuth);

impl FromRequestParts<DeliveryState> for RequireArticlesRead {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &DeliveryState,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequireArticlesRead(
            require_scope(state, parts, ApiScope::ArticlesRead).await?,
        ))
    }
}

impl FromRequestParts<DeliveryState> for RequireSourcesRead {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &DeliveryState,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequireSourcesRead(
            require_scope(state, parts, ApiScope::SourcesRead).await?,
        ))
    }
}

impl FromRequestParts<DeliveryState> for RequireServiceHealthRead {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &DeliveryState,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequireServiceHealthRead(
            require_scope(state, parts, ApiScope::ServiceHealthRead).await?,
        ))
    }
}
