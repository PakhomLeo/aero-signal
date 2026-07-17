//! `GET/PATCH /api/admin/me`

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use server_core::AppError;

use crate::domain::administrator::AdministratorPublic;
use crate::service::PatchMeCommand;

use super::AdminState;
use super::auth::AdminAuth;

#[derive(Debug, Deserialize)]
pub struct PatchMeRequest {
    pub username: Option<String>,
    pub current_password: Option<String>,
    pub new_password: Option<String>,
}

pub fn me_routes() -> Router<AdminState> {
    Router::new().route("/me", get(get_me).patch(patch_me))
}

async fn get_me(
    State(state): State<AdminState>,
    AdminAuth(_auth): AdminAuth,
) -> Result<Json<AdministratorPublic>, AppError> {
    let me = state.admin.get_me().await?;
    Ok(Json(me))
}

async fn patch_me(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<PatchMeRequest>,
) -> Result<Json<AdministratorPublic>, AppError> {
    let me = state
        .admin
        .patch_me(
            &auth,
            PatchMeCommand {
                username: body.username,
                current_password: body.current_password,
                new_password: body.new_password,
            },
        )
        .await?;
    Ok(Json(me))
}
