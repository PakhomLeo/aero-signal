//! 采集登录账号 Admin API。

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use server_core::AppError;
use uuid::Uuid;

use crate::domain::collector::CollectorAccountPublic;
use crate::service::qr_login::{
    AdvanceQrSessionCommand, CreateQrSessionCommand, PatchCollectorAccountCommand,
    QrSessionResponse, ReauthorizeCommand,
};

use super::AdminState;
use super::auth::AdminAuth;

pub fn collector_accounts_routes(enable_dev_routes: bool) -> Router<AdminState> {
    let routes = Router::new()
        .route(
            "/collector-accounts",
            get(list_collector_accounts).patch(patch_collector_account_batch),
        )
        .route(
            "/collector-accounts/{id}",
            axum::routing::patch(patch_collector_account),
        )
        .route("/collector-accounts/qr-sessions", post(create_qr_session))
        .route(
            "/collector-accounts/qr-sessions/{ticket}",
            get(poll_qr_session).delete(cancel_qr_session),
        )
        .route("/collector-accounts/{id}/probe-now", post(probe_now))
        .route("/collector-accounts/{id}/reauthorize", post(reauthorize));
    if enable_dev_routes {
        routes.route(
            "/collector-accounts/qr-sessions/{ticket}/advance",
            post(advance_qr_session),
        )
    } else {
        routes
    }
}

async fn create_qr_session(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<CreateQrSessionCommand>,
) -> Result<Json<QrSessionResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let resp = collector.create_qr_session(&auth, body).await?;
    Ok(Json(resp))
}

async fn poll_qr_session(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(ticket): Path<String>,
) -> Result<Json<QrSessionResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let resp = collector.poll_qr_session(&auth, &ticket).await?;
    Ok(Json(resp))
}

async fn cancel_qr_session(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(ticket): Path<String>,
) -> Result<Json<QrSessionResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let resp = collector.cancel_qr_session(&auth, &ticket).await?;
    Ok(Json(resp))
}

/// Fake/dev：推进二维码会话（仅 connector=fake）。
async fn advance_qr_session(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(ticket): Path<String>,
    Json(body): Json<AdvanceQrSessionCommand>,
) -> Result<Json<QrSessionResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let resp = collector.advance_qr_session(&auth, &ticket, body).await?;
    Ok(Json(resp))
}

async fn list_collector_accounts(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<Vec<CollectorAccountPublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let list = collector.list_accounts().await?;
    Ok(Json(list))
}

#[derive(Debug, serde::Deserialize)]
pub struct PatchCollectorWithId {
    pub id: Uuid,
    #[serde(flatten)]
    pub patch: PatchCollectorAccountCommand,
}

async fn patch_collector_account_batch(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<PatchCollectorWithId>,
) -> Result<Json<CollectorAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let p = collector.patch_account(body.id, body.patch).await?;
    Ok(Json(p))
}

async fn patch_collector_account(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchCollectorAccountCommand>,
) -> Result<Json<CollectorAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let p = collector.patch_account(id, body).await?;
    Ok(Json(p))
}

async fn probe_now(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<CollectorAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let p = collector.probe_now(id).await?;
    Ok(Json(p))
}

async fn reauthorize(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReauthorizeCommand>,
) -> Result<Json<CollectorAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let collector = state
        .collector
        .as_ref()
        .ok_or_else(|| AppError::internal("collector-login 服务未配置"))?;
    let p = collector.reauthorize(id, body).await?;
    Ok(Json(p))
}
