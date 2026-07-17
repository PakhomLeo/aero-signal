//! 消费端 Delivery API（API Token 鉴权，非 admin session）。

mod articles;
mod auth;
mod health;
mod sources;

pub use auth::{DeliveryAuth, DeliveryState, extract_client_ip};

use std::sync::Arc;

use axum::Router;

use crate::service::{
    ApiTokenService, ArticleCursorService, DeliveryGroupService, HealthDeliveryService,
    SourceValidationService,
};

use self::articles::articles_routes;
use self::health::health_routes;
use self::sources::sources_routes;

/// 消费端路由（挂到 `/api`）。
pub fn delivery_router(
    tokens: Arc<ApiTokenService>,
    groups: Arc<DeliveryGroupService>,
    articles: Arc<ArticleCursorService>,
    health: Arc<HealthDeliveryService>,
    sources: Option<Arc<SourceValidationService>>,
) -> Router {
    let state = DeliveryState {
        tokens,
        groups,
        articles,
        health,
        sources,
    };
    Router::new()
        .merge(articles_routes())
        .merge(sources_routes())
        .merge(health_routes())
        .with_state(state)
}
