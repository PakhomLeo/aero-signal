//! 管理端 API。

mod articles;
mod auth;
mod collector_accounts;
mod daily_plan;
mod delivery_groups;
mod egress_settings;
mod health_snapshots;
mod jobs;
mod me;
mod settings;
mod sources;

pub use auth::{AdminAuth, extract_bearer};
pub use jobs::JobsState;

use std::sync::Arc;

use axum::Router;

use crate::service::{
    AdministratorService, ApiTokenService, DailyPlanService, DailyPreflightService,
    DeliveryGroupService, EgressService, ManualEditService, QrLoginService,
    SourceValidationService,
};

use self::articles::articles_routes;
use self::auth::auth_routes;
use self::collector_accounts::collector_accounts_routes;
use self::daily_plan::daily_plan_routes;
use self::delivery_groups::delivery_groups_routes;
use self::egress_settings::egress_settings_routes;
use self::health_snapshots::health_snapshots_routes;
use self::jobs::jobs_routes;
use self::me::me_routes;
use self::settings::settings_routes;
use self::sources::sources_routes;

/// 管理端共享状态。
#[derive(Clone)]
pub struct AdminState {
    pub admin: Arc<AdministratorService>,
    pub egress: Arc<EgressService>,
    pub jobs: JobsState,
    pub collector: Option<Arc<QrLoginService>>,
    pub source: Option<Arc<SourceValidationService>>,
    pub health: Option<Arc<DailyPreflightService>>,
    pub daily_plan: Option<Arc<DailyPlanService>>,
    pub manual_edit: Option<Arc<ManualEditService>>,
    pub delivery: Option<Arc<DeliveryGroupService>>,
    pub api_tokens: Option<Arc<ApiTokenService>>,
}

/// 装配管理端状态（Composition root 与测试共用）。
#[allow(clippy::too_many_arguments)]
pub fn build_admin_state(
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs: JobsState,
    collector: Option<Arc<QrLoginService>>,
    source: Option<Arc<SourceValidationService>>,
    health: Option<Arc<DailyPreflightService>>,
    daily_plan: Option<Arc<DailyPlanService>>,
    manual_edit: Option<Arc<ManualEditService>>,
    delivery: Option<Arc<DeliveryGroupService>>,
    api_tokens: Option<Arc<ApiTokenService>>,
) -> AdminState {
    AdminState {
        admin,
        egress,
        jobs,
        collector,
        source,
        health,
        daily_plan,
        manual_edit,
        delivery,
        api_tokens,
    }
}

/// 管理端完整路由（挂到 `/api/admin`）。
pub fn admin_router(state: AdminState) -> Router {
    admin_router_for_environment(state, true)
}

/// 按运行环境装配管理端路由；production 不挂载 Fake/dev-only endpoint。
pub fn admin_router_for_environment(state: AdminState, enable_dev_routes: bool) -> Router {
    Router::new()
        .merge(auth_routes())
        .merge(me_routes())
        .merge(settings_routes())
        .merge(egress_settings_routes())
        .merge(jobs_routes())
        .merge(collector_accounts_routes(enable_dev_routes))
        .merge(sources_routes())
        .merge(health_snapshots_routes())
        .merge(daily_plan_routes())
        .merge(articles_routes())
        .merge(delivery_groups_routes())
        .with_state(state)
}

/// 完整管理端路由（含 delivery 分组 / Token）；`runtime` 与测试注入入口。
#[allow(clippy::too_many_arguments)]
pub fn admin_router_with_delivery(
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs: JobsState,
    collector: Option<Arc<QrLoginService>>,
    source: Option<Arc<SourceValidationService>>,
    health: Option<Arc<DailyPreflightService>>,
    daily_plan: Option<Arc<DailyPlanService>>,
    manual_edit: Option<Arc<ManualEditService>>,
    delivery: Option<Arc<DeliveryGroupService>>,
    api_tokens: Option<Arc<ApiTokenService>>,
) -> Router {
    admin_router_for_environment(
        build_admin_state(
            admin,
            egress,
            jobs,
            collector,
            source,
            health,
            daily_plan,
            manual_edit,
            delivery,
            api_tokens,
        ),
        true,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn admin_router_with_delivery_for_environment(
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs: JobsState,
    collector: Option<Arc<QrLoginService>>,
    source: Option<Arc<SourceValidationService>>,
    health: Option<Arc<DailyPreflightService>>,
    daily_plan: Option<Arc<DailyPlanService>>,
    manual_edit: Option<Arc<ManualEditService>>,
    delivery: Option<Arc<DeliveryGroupService>>,
    api_tokens: Option<Arc<ApiTokenService>>,
    enable_dev_routes: bool,
) -> Router {
    admin_router_for_environment(
        build_admin_state(
            admin,
            egress,
            jobs,
            collector,
            source,
            health,
            daily_plan,
            manual_edit,
            delivery,
            api_tokens,
        ),
        enable_dev_routes,
    )
}

/// 兼容：仅 admin + egress + jobs。
pub fn admin_router_minimal(
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs: JobsState,
) -> Router {
    admin_router_with_delivery(
        admin, egress, jobs, None, None, None, None, None, None, None,
    )
}

/// 兼容：含 health / preflight（单测用）。
pub fn admin_router_with_health(
    admin: Arc<AdministratorService>,
    egress: Arc<EgressService>,
    jobs: JobsState,
    collector: Option<Arc<QrLoginService>>,
    source: Option<Arc<SourceValidationService>>,
    health: Option<Arc<DailyPreflightService>>,
) -> Router {
    admin_router_with_delivery(
        admin, egress, jobs, collector, source, health, None, None, None, None,
    )
}
