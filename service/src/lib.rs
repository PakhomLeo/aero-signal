//! Aero Signal 主服务库。

pub mod adapters;
pub mod api;
pub mod app;
pub mod audit;
pub mod clock;
pub mod domain;
pub mod dto;
pub mod error;
pub mod idempotency;
pub mod jobs;
pub mod logging_redact;
pub mod repository;
pub mod runtime;
pub mod scheduler;
pub mod service;

pub use server_core::{
    AppState, ServiceConfig, StoreMode, SystemClock, TRACE_ID_HEADER, healthz_router,
    inject_trace_id,
};

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use server_core::Clock;

use crate::adapters::secret_store::MemorySecretStore;
use crate::api::admin::{JobsState, admin_router_with_delivery_for_environment};
use crate::api::delivery::delivery_router;
use crate::repository::{
    AdministratorRepository, AuditRepository, MemoryAdministratorStore, MemoryEgressStore,
    SettingsRepository,
};
use crate::runtime::{build_http_router, maybe_embed_worker};
use crate::service::{
    AdministratorService, ApiTokenService, ArticleCursorService, DailyPlanService,
    DailyPreflightService, DeliveryGroupService, EgressService, HealthDeliveryService,
    ManualEditService, QrLoginService, SourceValidationService,
};

/// 构建 HTTP 路由（内存仓储）。
///
/// 若配置 `embed_worker=true`（`for_test_listen` / `SIGNAL_EMBED_WORKER=1`），
/// 会在当前进程内嵌 Worker，便于单进程假登录联调。生产请用独立 `signal-worker`。
pub fn build_router(config: &ServiceConfig) -> Router {
    let mut cfg = config.clone();
    cfg.store_mode = StoreMode::Memory;
    let handles = crate::app::build_memory_app_for_tests(&cfg);
    maybe_embed_worker(&handles);
    build_http_router(&handles)
}

/// 测试用：注入采集登录服务（不含 delivery 消费 API）。
pub fn build_router_with_collector(
    config: &ServiceConfig,
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs_repo: Arc<dyn job_pipeline::JobRepository>,
    collector: Option<Arc<QrLoginService>>,
) -> Router {
    build_router_with_delivery(
        config, admin, egress, jobs_repo, collector, None, None, None, None, None, None, None, None,
    )
}

/// 测试用：可注入 delivery 消费与管理服务。
#[allow(clippy::too_many_arguments)]
pub fn build_router_with_delivery(
    config: &ServiceConfig,
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs_repo: Arc<dyn job_pipeline::JobRepository>,
    collector: Option<Arc<QrLoginService>>,
    source: Option<Arc<SourceValidationService>>,
    health: Option<Arc<DailyPreflightService>>,
    daily_plan: Option<Arc<DailyPlanService>>,
    manual_edit: Option<Arc<ManualEditService>>,
    delivery_groups: Option<Arc<DeliveryGroupService>>,
    api_tokens: Option<Arc<ApiTokenService>>,
    article_cursor: Option<Arc<ArticleCursorService>>,
    health_delivery: Option<Arc<HealthDeliveryService>>,
) -> Router {
    use axum::middleware;

    let admin_routes = Router::new().nest(
        "/api/admin",
        admin_router_with_delivery_for_environment(
            admin,
            egress,
            JobsState { repo: jobs_repo },
            collector,
            source.clone(),
            health,
            daily_plan,
            manual_edit,
            delivery_groups.clone(),
            api_tokens.clone(),
            config.environment.allows_fake(),
        ),
    );

    let mut app = healthz_router(config.product.clone()).merge(admin_routes);

    if let (Some(tokens), Some(groups), Some(cursor), Some(hdel)) =
        (api_tokens, delivery_groups, article_cursor, health_delivery)
    {
        app = app.merge(Router::new().nest(
            "/api",
            delivery_router(tokens, groups, cursor, hdel, source),
        ));
    }

    app.layer(middleware::from_fn(server_core::trace::inject_trace_id))
}

/// 兼容旧测试：仅注入 job 仓储（管理员为内存空库）。
pub fn build_router_with_jobs(
    config: &ServiceConfig,
    repo: Arc<dyn job_pipeline::JobRepository>,
) -> Router {
    let store = Arc::new(MemoryAdministratorStore::new());
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let admin = Arc::new(AdministratorService::new(
        store.clone() as Arc<dyn AdministratorRepository>,
        store.clone() as Arc<dyn SettingsRepository>,
        crate::audit::AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
        Duration::from_secs(12 * 60 * 60),
    ));
    let egress_store = Arc::new(MemoryEgressStore::with_local_direct());
    let secrets = Arc::new(MemorySecretStore::new());
    let egress = Arc::new(EgressService::new(
        egress_store as Arc<dyn crate::repository::EgressRepository>,
        secrets as Arc<dyn crate::adapters::SecretStore>,
        crate::audit::AuditWriter::new(store as Arc<dyn AuditRepository>),
        clock,
    ));
    build_router_with_collector(config, admin, egress, repo, None)
}
