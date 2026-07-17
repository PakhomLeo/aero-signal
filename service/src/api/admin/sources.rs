//! 监测公众号 Admin API。

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use server_core::AppError;
use uuid::Uuid;

use crate::domain::source::SourceAccountPublic;
use crate::service::source_validation::{
    ConfirmSourceCommand, FetchNowCommand, FetchNowResponse, PatchSourceCommand,
    ValidateSourceCommand, ValidateSourceResponse,
};

use super::AdminState;
use super::auth::AdminAuth;

pub fn sources_routes() -> Router<AdminState> {
    Router::new()
        .route("/sources/validate", post(validate_source))
        .route("/source-validations/{id}", get(get_validation_run))
        .route("/sources", post(confirm_source).get(list_sources))
        .route("/sources/{id}", get(get_source).patch(patch_source))
        .route("/sources/{id}/verify-again", post(verify_again))
        .route("/sources/{id}/fetch-now", post(fetch_now))
}

async fn get_validation_run(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<crate::domain::source::SourceValidationRun>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.get_validation_run(id).await?))
}

async fn validate_source(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<ValidateSourceCommand>,
) -> Result<Json<ValidateSourceResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    let resp = svc.validate(body).await?;
    Ok(Json(resp))
}

async fn confirm_source(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<ConfirmSourceCommand>,
) -> Result<Json<SourceAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    let resp = svc.confirm(body).await?;
    Ok(Json(resp))
}

async fn list_sources(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<Vec<SourceAccountPublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.list().await?))
}

async fn get_source(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<SourceAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.get(id).await?))
}

async fn patch_source(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchSourceCommand>,
) -> Result<Json<SourceAccountPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.patch(id, body).await?))
}

#[derive(Debug, serde::Deserialize)]
pub struct VerifyAgainBody {
    pub url: Option<String>,
}

async fn verify_again(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<VerifyAgainBody>,
) -> Result<Json<ValidateSourceResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.verify_again(id, body.url).await?))
}

async fn fetch_now(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(cmd): Json<FetchNowCommand>,
) -> Result<Json<FetchNowResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .source
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    Ok(Json(svc.fetch_now(id, cmd).await?))
}
