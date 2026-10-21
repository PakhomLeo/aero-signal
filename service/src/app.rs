//! Composition root：按 StoreMode 装配仓储与服务。

use std::sync::Arc;
use std::time::Duration;

use job_pipeline::{JobHandlerRegistry, JobRepository, MemoryRepo, Worker};
use server_core::{AppError, Clock, ServiceConfig, StoreMode, SystemClock};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::adapters::DefaultProxyProviderAdapter;
use crate::adapters::factory::{RuntimeAdapters, build_memory_adapters, build_runtime_adapters};
use crate::audit::AuditWriter;
use crate::jobs::{NormalizerJobDeps, signal_job_registry_with};
use crate::repository::{
    AdministratorRepository, ArticleRepository, AuditRepository, CollectorRepository,
    DailyPlanRepository, DeliveryRepository, EgressRepository, HealthSnapshotRepository,
    MemoryAdministratorStore, MemoryArticleStore, MemoryCollectorStore, MemoryDailyPlanStore,
    MemoryDeliveryStore, MemoryEgressStore, MemoryHealthStore, MemoryRetentionStore,
    MemorySourceStore, PgAdministratorRepository, PgArticleRepository, PgCollectorRepository,
    PgDailyPlanRepository, PgDeliveryRepository, PgEgressRepository, PgHealthSnapshotRepository,
    PgRetentionRepository, PgSourceRepository, RetentionRepository, SettingsRepository,
    SourceRepository,
};
use crate::service::{
    AdministratorService, ApiTokenService, ArticleCaptureService, ArticleCursorService,
    CollectorConnectorResolver, ContentNormalizerService, DailyPlanService, DailyPreflightService,
    DefaultHealthComponentChecker, DeliveryGroupService, DynamicLeaseManager, EgressResolver,
    EgressService, HealthDeliveryService, ManualEditService, OcrJobPlanner, QrLoginService,
    RetentionCleanupService, RevisionService, SourceValidationService,
};
use crawler_capture::{CdpBrowser, ObjectStore};
use platform_connectors::registry_for_environment;

/// 装配后的应用句柄（HTTP 与 Worker 共用）。
pub struct AppHandles {
    pub config: ServiceConfig,
    pub store_mode: StoreMode,
    pub clock: Arc<dyn Clock>,
    pub jobs_repo: Arc<dyn JobRepository>,
    pub admin: Arc<AdministratorService>,
    pub egress: Arc<EgressService>,
    pub collector: Arc<QrLoginService>,
    pub source: Arc<SourceValidationService>,
    pub preflight: Arc<DailyPreflightService>,
    pub daily_plan: Arc<DailyPlanService>,
    pub manual_edit: Arc<ManualEditService>,
    pub delivery_groups: Arc<DeliveryGroupService>,
    pub api_tokens: Arc<ApiTokenService>,
    pub article_cursor: Arc<ArticleCursorService>,
    pub health_delivery: Arc<HealthDeliveryService>,
    pub retention: Arc<RetentionCleanupService>,
    pub capture: Arc<ArticleCaptureService>,
    pub normalizer: Arc<ContentNormalizerService>,
    pub revisions: Arc<RevisionService>,
    pub articles: Arc<dyn ArticleRepository>,
    pub job_registry: JobHandlerRegistry,
}

impl AppHandles {
    /// 构建 job Worker（不启动）。
    pub fn build_worker(&self, worker_id: &str) -> Worker {
        Worker::new(worker_id, self.jobs_repo.clone())
            .with_poll_interval(1)
            .with_registry(&self.job_registry)
    }
}

/// 按配置装配全部服务与 job registry。
pub async fn build_app(config: &ServiceConfig) -> Result<AppHandles, AppError> {
    config.validate_runtime()?;
    match config.store_mode {
        StoreMode::Memory => Ok(build_memory_app(config)),
        StoreMode::Postgres => build_postgres_app(config).await,
    }
}

/// 同步 memory 装配（供 `build_router` / 测试）。
pub fn build_memory_app_for_tests(config: &ServiceConfig) -> AppHandles {
    assert!(
        config.environment.allows_fake(),
        "production 不得使用 memory/Fake 测试装配"
    );
    build_memory_app(config)
}

fn build_memory_app(config: &ServiceConfig) -> AppHandles {
    let adapters = build_memory_adapters();
    let store = Arc::new(MemoryAdministratorStore::new());
    let admin_repo = store.clone() as Arc<dyn AdministratorRepository>;
    let settings_repo = store.clone() as Arc<dyn SettingsRepository>;
    let audit_repo = store.clone() as Arc<dyn AuditRepository>;
    let egress_repo = Arc::new(MemoryEgressStore::with_local_direct()) as Arc<dyn EgressRepository>;
    let collector_repo = Arc::new(MemoryCollectorStore::new()) as Arc<dyn CollectorRepository>;
    let sources = Arc::new(MemorySourceStore::new()) as Arc<dyn SourceRepository>;
    let articles = Arc::new(MemoryArticleStore::new()) as Arc<dyn ArticleRepository>;
    let health_repo = Arc::new(MemoryHealthStore::new()) as Arc<dyn HealthSnapshotRepository>;
    let plan_repo = Arc::new(MemoryDailyPlanStore::new()) as Arc<dyn DailyPlanRepository>;
    let delivery_repo = Arc::new(MemoryDeliveryStore::new()) as Arc<dyn DeliveryRepository>;
    let retention_repo = Arc::new(MemoryRetentionStore::new()) as Arc<dyn RetentionRepository>;
    let jobs_repo = Arc::new(MemoryRepo::new()) as Arc<dyn JobRepository>;

    assemble(
        config.clone(),
        StoreMode::Memory,
        adapters,
        admin_repo,
        settings_repo,
        audit_repo,
        egress_repo,
        collector_repo,
        sources,
        articles,
        health_repo,
        plan_repo,
        delivery_repo,
        retention_repo,
        jobs_repo,
    )
}

async fn build_postgres_app(config: &ServiceConfig) -> Result<AppHandles, AppError> {
    let url = config.database_url.as_deref().ok_or_else(|| {
        AppError::new(
            "config.database_url_required",
            "store_mode=postgres 时必须设置 DATABASE_URL",
        )
    })?;
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(url)
        .await
        .map_err(|e| {
            AppError::new(
                "config.database_connect_failed",
                format!("连接 PostgreSQL 失败: {e}"),
            )
        })?;

    run_migrations(&pool).await?;

    let admin_pg = Arc::new(PgAdministratorRepository::new(pool.clone()));
    let admin_repo = admin_pg.clone() as Arc<dyn AdministratorRepository>;
    let settings_repo = admin_pg.clone() as Arc<dyn SettingsRepository>;
    let audit_repo = admin_pg as Arc<dyn AuditRepository>;
    let egress_repo = Arc::new(PgEgressRepository::new(pool.clone())) as Arc<dyn EgressRepository>;
    let collector_repo =
        Arc::new(PgCollectorRepository::new(pool.clone())) as Arc<dyn CollectorRepository>;
    let sources = Arc::new(PgSourceRepository::new(pool.clone())) as Arc<dyn SourceRepository>;
    let articles = Arc::new(PgArticleRepository::new(pool.clone())) as Arc<dyn ArticleRepository>;
    let health_repo = Arc::new(PgHealthSnapshotRepository::new(pool.clone()))
        as Arc<dyn HealthSnapshotRepository>;
    let plan_repo =
        Arc::new(PgDailyPlanRepository::new(pool.clone())) as Arc<dyn DailyPlanRepository>;
    let delivery_repo =
        Arc::new(PgDeliveryRepository::new(pool.clone())) as Arc<dyn DeliveryRepository>;
    let retention_repo =
        Arc::new(PgRetentionRepository::new(pool.clone())) as Arc<dyn RetentionRepository>;
    let jobs_repo =
        Arc::new(job_pipeline::postgres::PgJobRepository::new(pool)) as Arc<dyn JobRepository>;

    let adapters = build_runtime_adapters(config).await?;

    Ok(assemble(
        config.clone(),
        StoreMode::Postgres,
        adapters,
        admin_repo,
        settings_repo,
        audit_repo,
        egress_repo,
        collector_repo,
        sources,
        articles,
        health_repo,
        plan_repo,
        delivery_repo,
        retention_repo,
        jobs_repo,
    ))
}

async fn run_migrations(pool: &PgPool) -> Result<(), AppError> {
    // 相对仓库根或 CARGO_MANIFEST_DIR 的 migrations
    let candidates = [
        std::path::PathBuf::from("migrations"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../migrations"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../migrations"),
    ];
    let dir = candidates
        .into_iter()
        .find(|p| p.join("20260709000001_placeholder.sql").exists())
        .ok_or_else(|| {
            AppError::new(
                "config.migrations_not_found",
                "找不到 migrations 目录（请在 Signal 仓库根或 service 目录启动）",
            )
        })?;
    let migrator = sqlx::migrate::Migrator::new(dir).await.map_err(|e| {
        AppError::new(
            "config.migrations_load_failed",
            format!("加载迁移失败: {e}"),
        )
    })?;
    migrator
        .run(pool)
        .await
        .map_err(|e| AppError::new("config.migrations_run_failed", format!("执行迁移失败: {e}")))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    config: ServiceConfig,
    store_mode: StoreMode,
    adapters: RuntimeAdapters,
    admin_repo: Arc<dyn AdministratorRepository>,
    settings_repo: Arc<dyn SettingsRepository>,
    audit_repo: Arc<dyn AuditRepository>,
    egress_repo: Arc<dyn EgressRepository>,
    collector_repo: Arc<dyn CollectorRepository>,
    sources: Arc<dyn SourceRepository>,
    articles: Arc<dyn ArticleRepository>,
    health_repo: Arc<dyn HealthSnapshotRepository>,
    plan_repo: Arc<dyn DailyPlanRepository>,
    delivery_repo: Arc<dyn DeliveryRepository>,
    retention_repo: Arc<dyn RetentionRepository>,
    jobs_repo: Arc<dyn JobRepository>,
) -> AppHandles {
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let secrets = adapters.secrets;
    let connectors = Arc::new(registry_for_environment(config.environment.allows_fake()));
    let audit_writer = AuditWriter::new(audit_repo.clone());

    let admin = Arc::new(
        AdministratorService::new(
            admin_repo,
            settings_repo.clone(),
            audit_writer.clone(),
            clock.clone(),
            Duration::from_secs(12 * 60 * 60),
        )
        .with_bootstrap_token(AdministratorService::bootstrap_token_from_env()),
    );
    let egress = Arc::new(EgressService::new(
        egress_repo.clone(),
        secrets.clone(),
        audit_writer.clone(),
        clock.clone(),
    ));
    let collector = Arc::new(
        QrLoginService::new(
            adapters.qr_tickets,
            collector_repo.clone(),
            secrets.clone(),
            connectors.clone(),
            egress.clone(),
            clock.clone(),
        )
        .with_audit(audit_writer.clone()),
    );
    let connector_resolver = Arc::new(
        CollectorConnectorResolver::new(
            collector_repo.clone(),
            secrets.clone(),
            connectors.clone(),
            clock.clone(),
        )
        .with_egress_repository(egress_repo.clone()),
    );
    let detail_egress_resolver = Arc::new(EgressResolver::new(
        egress_repo.clone(),
        Arc::new(DynamicLeaseManager::new()),
        Arc::new(DefaultProxyProviderAdapter::new(secrets.clone())),
    ));

    let heartbeat = Arc::new(crate::service::HeartbeatService::new(
        sources.clone(),
        clock.clone(),
    ));
    let revisions = Arc::new(RevisionService::new(articles.clone()));
    let ocr_planner = Arc::new(OcrJobPlanner::new(jobs_repo.clone()));
    let normalizer = Arc::new(ContentNormalizerService::new(
        articles.clone(),
        revisions.clone(),
        ocr_planner.clone(),
        clock.clone(),
    ));
    let request_limiter = Arc::new(crate::service::RequestRateLimiter::default());
    let secrets_for_capture = secrets.clone();
    let capture = {
        let svc = ArticleCaptureService::new(
            articles.clone(),
            jobs_repo.clone(),
            connectors.clone(),
            clock.clone(),
            adapters.object_store.clone(),
        );
        let svc = if let Some(cdp) = config.browser_cdp_url.as_deref() {
            tracing::info!(url = %cdp, "ArticleCapture 接入 Chromium CDP");
            svc.with_browser(Arc::new(CdpBrowser::new(cdp)))
        } else {
            svc
        };
        Arc::new(
            svc.with_collector_credentials(collector_repo.clone(), secrets_for_capture)
                .with_request_limiter(request_limiter.clone())
                .with_connector_resolver(connector_resolver.clone())
                .with_egress_resolver(detail_egress_resolver)
                .with_heartbeat(heartbeat)
                .with_sources(sources.clone())
                .with_plans(plan_repo.clone())
                .with_normalizer(normalizer.clone()),
        )
    };
    let manual_edit = Arc::new(
        ManualEditService::new(
            articles.clone(),
            revisions.clone(),
            ocr_planner,
            jobs_repo.clone(),
            clock.clone(),
        )
        .with_audit(audit_writer.clone()),
    );
    let source = Arc::new(
        SourceValidationService::new(
            sources.clone(),
            articles.clone(),
            jobs_repo.clone(),
            capture.clone(),
            connectors.clone(),
            clock.clone(),
        )
        .with_request_limiter(request_limiter)
        .with_connector_resolver(connector_resolver.clone())
        .with_audit(audit_writer.clone()),
    );

    let object_store: Arc<dyn ObjectStore> = adapters.object_store;
    let checker = Arc::new(DefaultHealthComponentChecker::new(
        collector_repo.clone(),
        egress_repo.clone(),
        connectors,
        object_store.clone(),
        secrets.clone(),
    ));
    let preflight = Arc::new(DailyPreflightService::new(
        health_repo.clone(),
        jobs_repo.clone(),
        settings_repo.clone(),
        checker,
        clock.clone(),
    ));
    let daily_plan = Arc::new(
        DailyPlanService::new(
            plan_repo,
            health_repo.clone(),
            sources.clone(),
            collector_repo,
            settings_repo,
            jobs_repo.clone(),
            clock.clone(),
        )
        .with_connector_resolver(connector_resolver)
        .with_audit(audit_writer.clone()),
    );
    let retention = Arc::new(
        RetentionCleanupService::new(retention_repo, clock.clone())
            .with_audit(audit_writer.clone()),
    );
    let delivery_groups = Arc::new(
        DeliveryGroupService::new(delivery_repo.clone(), sources, clock.clone())
            .with_audit(audit_writer.clone()),
    );
    let api_tokens = Arc::new(
        ApiTokenService::new(delivery_repo.clone(), clock.clone()).with_audit(audit_writer),
    );
    let article_cursor = Arc::new(ArticleCursorService::new(articles.clone(), delivery_repo));
    let health_delivery = Arc::new(HealthDeliveryService::new(health_repo));

    let job_registry = signal_job_registry_with(
        Some(capture.clone()),
        Some(preflight.clone()),
        Some(daily_plan.clone()),
        Some(NormalizerJobDeps::new(
            articles.clone(),
            normalizer.clone(),
            revisions.clone(),
            (config.environment.allows_fake() && config.ocr_provider.as_deref() == Some("fake"))
                .then(|| {
                    Arc::new(provider_hub::FakeOcrProvider::new("fake-ocr"))
                        as Arc<dyn provider_hub::OcrProvider>
                }),
            object_store.clone(),
            clock.clone(),
        )),
        Some(retention.clone()),
        Some(source.clone()),
    );

    AppHandles {
        config,
        store_mode,
        clock,
        jobs_repo,
        admin,
        egress,
        collector,
        source,
        preflight,
        daily_plan,
        manual_edit,
        delivery_groups,
        api_tokens,
        article_cursor,
        health_delivery,
        retention,
        capture,
        normalizer,
        revisions,
        articles,
        job_registry,
    }
}
