//! Admin：`/api/admin/delivery-groups` CRUD 与 Token 签发/撤销。

use axum::extract::{Path, State};
use axum::routing::{delete, get};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use server_core::AppError;
use uuid::Uuid;

use super::AdminState;
use super::auth::AdminAuth;
use crate::domain::delivery::{ApiScope, ApiTokenPublic, DeliveryGroupPublic, DeliveryGroupStatus};
use crate::service::api_token::IssueTokenCommand;
use crate::service::delivery_group::{CreateGroupCommand, PatchGroupCommand};

pub fn delivery_groups_routes() -> Router<AdminState> {
    Router::new()
        .route("/delivery-groups", get(list_groups).post(create_group))
        .route("/delivery-groups/{id}", get(get_group).patch(patch_group))
        .route(
            "/delivery-groups/{id}/tokens",
            get(list_tokens).post(issue_token),
        )
        .route(
            "/delivery-groups/{id}/tokens/{token_id}",
            delete(revoke_token),
        )
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupBody {
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub source_account_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct PatchGroupBody {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<DeliveryGroupStatus>,
    pub source_account_ids: Option<Vec<Uuid>>,
}

#[derive(Debug, Deserialize)]
pub struct IssueTokenBody {
    pub name: String,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub ip_allowlist: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
pub struct IssueTokenResponse {
    pub token: ApiTokenPublic,
    /// 明文仅此一次返回。
    pub plaintext: String,
}

async fn list_groups(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<Vec<DeliveryGroupPublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .delivery
        .as_ref()
        .ok_or_else(|| AppError::internal("delivery 服务未配置"))?;
    Ok(Json(svc.list().await?))
}

async fn create_group(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<CreateGroupBody>,
) -> Result<Json<DeliveryGroupPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .delivery
        .as_ref()
        .ok_or_else(|| AppError::internal("delivery 服务未配置"))?;
    Ok(Json(
        svc.create(CreateGroupCommand {
            name: body.name,
            description: body.description,
            source_account_ids: body.source_account_ids,
        })
        .await?,
    ))
}

async fn get_group(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<DeliveryGroupPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .delivery
        .as_ref()
        .ok_or_else(|| AppError::internal("delivery 服务未配置"))?;
    Ok(Json(svc.get(id).await?))
}

async fn patch_group(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchGroupBody>,
) -> Result<Json<DeliveryGroupPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let svc = state
        .delivery
        .as_ref()
        .ok_or_else(|| AppError::internal("delivery 服务未配置"))?;
    Ok(Json(
        svc.patch(
            id,
            PatchGroupCommand {
                name: body.name,
                description: body.description,
                status: body.status,
                source_account_ids: body.source_account_ids,
            },
        )
        .await?,
    ))
}

async fn list_tokens(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<ApiTokenPublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let tokens = state
        .api_tokens
        .as_ref()
        .ok_or_else(|| AppError::internal("api token 服务未配置"))?;
    // 确认分组存在
    let groups = state
        .delivery
        .as_ref()
        .ok_or_else(|| AppError::internal("delivery 服务未配置"))?;
    let _ = groups.get(id).await?;
    Ok(Json(tokens.list_for_group(id).await?))
}

async fn issue_token(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<IssueTokenBody>,
) -> Result<Json<IssueTokenResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let tokens = state
        .api_tokens
        .as_ref()
        .ok_or_else(|| AppError::internal("api token 服务未配置"))?;
    let mut scopes = Vec::new();
    for s in &body.scopes {
        scopes.push(ApiScope::parse(s).map_err(|e| AppError::new("token.invalid_scope", e))?);
    }
    let issued = tokens
        .issue(IssueTokenCommand {
            delivery_group_id: id,
            name: body.name,
            scopes,
            ip_allowlist: body.ip_allowlist,
            expires_at: body.expires_at,
        })
        .await?;
    let now = chrono::Utc::now();
    Ok(Json(IssueTokenResponse {
        token: ApiTokenPublic::from_token(&issued.token, now),
        plaintext: issued.plaintext.into_inner(),
    }))
}

async fn revoke_token(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path((_group_id, token_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiTokenPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let tokens = state
        .api_tokens
        .as_ref()
        .ok_or_else(|| AppError::internal("api token 服务未配置"))?;
    Ok(Json(tokens.revoke(token_id).await?))
}
