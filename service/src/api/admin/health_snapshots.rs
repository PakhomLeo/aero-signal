//! `GET /api/admin/health-snapshots/latest`
//! `POST /api/admin/health-snapshots/check-now`（立即检查，不覆盖快照）
//! `POST /api/admin/health-snapshots/run-preflight`（入队日预检，写不可变快照）

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use server_core::AppError;
use uuid::Uuid;

use super::AdminState;
use super::auth::AdminAuth;
use crate::service::daily_preflight::HealthSnapshotAdminDto;

pub fn health_snapshots_routes() -> Router<AdminState> {
    Router::new()
        .route("/health-snapshots/latest", get(get_latest))
        .route("/health-snapshots/check-now", post(check_now))
        .route("/health-snapshots/run-preflight", post(run_preflight))
}

async fn get_latest(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<HealthSnapshotAdminDto>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let health = state
        .health
        .as_ref()
        .ok_or_else(|| AppError::internal("daily-preflight 服务未配置"))?;
    match health.latest_snapshot_view().await {
        Ok(Some(dto)) => Ok(Json(dto)),
        Ok(None) => Err(AppError::not_found(
            "health.snapshot_unavailable",
            "尚无健康快照",
        )),
        Err(e) => Err(AppError::internal(e)),
    }
}

#[derive(Debug, Deserialize)]
pub struct CheckNowBody {
    /// collector / connector / fixed_egress / dynamic_egress / worker / object_store / all
    #[serde(default = "default_target_all")]
    pub target_type: String,
    pub target_id: Option<Uuid>,
}

fn default_target_all() -> String {
    "all".into()
}

#[derive(Debug, serde::Serialize)]
pub struct CheckNowResponse {
    pub job_id: String,
    pub kind: String,
    pub idempotency_key: Option<String>,
    pub snapshot_overwrite: bool,
}

async fn check_now(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Json(body): Json<CheckNowBody>,
) -> Result<Json<CheckNowResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let health = state
        .health
        .as_ref()
        .ok_or_else(|| AppError::internal("daily-preflight 服务未配置"))?;
    let job = health
        .enqueue_immediate_check(&body.target_type, body.target_id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(CheckNowResponse {
        job_id: job.id.to_string(),
        kind: job.kind,
        idempotency_key: job.idempotency_key,
        snapshot_overwrite: false,
    }))
}

#[derive(Debug, serde::Serialize)]
pub struct RunPreflightResponse {
    pub job_id: String,
    pub kind: String,
    pub idempotency_key: Option<String>,
    pub snapshot_date: String,
}

/// 入队 `daily_preflight`（幂等键 `preflight:<date>`），由 worker 写日快照。
/// 与 `check-now` 不同：本接口会固化不可变健康快照。
async fn run_preflight(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
) -> Result<Json<RunPreflightResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;
    let health = state
        .health
        .as_ref()
        .ok_or_else(|| AppError::internal("daily-preflight 服务未配置"))?;
    let date = health.business_date().await.map_err(AppError::internal)?;
    let job = health
        .enqueue_daily(date)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(RunPreflightResponse {
        job_id: job.id.to_string(),
        kind: job.kind,
        idempotency_key: job.idempotency_key,
        snapshot_date: date.format("%Y-%m-%d").to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::adapters::secret_store::MemorySecretStore;
    use crate::api::admin::{JobsState, admin_router_with_health};
    use crate::audit::AuditWriter;
    use crate::domain::BootstrapCredentials;
    use crate::domain::health::{
        HealthComponentStatus, HealthComponentType, OverallHealthStatus, ServiceHealthComponent,
        ServiceHealthSnapshot,
    };
    use crate::jobs::signal_job_registry_with;
    use crate::repository::{
        HealthSnapshotRepository, MemoryAdministratorStore, MemoryEgressStore, MemoryHealthStore,
        SettingsRepository,
    };
    use crate::service::daily_preflight::{
        ComponentCheckResult, DailyPreflightService, FakeHealthComponentChecker,
    };
    use crate::service::{AdministratorService, EgressService, LoginCommand, PatchMeCommand};
    use chrono::Utc;
    use job_pipeline::{JobRepository, JobStatus, MemoryRepo, Worker};
    use serde_json::json;
    use server_core::{Clock, FrozenClock, SystemClock};
    use std::time::Duration;
    use tower::ServiceExt;

    async fn authed_token(admin: &AdministratorService) -> String {
        admin
            .bootstrap(BootstrapCredentials {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
            })
            .await
            .unwrap();
        let login = admin
            .login(LoginCommand {
                username: "admin".into(),
                password: "Bootstrap9!".into(),
                ip: None,
                user_agent: None,
            })
            .await
            .unwrap();
        let auth = admin
            .sessions()
            .authenticate_token(login.token.as_str())
            .await
            .unwrap();
        admin
            .patch_me(
                &auth,
                PatchMeCommand {
                    current_password: Some("Bootstrap9!".into()),
                    new_password: Some("NewSecure1!".into()),
                    username: None,
                },
            )
            .await
            .unwrap();
        login.token.into_inner()
    }

    fn test_egress(store: &Arc<MemoryAdministratorStore>) -> Arc<EgressService> {
        let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
        Arc::new(EgressService::new(
            Arc::new(MemoryEgressStore::with_local_direct())
                as Arc<dyn crate::repository::EgressRepository>,
            Arc::new(MemorySecretStore::new()) as Arc<dyn crate::adapters::SecretStore>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            clock,
        ))
    }

    #[tokio::test]
    async fn latest_requires_auth() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
        let health_store = Arc::new(MemoryHealthStore::new());
        let checker = Arc::new(FakeHealthComponentChecker::new(vec![]));
        let preflight = Arc::new(DailyPreflightService::new(
            health_store as Arc<dyn HealthSnapshotRepository>,
            Arc::new(MemoryRepo::new()) as Arc<dyn job_pipeline::JobRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            checker,
            Arc::new(SystemClock) as Arc<dyn Clock>,
        ));
        let app = admin_router_with_health(
            admin,
            test_egress(&store),
            JobsState {
                repo: Arc::new(MemoryRepo::new()),
            },
            None,
            None,
            Some(preflight),
        );
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health-snapshots/latest")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn latest_returns_redacted_snapshot() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
        let token = authed_token(&admin).await;

        let health_store = Arc::new(MemoryHealthStore::new());
        let now = Utc::now();
        let snap_id = Uuid::new_v4();
        let snap = ServiceHealthSnapshot {
            id: snap_id,
            snapshot_date: chrono::NaiveDate::from_ymd_opt(2023, 11, 18).unwrap(),
            generated_at: now,
            overall_status: OverallHealthStatus::Degraded,
            yesterday_plan_success_count: 1,
            yesterday_plan_failure_count: 0,
            yesterday_article_failure_count: 0,
            today_eligible_source_count: 2,
            today_blocked_source_count: 1,
            pending_action_count: 1,
            summary_json: json!({ "ok": true }),
        };
        let comps = vec![ServiceHealthComponent {
            id: Uuid::new_v4(),
            snapshot_id: snap_id,
            component_type: HealthComponentType::Collector,
            component_id: Some(Uuid::new_v4()),
            status: HealthComponentStatus::Unhealthy,
            reason_code: Some("collector.needs_relogin".into()),
            checked_at: now,
            safe_details_json: json!({
                "error_code": "collector.needs_relogin",
                "cookie": "secret",
                "token": "t",
            }),
        }];
        health_store.insert_snapshot(&snap, &comps).await.unwrap();

        let checker = Arc::new(FakeHealthComponentChecker::new(vec![
            ComponentCheckResult {
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                safe_details_json: json!({}),
                checked_at: now,
            },
        ]));
        let preflight = Arc::new(DailyPreflightService::new(
            health_store as Arc<dyn HealthSnapshotRepository>,
            Arc::new(MemoryRepo::new()) as Arc<dyn job_pipeline::JobRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            checker,
            Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000)) as Arc<dyn Clock>,
        ));

        let app = admin_router_with_health(
            admin,
            test_egress(&store),
            JobsState {
                repo: Arc::new(MemoryRepo::new()),
            },
            None,
            None,
            Some(preflight),
        );
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health-snapshots/latest")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["overall_status"], "degraded");
        assert_eq!(v["snapshot_date"], "2023-11-18");
        let details = &v["components"][0]["safe_details"];
        assert!(details.get("cookie").is_none());
        assert!(details.get("token").is_none());
        assert_eq!(details["error_code"], "collector.needs_relogin");
    }

    #[tokio::test]
    async fn run_preflight_then_latest_ok() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
        let token = authed_token(&admin).await;

        let health_store = Arc::new(MemoryHealthStore::new());
        let jobs = Arc::new(MemoryRepo::new());
        let now = Utc::now();
        let checker = Arc::new(FakeHealthComponentChecker::new(vec![
            ComponentCheckResult {
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                safe_details_json: json!({ "status": "ok" }),
                checked_at: now,
            },
            ComponentCheckResult {
                component_type: HealthComponentType::ObjectStore,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                safe_details_json: json!({ "status": "ok" }),
                checked_at: now,
            },
        ]));
        let preflight = Arc::new(DailyPreflightService::new(
            health_store.clone() as Arc<dyn HealthSnapshotRepository>,
            jobs.clone() as Arc<dyn JobRepository>,
            store.clone() as Arc<dyn SettingsRepository>,
            checker,
            Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000)) as Arc<dyn Clock>,
        ));

        let app = admin_router_with_health(
            admin,
            test_egress(&store),
            JobsState {
                repo: jobs.clone() as Arc<dyn JobRepository>,
            },
            None,
            None,
            Some(preflight.clone()),
        );

        // 尚无快照 → 404
        let missing = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health-snapshots/latest")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), axum::http::StatusCode::NOT_FOUND);

        let enqueue = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/health-snapshots/run-preflight")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(enqueue.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(enqueue.into_body(), usize::MAX)
            .await
            .unwrap();
        let enq: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(enq["kind"], "daily_preflight");
        let job_id = enq["job_id"].as_str().unwrap().to_string();

        let reg = signal_job_registry_with(None, Some(preflight), None, None, None, None);
        let worker = Worker::new("test-preflight", jobs.clone() as Arc<dyn JobRepository>)
            .with_registry(&reg);
        worker.run_once().await.unwrap();

        let job = jobs
            .get_job(job_pipeline::JobId::from_raw(
                uuid::Uuid::parse_str(&job_id).unwrap(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);

        let latest = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/health-snapshots/latest")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(latest.status(), axum::http::StatusCode::OK);
        let bytes = axum::body::to_bytes(latest.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert!(v.get("snapshot_date").is_some());
        assert!(v.get("overall_status").is_some());
    }
}
