//! 出口与代理供应商 Admin API。

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use server_core::AppError;
use uuid::Uuid;

use crate::adapters::proxy_provider::EgressTestResult;
use crate::domain::egress::{EgressProfilePublic, ProxyProviderPublic};
use crate::service::egress::{
    CreateEgressProfileCommand, CreateProxyProviderCommand, PatchEgressProfileCommand,
    PatchProxyProviderCommand, TestEgressCommand,
};

use super::AdminState;
use super::auth::AdminAuth;

pub fn egress_settings_routes() -> Router<AdminState> {
    Router::new()
        .route(
            "/settings/egress-profiles",
            get(list_egress_profiles)
                .post(create_egress_profile)
                .patch(patch_egress_profile_batch),
        )
        .route(
            "/settings/egress-profiles/{id}",
            axum::routing::patch(patch_egress_profile),
        )
        .route(
            "/settings/egress-profiles/{id}/test",
            post(test_egress_profile),
        )
        .route(
            "/settings/proxy-providers",
            get(list_proxy_providers)
                .post(create_proxy_provider)
                .patch(patch_proxy_provider_batch),
        )
        .route(
            "/settings/proxy-providers/{id}",
            axum::routing::patch(patch_proxy_provider),
        )
}

async fn list_egress_profiles(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<Vec<EgressProfilePublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let list = state.egress.list_egress_profiles().await?;
    Ok(Json(list))
}

async fn create_egress_profile(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<CreateEgressProfileCommand>,
) -> Result<Json<EgressProfilePublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state.egress.create_egress_profile(&auth, body).await?;
    Ok(Json(p))
}

/// PATCH 集合：body 必须含 id（与 BACKEND 的 PATCH 集合路径对齐）。
#[derive(Debug, Deserialize)]
pub struct PatchEgressProfileWithId {
    pub id: Uuid,
    #[serde(flatten)]
    pub patch: PatchEgressProfileCommand,
}

async fn patch_egress_profile_batch(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<PatchEgressProfileWithId>,
) -> Result<Json<EgressProfilePublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state
        .egress
        .patch_egress_profile(&auth, body.id, body.patch)
        .await?;
    Ok(Json(p))
}

async fn patch_egress_profile(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchEgressProfileCommand>,
) -> Result<Json<EgressProfilePublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state.egress.patch_egress_profile(&auth, id, body).await?;
    Ok(Json(p))
}

async fn test_egress_profile(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    body: Option<Json<TestEgressCommand>>,
) -> Result<Json<EgressTestResult>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let cmd = body.map(|j| j.0).unwrap_or_default();
    let r = state.egress.test_egress_profile(&auth, id, cmd).await?;
    Ok(Json(r))
}

async fn list_proxy_providers(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<Vec<ProxyProviderPublic>>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let list = state.egress.list_proxy_providers().await?;
    Ok(Json(list))
}

async fn create_proxy_provider(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<CreateProxyProviderCommand>,
) -> Result<Json<ProxyProviderPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state.egress.create_proxy_provider(&auth, body).await?;
    Ok(Json(p))
}

#[derive(Debug, Deserialize)]
pub struct PatchProxyProviderWithId {
    pub id: Uuid,
    #[serde(flatten)]
    pub patch: PatchProxyProviderCommand,
}

async fn patch_proxy_provider_batch(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<PatchProxyProviderWithId>,
) -> Result<Json<ProxyProviderPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state
        .egress
        .patch_proxy_provider(&auth, body.id, body.patch)
        .await?;
    Ok(Json(p))
}

async fn patch_proxy_provider(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchProxyProviderCommand>,
) -> Result<Json<ProxyProviderPublic>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let p = state.egress.patch_proxy_provider(&auth, id, body).await?;
    Ok(Json(p))
}
