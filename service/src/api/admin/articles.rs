//! 文章 Admin API：GET/PATCH content、reocr、refetch。

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use server_core::AppError;
use uuid::Uuid;

use crate::service::manual_edit::{
    ArticleDetailDto, PatchArticleContentCommand, RefetchCommand, RefetchResponse, ReocrCommand,
    ReocrResponse,
};

use super::AdminState;
use super::auth::AdminAuth;

pub fn articles_routes() -> Router<AdminState> {
    Router::new()
        .route("/articles/{id}", get(get_article))
        .route(
            "/articles/{id}/content",
            axum::routing::patch(patch_content),
        )
        .route("/articles/{id}/reocr", post(reocr))
        .route("/articles/{id}/refetch", post(refetch))
        .route(
            "/articles/{id}/revisions/{revision}/restore",
            post(restore_revision),
        )
}

async fn get_article(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<ArticleDetailDto>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .manual_edit
        .as_ref()
        .ok_or_else(|| AppError::internal("content-normalizer 服务未配置"))?;
    Ok(Json(svc.get(id).await?))
}

async fn patch_content(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchArticleContentCommand>,
) -> Result<Json<ArticleDetailDto>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .manual_edit
        .as_ref()
        .ok_or_else(|| AppError::internal("content-normalizer 服务未配置"))?;
    Ok(Json(svc.patch_content(id, body).await?))
}

async fn reocr(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<ReocrCommand>,
) -> Result<Json<ReocrResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .manual_edit
        .as_ref()
        .ok_or_else(|| AppError::internal("content-normalizer 服务未配置"))?;
    Ok(Json(svc.reocr(id, body).await?))
}

async fn refetch(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    body: Option<Json<RefetchCommand>>,
) -> Result<Json<RefetchResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .manual_edit
        .as_ref()
        .ok_or_else(|| AppError::internal("content-normalizer 服务未配置"))?;
    Ok(Json(
        svc.refetch(id, body.map(|Json(value)| value).unwrap_or_default())
            .await?,
    ))
}

async fn restore_revision(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path((id, revision)): Path<(Uuid, i32)>,
) -> Result<Json<ArticleDetailDto>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .manual_edit
        .as_ref()
        .ok_or_else(|| AppError::internal("content-normalizer 服务未配置"))?;
    Ok(Json(svc.restore_revision(id, revision).await?))
}
