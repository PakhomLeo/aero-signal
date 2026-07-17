//! Admin job 查询：`GET /api/admin/jobs/{id}`（需管理员会话）。

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use job_pipeline::{Job, JobId, JobRepository};
use serde::{Deserialize, Serialize};
use server_core::AppError;
use uuid::Uuid;

use super::AdminState;
use super::auth::AdminAuth;

/// 可注入的 job 仓储（内存或 PG）。
#[derive(Clone)]
pub struct JobsState {
    pub repo: Arc<dyn JobRepository>,
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub id: String,
    pub kind: String,
    pub status: String,
    pub progress_percent: f64,
    pub progress_message: String,
    pub progress_stage: String,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub idempotency_key: Option<String>,
    pub next_run_at: Option<String>,
    pub result_json: Option<serde_json::Value>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub action_code: Option<String>,
    pub action_message: Option<String>,
    pub safe_context_json: Option<serde_json::Value>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<Job> for JobResponse {
    fn from(j: Job) -> Self {
        Self {
            id: j.id.to_string(),
            kind: j.kind,
            status: j.status.as_str().to_string(),
            progress_percent: j.progress_percent,
            progress_message: j.progress_message,
            progress_stage: j.progress_stage,
            attempt_count: j.attempt_count,
            max_attempts: j.max_attempts,
            idempotency_key: j.idempotency_key,
            next_run_at: j.next_run_at.map(|t| t.to_rfc3339()),
            result_json: j.result_json,
            last_error_code: j.last_error_code,
            last_error_message: j.last_error_message,
            action_code: j.action_code,
            action_message: j.action_message,
            safe_context_json: j.safe_context_json,
            created_at: j.created_at.to_rfc3339(),
            updated_at: j.updated_at.to_rfc3339(),
        }
    }
}

pub fn jobs_routes() -> Router<AdminState> {
    Router::new()
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/cancel", post(cancel_job))
}

async fn get_job(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;

    let uuid = Uuid::parse_str(&id).map_err(|_| {
        AppError::new("job.invalid_id", format!("无效的 job id: {id}"))
            .with_status(axum::http::StatusCode::BAD_REQUEST)
    })?;
    let job_id = JobId::from_raw(uuid);

    let job = state
        .jobs
        .repo
        .get_job(job_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()).with_detail("code", e.code))?;

    match job {
        Some(j) => Ok(Json(JobResponse::from(j))),
        None => Err(AppError::not_found(
            "job.not_found",
            format!("job 不存在: {id}"),
        )),
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CancelJobResponse {
    pub id: String,
    pub status: String,
    pub cancelled: bool,
}

async fn cancel_job(
    State(state): State<AdminState>,
    AdminAuth(auth): AdminAuth,
    Path(id): Path<String>,
) -> Result<Json<CancelJobResponse>, AppError> {
    state.admin.ensure_password_changed(&auth)?;

    let uuid = Uuid::parse_str(&id).map_err(|_| {
        AppError::new("job.invalid_id", format!("无效的 job id: {id}"))
            .with_status(axum::http::StatusCode::BAD_REQUEST)
    })?;
    let job_id = JobId::from_raw(uuid);

    let job = state
        .jobs
        .repo
        .get_job(job_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()).with_detail("code", e.code))?
        .ok_or_else(|| AppError::not_found("job.not_found", format!("job 不存在: {id}")))?;

    if job.status.is_terminal() {
        return Ok(Json(CancelJobResponse {
            id: id.clone(),
            status: job.status.as_str().to_string(),
            cancelled: job.status == job_pipeline::JobStatus::Cancelled,
        }));
    }

    state
        .jobs
        .repo
        .request_cancel(job_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()).with_detail("code", e.code))?;

    let updated = state
        .jobs
        .repo
        .get_job(job_id)
        .await
        .map_err(|e| AppError::internal(e.to_string()))?
        .unwrap_or(job);

    Ok(Json(CancelJobResponse {
        id: id.clone(),
        status: updated.status.as_str().to_string(),
        cancelled: updated.status == job_pipeline::JobStatus::Cancelled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::secret_store::MemorySecretStore;
    use crate::api::admin::{JobsState, admin_router_minimal};
    use crate::audit::AuditWriter;
    use crate::domain::BootstrapCredentials;
    use crate::repository::{MemoryAdministratorStore, MemoryEgressStore};
    use crate::service::{AdministratorService, EgressService, LoginCommand, PatchMeCommand};
    use job_pipeline::{CreateJob, MemoryRepo};
    use serde_json::json;
    use server_core::{Clock, SystemClock};
    use std::time::Duration;
    use tower::ServiceExt;

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
    async fn get_job_requires_auth() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn crate::repository::SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
        let app = admin_router_minimal(
            admin,
            test_egress(&store),
            JobsState {
                repo: Arc::new(MemoryRepo::new()),
            },
        );
        let id = Uuid::new_v4();
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/jobs/{id}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_job_returns_json_when_authed() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn crate::repository::SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
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
        let token = login.token.into_inner();

        let repo = MemoryRepo::new();
        let created = repo
            .create_job(CreateJob::new("daily_preflight", "admin-get-1", json!({})))
            .await
            .unwrap();

        let app = admin_router_minimal(
            admin,
            test_egress(&store),
            JobsState {
                repo: Arc::new(repo),
            },
        );
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/jobs/{}", created.id))
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
        assert_eq!(v["id"], created.id.to_string());
        assert_eq!(v["kind"], "daily_preflight");
    }

    #[tokio::test]
    async fn cancel_job_marks_cancelled_when_authed() {
        let store = Arc::new(MemoryAdministratorStore::new());
        let admin = Arc::new(AdministratorService::new(
            store.clone() as Arc<dyn crate::repository::AdministratorRepository>,
            store.clone() as Arc<dyn crate::repository::SettingsRepository>,
            AuditWriter::new(store.clone() as Arc<dyn crate::repository::AuditRepository>),
            Arc::new(SystemClock),
            Duration::from_secs(3600),
        ));
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
        let token = login.token.into_inner();

        let repo = Arc::new(MemoryRepo::new());
        let created = repo
            .create_job(CreateJob::new(
                "daily_preflight",
                "admin-cancel-1",
                json!({}),
            ))
            .await
            .unwrap();

        let app =
            admin_router_minimal(admin, test_egress(&store), JobsState { repo: repo.clone() });
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/jobs/{}/cancel", created.id))
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
        let body: CancelJobResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.id, created.id.to_string());
        assert_eq!(body.status, "cancelling");
        assert!(!body.cancelled);

        let updated = repo.get_job(created.id).await.unwrap().unwrap();
        assert_eq!(updated.status, job_pipeline::JobStatus::Cancelling);
        assert!(updated.cancel_requested_at.is_some());
    }
}
