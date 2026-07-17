//! `GET /api/sources`

use std::time::Instant;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use server_core::AppError;
use uuid::Uuid;

use super::auth::{DeliveryState, RequireSourcesRead};
use crate::domain::delivery::ApiScope;
use crate::domain::source::SourceStatus;

pub fn sources_routes() -> Router<DeliveryState> {
    Router::new().route("/sources", get(list_sources))
}

#[derive(Debug, Serialize)]
pub struct DeliverySourceSummary {
    pub id: Uuid,
    pub display_name: String,
    pub status: SourceStatus,
    pub preferred_connector: Option<String>,
}

async fn list_sources(
    State(state): State<DeliveryState>,
    RequireSourcesRead(auth): RequireSourcesRead,
) -> Result<Json<Vec<DeliverySourceSummary>>, AppError> {
    let started = Instant::now();
    let source_svc = state
        .sources
        .as_ref()
        .ok_or_else(|| AppError::internal("source 服务未配置"))?;
    let ids = state
        .groups
        .list_source_ids(auth.auth.token.delivery_group_id)
        .await?;
    let mut out = Vec::new();
    for id in ids {
        if let Ok(s) = source_svc.get(id).await {
            out.push(DeliverySourceSummary {
                id: s.id,
                display_name: s.display_name,
                status: s.status,
                preferred_connector: s.preferred_connector,
            });
        }
    }
    let _ = state
        .tokens
        .log_call(
            &auth.auth,
            "GET",
            "/api/sources",
            Some(ApiScope::SourcesRead.as_str()),
            200,
            started.elapsed().as_millis() as i32,
            0,
            auth.client_ip.as_deref(),
        )
        .await;
    Ok(Json(out))
}
