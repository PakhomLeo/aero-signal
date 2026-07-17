//! HTTP / Worker 运行时装配（基于 AppHandles）。

use axum::Router;
use axum::middleware;

use crate::api::admin::{JobsState, admin_router_with_delivery_for_environment};
use crate::api::delivery::delivery_router;
use crate::app::AppHandles;
use server_core::healthz_router;

/// 由 AppHandles 构建完整 HTTP 路由（不含内嵌 Worker）。
pub fn build_http_router(handles: &AppHandles) -> Router {
    let admin_routes = Router::new().nest(
        "/api/admin",
        admin_router_with_delivery_for_environment(
            handles.admin.clone(),
            handles.egress.clone(),
            JobsState {
                repo: handles.jobs_repo.clone(),
            },
            Some(handles.collector.clone()),
            Some(handles.source.clone()),
            Some(handles.preflight.clone()),
            Some(handles.daily_plan.clone()),
            Some(handles.manual_edit.clone()),
            Some(handles.delivery_groups.clone()),
            Some(handles.api_tokens.clone()),
            handles.config.environment.allows_fake(),
        ),
    );

    let delivery = Router::new().nest(
        "/api",
        delivery_router(
            handles.api_tokens.clone(),
            handles.delivery_groups.clone(),
            handles.article_cursor.clone(),
            handles.health_delivery.clone(),
            Some(handles.source.clone()),
        ),
    );

    healthz_router(handles.config.product.clone())
        .merge(admin_routes)
        .merge(delivery)
        .layer(middleware::from_fn(server_core::trace::inject_trace_id))
}

/// 仅 memory 联调：在当前进程 spawn Worker。生产应使用独立 `signal-worker`。
pub fn maybe_embed_worker(handles: &AppHandles) {
    if !handles.config.embed_worker {
        return;
    }
    if handles.store_mode != server_core::StoreMode::Memory {
        tracing::warn!(
            store = handles.store_mode.as_str(),
            "SIGNAL_EMBED_WORKER 在非 memory 模式下被忽略；请使用独立 signal-worker"
        );
        return;
    }
    let worker = handles.build_worker("memory-embed-worker");
    tokio::spawn(async move {
        worker.run_forever().await;
    });
    tracing::info!("已内嵌 memory Worker（SIGNAL_EMBED_WORKER）；生产请改用 signal-worker");
}
