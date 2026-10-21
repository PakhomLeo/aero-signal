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
    /// 必须与服务端 `SIGNAL_BOOTSTRAP_TOKEN` 一致。
    #[serde(default)]
    pub bootstrap_token: Option<String>,
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
        .bootstrap_with_token(
            body.bootstrap_token.as_deref(),
            BootstrapCredentials {
                username: body.username,
                password: body.password,
            },
        )
        .await?;
    Ok(Json(BootstrapResponse {
        username: result.username,
        must_change_password: result.must_change_password,
    }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use job_pipeline::MemoryRepo;
    use server_core::{Clock, SystemClock};
    use tower::ServiceExt;

    use crate::adapters::MemorySecretStore;
    use crate::api::admin::{JobsState, admin_router_minimal};
    use crate::audit::AuditWriter;
    use crate::repository::{MemoryAdministratorStore, MemoryEgressStore};
    use crate::service::{AdministratorService, EgressService};

    fn app(token: Option<&str>) -> axum::Router {
        let store = Arc::new(MemoryAdministratorStore::new());
        let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
        let admin = Arc::new(
            AdministratorService::new(
                store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
                store.clone() as Arc<dyn crate::repository::SettingsRepository>,
                AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
                clock.clone(),
                Duration::from_secs(3600),
            )
            .with_bootstrap_token(token.map(str::to_string)),
        );
        let egress = Arc::new(EgressService::new(
            Arc::new(MemoryEgressStore::with_local_direct())
                as Arc<dyn crate::repository::EgressRepository>,
            Arc::new(MemorySecretStore::new()) as Arc<dyn crate::adapters::SecretStore>,
            AuditWriter::new(store as Arc<dyn crate::repository::AuditRepository>),
            clock,
        ));
        admin_router_minimal(
            admin,
            egress,
            JobsState {
                repo: Arc::new(MemoryRepo::new()),
            },
        )
    }

    async fn post_bootstrap(app: &axum::Router, body: serde_json::Value) -> (StatusCode, String) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/bootstrap")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    #[tokio::test]
    async fn http_bootstrap_disabled_without_configured_token() {
        let app = app(None);
        let (status, body) = post_bootstrap(
            &app,
            serde_json::json!({"username":"admin","password":"Bootstrap9!","bootstrap_token":"x"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("admin.bootstrap_disabled"), "{body}");
    }

    #[tokio::test]
    async fn http_bootstrap_checks_token_and_rejects_second_admin() {
        let app = app(Some("init-token-123"));
        let (status, body) = post_bootstrap(
            &app,
            serde_json::json!({"username":"admin","password":"Bootstrap9!"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("admin.bootstrap_token_invalid"), "{body}");

        let (status, body) = post_bootstrap(
            &app,
            serde_json::json!({"username":"admin","password":"Bootstrap9!","bootstrap_token":"wrong"}),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("admin.bootstrap_token_invalid"), "{body}");

        let (status, body) = post_bootstrap(
            &app,
            serde_json::json!({"username":"admin","password":"Bootstrap9!","bootstrap_token":"init-token-123"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("\"must_change_password\":true"), "{body}");

        let (status, body) = post_bootstrap(
            &app,
            serde_json::json!({"username":"admin2","password":"Bootstrap9!","bootstrap_token":"init-token-123"}),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("admin.already_bootstrapped"), "{body}");
    }
}
