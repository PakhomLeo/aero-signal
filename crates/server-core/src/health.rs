//! 进程级健康检查（非业务日快照）。

use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

/// 健康状态。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthStatus {
    pub status: String,
    pub product: String,
}

impl HealthStatus {
    pub fn ok(product: impl Into<String>) -> Self {
        Self {
            status: "ok".to_string(),
            product: product.into(),
        }
    }
}

/// `GET /healthz`：进程存活。
pub async fn healthz() -> Json<HealthStatus> {
    Json(HealthStatus::ok("signal"))
}

/// 挂载进程级健康路由。
pub fn healthz_router(product: impl Into<String>) -> Router {
    let product = product.into();
    Router::new().route(
        "/healthz",
        get(move || {
            let product = product.clone();
            async move { Json(HealthStatus::ok(product)) }
        }),
    )
}

/// `GET /readyz` 占位：M01 仅表示进程可接受请求（依赖检查留给后续）。
pub async fn readyz() -> Json<HealthStatus> {
    Json(HealthStatus::ok("signal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    #[tokio::test]
    async fn healthz_returns_ok_json() {
        let app = healthz_router("signal");
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let status: HealthStatus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.status, "ok");
        assert_eq!(status.product, "signal");
    }
}
