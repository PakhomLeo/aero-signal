//! 管理员认证 API 与会话提取器。

use axum::extract::FromRequestParts;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::request::Parts;
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use server_core::AppError;

use crate::domain::BootstrapCredentials;
use crate::domain::administrator::AdministratorPublic;
use crate::service::{AuthSession, LoginCommand};

use super::AdminState;

/// 已认证管理员会话（Axum extractor）。
#[derive(Debug, Clone)]
pub struct AdminAuth(pub AuthSession);

impl AdminAuth {
    pub fn session(&self) -> &AuthSession {
        &self.0
    }
}

impl FromRequestParts<AdminState> for AdminAuth {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AdminState,
    ) -> Result<Self, Self::Rejection> {
        let token = extract_bearer(&parts.headers)
            .ok_or_else(|| AppError::unauthorized("缺少 Authorization Bearer 会话"))?;
        let auth = state.admin.sessions().authenticate_token(&token).await?;
        Ok(AdminAuth(auth))
    }
}

pub fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let rest = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    let token = rest.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: String,
    pub admin: AdministratorPublic,
}

#[derive(Debug, Deserialize)]
pub struct BootstrapRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct BootstrapResponse {
    pub username: String,
    pub must_change_password: bool,
}

pub fn auth_routes() -> Router<AdminState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/bootstrap", post(bootstrap))
}

async fn login(
    State(state): State<AdminState>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());
    let ua = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let result = state
        .admin
        .login(LoginCommand {
            username: body.username,
            password: body.password,
            ip,
            user_agent: ua,
        })
        .await?;

    Ok(Json(LoginResponse {
        token: result.token.into_inner(),
        expires_at: result.expires_at.to_rfc3339(),
        admin: result.admin,
    }))
}

async fn logout(
    State(state): State<AdminState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = extract_bearer(&headers)
        .ok_or_else(|| AppError::unauthorized("缺少 Authorization Bearer 会话"))?;
    let auth = state.admin.sessions().authenticate_token(&token).await?;
    state.admin.logout(auth.session_id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

async fn bootstrap(
    State(state): State<AdminState>,
    Json(body): Json<BootstrapRequest>,
) -> Result<Json<BootstrapResponse>, AppError> {
    let result = state
        .admin
        .bootstrap(BootstrapCredentials {
            username: body.username,
            password: body.password,
        })
        .await?;
    Ok(Json(BootstrapResponse {
        username: result.username,
        must_change_password: result.must_change_password,
    }))
}
