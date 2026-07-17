//! PostgreSQL JobRepository（sqlx）。

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::error::{JobError, JobResult};
use crate::safe_context::validate_safe_context;
use crate::types::{CreateJob, Job, JobId, JobStatus};
use crate::worker::JobRepository;

/// 基于 sqlx 的 PostgreSQL 仓储。
#[derive(Clone)]
pub struct PgJobRepository {
    pool: PgPool,
}

impl PgJobRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_status(s: &str) -> JobResult<JobStatus> {
    JobStatus::parse(s)
        .ok_or_else(|| JobError::permanent("job_pipeline.invalid_status", format!("未知状态: {s}")))
}

fn row_to_job(row: &PgRow) -> JobResult<Job> {
    let id: Uuid = row.try_get("id").map_err(sqlx_err)?;
    let status_s: String = row.try_get("status").map_err(sqlx_err)?;
    let dep: Option<serde_json::Value> = row.try_get("dependency_keys").map_err(sqlx_err)?;
    let dependency_keys = match dep {
        Some(serde_json::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        _ => vec![],
    };

    Ok(Job {
        id: JobId::from_raw(id),
        kind: row.try_get("kind").map_err(sqlx_err)?,
        status: map_status(&status_s)?,
        payload_json: row.try_get("payload_json").map_err(sqlx_err)?,
        idempotency_key: row.try_get("idempotency_key").map_err(sqlx_err)?,
        progress_percent: row
            .try_get::<f64, _>("progress_percent")
            .map_err(sqlx_err)?,
        progress_message: row
            .try_get::<Option<String>, _>("progress_message")
            .map_err(sqlx_err)?
            .unwrap_or_default(),
        progress_stage: row
            .try_get::<Option<String>, _>("progress_stage")
            .map_err(sqlx_err)?
            .unwrap_or_default(),
        attempt_count: row.try_get::<i32, _>("attempt_count").map_err(sqlx_err)? as u32,
        max_attempts: row.try_get::<i32, _>("max_attempts").map_err(sqlx_err)? as u32,
        next_run_at: row.try_get("next_run_at").map_err(sqlx_err)?,
        locked_by: row.try_get("locked_by").map_err(sqlx_err)?,
        locked_until: row.try_get("locked_until").map_err(sqlx_err)?,
        lease_heartbeat_at: row.try_get("lease_heartbeat_at").map_err(sqlx_err)?,
        execution_timeout_at: row.try_get("execution_timeout_at").map_err(sqlx_err)?,
        priority: row.try_get("priority").map_err(sqlx_err)?,
        dependency_keys,
        cost_budget_limit: row.try_get("cost_budget_limit").map_err(sqlx_err)?,
        cancel_requested_at: row.try_get("cancel_requested_at").map_err(sqlx_err)?,
        cancelled_at: row.try_get("cancelled_at").map_err(sqlx_err)?,
        resume_token: row.try_get("resume_token").map_err(sqlx_err)?,
        result_json: row.try_get("result_json").map_err(sqlx_err)?,
        last_error_code: row.try_get("last_error_code").map_err(sqlx_err)?,
        last_error_message: row.try_get("last_error_message").map_err(sqlx_err)?,
        action_code: row.try_get("action_code").map_err(sqlx_err)?,
        action_message: row.try_get("action_message").map_err(sqlx_err)?,
        action_url_or_command: row.try_get("action_url_or_command").map_err(sqlx_err)?,
        safe_context_json: row.try_get("safe_context_json").map_err(sqlx_err)?,
        created_by: row.try_get("created_by").map_err(sqlx_err)?,
        created_at: row.try_get("created_at").map_err(sqlx_err)?,
        updated_at: row.try_get("updated_at").map_err(sqlx_err)?,
    })
}

fn sqlx_err(e: sqlx::Error) -> JobError {
    JobError::permanent("job_pipeline.db", e.to_string())
}

const SELECT_COLS: &str = r#"
    id, kind, status, payload_json, idempotency_key,
    progress_percent, progress_message, progress_stage,
    attempt_count, max_attempts, next_run_at,
    locked_by, locked_until, lease_heartbeat_at, execution_timeout_at,
    priority, dependency_keys, cost_budget_limit,
    cancel_requested_at, cancelled_at, resume_token,
    result_json, last_error_code, last_error_message,
    action_code, action_message, action_url_or_command,
    safe_context_json, created_by, created_at, updated_at
"#;

/// UPDATE … FROM candidate 时 RETURNING 需带表别名，避免 id 等列歧义。
const SELECT_COLS_J: &str = r#"
    j.id, j.kind, j.status, j.payload_json, j.idempotency_key,
    j.progress_percent, j.progress_message, j.progress_stage,
    j.attempt_count, j.max_attempts, j.next_run_at,
    j.locked_by, j.locked_until, j.lease_heartbeat_at, j.execution_timeout_at,
    j.priority, j.dependency_keys, j.cost_budget_limit,
    j.cancel_requested_at, j.cancelled_at, j.resume_token,
    j.result_json, j.last_error_code, j.last_error_message,
    j.action_code, j.action_message, j.action_url_or_command,
    j.safe_context_json, j.created_by, j.created_at, j.updated_at
"#;

#[async_trait]
impl JobRepository for PgJobRepository {
    async fn create_job(&self, cmd: CreateJob) -> JobResult<Job> {
        if let Some(ref ctx) = cmd.safe_context_json {
            validate_safe_context(ctx)
                .map_err(|e| JobError::permanent("job_pipeline.unsafe_context", e.to_string()))?;
        }

        // 幂等：已存在则返回。
        if let Some(ref key) = cmd.idempotency_key
            && let Some(existing) = sqlx::query(&format!(
                "SELECT {SELECT_COLS} FROM fetch_jobs WHERE idempotency_key = $1"
            ))
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(sqlx_err)?
        {
            let job = row_to_job(&existing)?;
            if job.kind != cmd.kind || job.payload_json != cmd.payload_json {
                return Err(JobError::permanent(
                    "job_pipeline.idempotency_conflict",
                    "idempotency key is already bound to a different job kind or payload",
                ));
            }
            return Ok(job);
        }

        let id = Uuid::new_v4();
        let now = chrono::Utc::now();
        let max_attempts = cmd.max_attempts.unwrap_or(3) as i32;
        let priority = cmd.priority.unwrap_or(0);
        let dep = serde_json::Value::Array(
            cmd.dependency_keys
                .iter()
                .map(|s| serde_json::Value::String(s.clone()))
                .collect(),
        );

        let row = sqlx::query(&format!(
            r#"
            INSERT INTO fetch_jobs (
                id, kind, status, payload_json, idempotency_key,
                progress_percent, progress_message, progress_stage,
                attempt_count, max_attempts, next_run_at,
                priority, dependency_keys, cost_budget_limit,
                resume_token, safe_context_json, created_by,
                created_at, updated_at
            ) VALUES (
                $1, $2, 'queued', $3, $4,
                0, '', 'created',
                0, $5, $6,
                $7, $8, $9,
                $10, $11, $12,
                $13, $13
            )
            ON CONFLICT (idempotency_key) WHERE idempotency_key IS NOT NULL
            DO NOTHING
            RETURNING {SELECT_COLS}
            "#
        ))
        .bind(id)
        .bind(&cmd.kind)
        .bind(&cmd.payload_json)
        .bind(&cmd.idempotency_key)
        .bind(max_attempts)
        .bind(cmd.next_run_at.unwrap_or(now))
        .bind(priority)
        .bind(&dep)
        .bind(cmd.cost_budget_limit)
        .bind(&cmd.resume_token)
        .bind(&cmd.safe_context_json)
        .bind(&cmd.created_by)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_err)?;

        if let Some(row) = row {
            return row_to_job(&row);
        }

        // 冲突：再查一次。
        if let Some(ref key) = cmd.idempotency_key {
            let existing = sqlx::query(&format!(
                "SELECT {SELECT_COLS} FROM fetch_jobs WHERE idempotency_key = $1"
            ))
            .bind(key)
            .fetch_one(&self.pool)
            .await
            .map_err(sqlx_err)?;
            let job = row_to_job(&existing)?;
            if job.kind != cmd.kind || job.payload_json != cmd.payload_json {
                return Err(JobError::permanent(
                    "job_pipeline.idempotency_conflict",
                    "idempotency key is already bound to a different job kind or payload",
                ));
            }
            return Ok(job);
        }

        Err(JobError::permanent(
            "job_pipeline.create_failed",
            "插入 job 失败且无幂等键可回查",
        ))
    }

    async fn get_job(&self, id: JobId) -> JobResult<Option<Job>> {
        let row = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM fetch_jobs WHERE id = $1"
        ))
        .bind(id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_err)?;
        row.map(|r| row_to_job(&r)).transpose()
    }

    async fn list_jobs(
        &self,
        kind: Option<&str>,
        status: Option<JobStatus>,
        limit: u32,
    ) -> JobResult<Vec<Job>> {
        let status_s = status.map(|s| s.as_str().to_string());
        let rows = sqlx::query(&format!(
            r#"
            SELECT {SELECT_COLS} FROM fetch_jobs
            WHERE ($1::text IS NULL OR kind = $1)
              AND ($2::text IS NULL OR status = $2)
            ORDER BY created_at DESC
            LIMIT $3
            "#
        ))
        .bind(kind)
        .bind(status_s)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_err)?;

        rows.iter().map(row_to_job).collect()
    }

    async fn acquire_next(
        &self,
        worker_id: &str,
        kinds: &[String],
        lease_secs: u64,
    ) -> JobResult<Option<Job>> {
        let _ = self.reclaim_expired_leases().await?;

        let kinds_arr: Vec<String> = kinds.to_vec();
        let row = sqlx::query(&format!(
            r#"
            WITH candidate AS (
                SELECT id FROM fetch_jobs
                WHERE status IN ('queued', 'retry_wait', 'cancelling')
                  AND (cardinality($1::text[]) = 0 OR kind = ANY($1))
                  AND (next_run_at IS NULL OR next_run_at <= now())
                  AND (locked_until IS NULL OR locked_until <= now())
                ORDER BY priority DESC, created_at ASC
                FOR UPDATE SKIP LOCKED
                LIMIT 1
            )
            UPDATE fetch_jobs j
            SET status = CASE WHEN j.status = 'cancelling' THEN j.status ELSE 'running' END,
                locked_by = $2,
                locked_until = now() + make_interval(secs => $3::double precision),
                lease_heartbeat_at = now(),
                attempt_count = CASE WHEN j.status = 'cancelling' THEN attempt_count ELSE attempt_count + 1 END,
                updated_at = now()
            FROM candidate
            WHERE j.id = candidate.id
            RETURNING {SELECT_COLS_J}
            "#
        ))
        .bind(&kinds_arr)
        .bind(worker_id)
        .bind(lease_secs as f64)
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_err)?;

        row.map(|r| row_to_job(&r)).transpose()
    }

    async fn update_progress(
        &self,
        id: JobId,
        worker_id: &str,
        percent: f64,
        message: &str,
        stage: &str,
    ) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                progress_percent = CASE WHEN $2 > 0 THEN $2 ELSE progress_percent END,
                progress_message = CASE WHEN $3 <> '' THEN $3 ELSE progress_message END,
                progress_stage = CASE WHEN $4 <> '' THEN $4 ELSE progress_stage END,
                updated_at = now()
            WHERE id = $1 AND ($5 = '' OR locked_by = $5)
            "#,
        )
        .bind(id.as_uuid())
        .bind(percent)
        .bind(message)
        .bind(stage)
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn update_result(
        &self,
        id: JobId,
        worker_id: &str,
        result: serde_json::Value,
    ) -> JobResult<()> {
        sqlx::query("UPDATE fetch_jobs SET result_json = $2, updated_at = now() WHERE id = $1 AND ($3 = '' OR locked_by = $3)")
            .bind(id.as_uuid())
            .bind(result)
            .bind(worker_id)
            .execute(&self.pool)
            .await
            .map_err(sqlx_err)?;
        Ok(())
    }

    async fn transition(
        &self,
        id: JobId,
        worker_id: &str,
        from: JobStatus,
        to: JobStatus,
    ) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET status = $3, updated_at = now()
            WHERE id = $1 AND status = $2 AND ($4 = '' OR locked_by = $4)
            "#,
        )
        .bind(id.as_uuid())
        .bind(from.as_str())
        .bind(to.as_str())
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn mark_cancelled(&self, id: JobId, worker_id: &str) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                status = 'cancelled',
                cancelled_at = now(),
                locked_by = NULL,
                locked_until = NULL,
                updated_at = now()
            WHERE id = $1 AND ($2 = '' OR locked_by = $2)
            "#,
        )
        .bind(id.as_uuid())
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn request_cancel(&self, id: JobId) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                cancel_requested_at = now(),
                status = 'cancelling',
                updated_at = now()
            WHERE id = $1
              AND status IN ('queued', 'running', 'retry_wait', 'needs_action')
            "#,
        )
        .bind(id.as_uuid())
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn fail(
        &self,
        id: JobId,
        worker_id: &str,
        code: &str,
        message: &str,
        retryable: bool,
    ) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                last_error_code = $2,
                last_error_message = $3,
                status = CASE
                    WHEN $4 AND attempt_count < max_attempts THEN 'retry_wait'
                    ELSE 'failed'
                END,
                updated_at = now()
            WHERE id = $1 AND ($5 = '' OR locked_by = $5)
            "#,
        )
        .bind(id.as_uuid())
        .bind(code)
        .bind(message)
        .bind(retryable)
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn mark_needs_action(
        &self,
        id: JobId,
        worker_id: &str,
        code: &str,
        message: &str,
    ) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                status = 'needs_action',
                last_error_code = $2,
                last_error_message = $3,
                action_code = $2,
                action_message = $3,
                locked_by = NULL,
                locked_until = NULL,
                updated_at = now()
            WHERE id = $1 AND ($4 = '' OR locked_by = $4)
            "#,
        )
        .bind(id.as_uuid())
        .bind(code)
        .bind(message)
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn schedule_retry(&self, id: JobId, worker_id: &str, delay_secs: u64) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                status = 'retry_wait',
                next_run_at = now() + make_interval(secs => $2::double precision),
                locked_by = NULL,
                locked_until = NULL,
                updated_at = now()
            WHERE id = $1 AND ($3 = '' OR locked_by = $3)
            "#,
        )
        .bind(id.as_uuid())
        .bind(delay_secs as f64)
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn heartbeat(&self, id: JobId, worker_id: &str, lease_secs: u64) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                lease_heartbeat_at = now(),
                locked_until = now() + make_interval(secs => $3::double precision),
                updated_at = now()
            WHERE id = $1 AND ($2 = '' OR locked_by = $2)
            "#,
        )
        .bind(id.as_uuid())
        .bind(worker_id)
        .bind(lease_secs as f64)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn release_lease(&self, id: JobId, worker_id: &str) -> JobResult<()> {
        sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                status = CASE WHEN status = 'running' THEN 'queued' ELSE status END,
                locked_by = NULL,
                locked_until = NULL,
                updated_at = now()
            WHERE id = $1 AND locked_by = $2
            "#,
        )
        .bind(id.as_uuid())
        .bind(worker_id)
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(())
    }

    async fn reclaim_expired_leases(&self) -> JobResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE fetch_jobs SET
                status = 'queued',
                locked_by = NULL,
                locked_until = NULL,
                updated_at = now()
            WHERE status = 'running'
              AND locked_until IS NOT NULL
              AND locked_until <= now()
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(sqlx_err)?;
        Ok(result.rows_affected())
    }
}

#[cfg(all(test, feature = "postgres-tests"))]
mod pg_tests {
    use super::*;
    use crate::types::CreateJob;
    use serde_json::json;

    async fn pool() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        PgPool::connect(&url).await.ok()
    }

    #[tokio::test]
    async fn pg_create_idempotent() {
        let Some(pool) = pool().await else {
            eprintln!("skip: DATABASE_URL unavailable");
            return;
        };
        let repo = PgJobRepository::new(pool);
        let key = format!("test-idem-{}", Uuid::new_v4());
        let cmd = CreateJob::new("daily_preflight", &key, json!({}));
        let a = repo.create_job(cmd.clone()).await.unwrap();
        let b = repo.create_job(cmd).await.unwrap();
        assert_eq!(a.id, b.id);
    }
}
