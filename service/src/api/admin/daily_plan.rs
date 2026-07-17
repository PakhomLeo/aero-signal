//! `GET /api/admin/daily-plan?date=`
//! `POST /api/admin/daily-plan/rebuild`

use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use server_core::AppError;

use super::AdminState;
use super::auth::AdminAuth;
use crate::service::daily_plan::DailyPlanListDto;

pub fn daily_plan_routes() -> Router<AdminState> {
    Router::new()
        .route("/daily-plan", get(get_daily_plan))
        .route("/daily-plan/rebuild", post(rebuild_daily_plan))
}

#[derive(Debug, Deserialize)]
pub struct DailyPlanQuery {
    /// YYYY-MM-DD；缺省为业务日。
    pub date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RebuildBody {
    /// YYYY-MM-DD；缺省为业务日。
    pub date: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct RebuildResponse {
    pub job_id: String,
    pub kind: String,
    pub idempotency_key: Option<String>,
    pub plan_date: String,
    pub rebuild: bool,
}

async fn get_daily_plan(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Query(q): Query<DailyPlanQuery>,
) -> Result<Json<DailyPlanListDto>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let plan = state
        .daily_plan
        .as_ref()
        .ok_or_else(|| AppError::internal("daily-plan 服务未配置"))?;
    let date = resolve_date(plan, q.date.as_deref()).await?;
    let dto = plan.list_for_date(date).await.map_err(AppError::internal)?;
    Ok(Json(dto))
}

async fn rebuild_daily_plan(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<RebuildBody>,
) -> Result<Json<RebuildResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let plan = state
        .daily_plan
        .as_ref()
        .ok_or_else(|| AppError::internal("daily-plan 服务未配置"))?;
    let date = resolve_date(plan, body.date.as_deref()).await?;
    let job = plan
        .enqueue_build(date, true)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(RebuildResponse {
        job_id: job.id.to_string(),
        kind: job.kind,
        idempotency_key: job.idempotency_key,
        plan_date: date.format("%Y-%m-%d").to_string(),
        rebuild: true,
    }))
}

async fn resolve_date(
    plan: &crate::service::daily_plan::DailyPlanService,
    raw: Option<&str>,
) -> Result<NaiveDate, AppError> {
    match raw {
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| AppError::new("daily_plan.bad_date", "date 须为 YYYY-MM-DD")),
        None => plan.business_date().await.map_err(AppError::internal),
    }
}
