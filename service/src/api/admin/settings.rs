//! `GET/PATCH /api/admin/settings/schedule`

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use server_core::AppError;

use crate::domain::ScheduleSettings;

use super::AdminState;
use super::auth::AdminAuth;

pub fn settings_routes() -> Router<AdminState> {
    Router::new().route(
        "/settings/schedule",
        get(get_schedule).patch(patch_schedule),
    )
}

async fn get_schedule(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<ScheduleSettings>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let s = state.admin.get_schedule().await?;
    Ok(Json(s))
}

async fn patch_schedule(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<ScheduleSettings>,
) -> Result<Json<ScheduleSettings>, AppError> {
    let s = state.admin.update_schedule(&auth, body).await?;
    Ok(Json(s))
}
