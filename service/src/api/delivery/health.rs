//! `GET /api/service-health/latest`、`GET /api/service-health?date=`

use std::time::Instant;

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use chrono::NaiveDate;
use serde::Deserialize;
use server_core::AppError;

use super::auth::{DeliveryState, RequireServiceHealthRead};
use crate::domain::delivery::ApiScope;
use crate::service::health_delivery::HealthSnapshotDeliveryDto;

pub fn health_routes() -> Router<DeliveryState> {
    Router::new()
        .route("/service-health/latest", get(latest))
        .route("/service-health", get(by_date))
}

#[derive(Debug, Deserialize)]
pub struct DateQuery {
    pub date: String,
}

async fn latest(
    State(state): State<DeliveryState>,
    RequireServiceHealthRead(auth): RequireServiceHealthRead,
) -> Result<Json<HealthSnapshotDeliveryDto>, AppError> {
    let started = Instant::now();
    let result = state.health.latest().await;
    let (status, redactions) = match &result {
        Ok(dto) => (200, estimate_redactions(dto)),
        Err(e) => (e.status.as_u16() as i32, 0),
    };
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            "/api/service-health/latest",
            Some(ApiScope::ServiceHealthRead.as_str()),
            status,
            started.elapsed().as_millis() as i32,
            redactions,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(result?))
}

async fn by_date(
    State(state): State<DeliveryState>,
    RequireServiceHealthRead(auth): RequireServiceHealthRead,
    Query(q): Query<DateQuery>,
) -> Result<Json<HealthSnapshotDeliveryDto>, AppError> {
    let started = Instant::now();
    let date = NaiveDate::parse_from_str(&q.date, "%Y-%m-%d")
        .map_err(|_| AppError::new("health.invalid_date", "date 须为 YYYY-MM-DD"))?;
    let result = state.health.by_date(date).await;
    let (status, redactions) = match &result {
        Ok(dto) => (200, estimate_redactions(dto)),
        Err(e) => (e.status.as_u16() as i32, 0),
    };
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            "/api/service-health",
            Some(ApiScope::ServiceHealthRead.as_str()),
            status,
            started.elapsed().as_millis() as i32,
            redactions,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(result?))
}

fn estimate_redactions(dto: &HealthSnapshotDeliveryDto) -> i32 {
    // 调用日志只记脱敏计数；DTO 已脱敏，此处用组件数作保守计数占位。
    dto.components.len() as i32
}
