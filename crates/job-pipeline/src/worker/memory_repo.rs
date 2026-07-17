//! 内存 JobRepository：单元测试与本地无 PG 场景。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::error::{JobError, JobResult};
use crate::safe_context::validate_safe_context;
use crate::types::{CreateJob, Job, JobId, JobStatus};
use crate::worker::JobRepository;

#[derive(Default)]
struct State {
    jobs: HashMap<JobId, Job>,
    /// idempotency_key → job id
    by_key: HashMap<String, JobId>,
}

/// 线程安全的内存仓储。
#[derive(Default, Clone)]
pub struct MemoryRepo {
    state: Arc<RwLock<State>>,
}

impl MemoryRepo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Test/fixture hook for exercising the worker's persisted execution deadline.
    pub async fn set_execution_timeout(
        &self,
        id: JobId,
        deadline: chrono::DateTime<chrono::Utc>,
    ) -> JobResult<()> {
        let mut state = self.state.write().await;
        let job = state
            .jobs
            .get_mut(&id)
            .ok_or_else(|| JobError::permanent("job_pipeline.not_found", "job not found"))?;
        job.execution_timeout_at = Some(deadline);
        Ok(())
    }
}

#[async_trait]
impl JobRepository for MemoryRepo {
    async fn create_job(&self, cmd: CreateJob) -> JobResult<Job> {
        if let Some(ref ctx) = cmd.safe_context_json {
            validate_safe_context(ctx)
                .map_err(|e| JobError::permanent("job_pipeline.unsafe_context", e.to_string()))?;
        }

        let mut s = self.state.write().await;
        if let Some(ref key) = cmd.idempotency_key
            && let Some(id) = s.by_key.get(key)
            && let Some(job) = s.jobs.get(id)
        {
            if job.kind != cmd.kind || job.payload_json != cmd.payload_json {
                return Err(JobError::permanent(
                    "job_pipeline.idempotency_conflict",
                    "idempotency key is already bound to a different job kind or payload",
                ));
            }
            return Ok(job.clone());
        }

        let now = chrono::Utc::now();
        let id = JobId::new();
        let job = Job {
            id,
            kind: cmd.kind,
            status: JobStatus::Queued,
            payload_json: cmd.payload_json,
            idempotency_key: cmd.idempotency_key.clone(),
            progress_percent: 0.0,
            progress_message: String::new(),
            progress_stage: String::from("created"),
            attempt_count: 0,
            max_attempts: cmd.max_attempts.unwrap_or(3),
            next_run_at: Some(cmd.next_run_at.unwrap_or(now)),
            locked_by: None,
            locked_until: None,
            lease_heartbeat_at: None,
            execution_timeout_at: None,
            priority: cmd.priority.unwrap_or(0),
            dependency_keys: cmd.dependency_keys,
            cost_budget_limit: cmd.cost_budget_limit,
            cancel_requested_at: None,
            cancelled_at: None,
            resume_token: cmd.resume_token,
            result_json: None,
            last_error_code: None,
            last_error_message: None,
            action_code: None,
            action_message: None,
            action_url_or_command: None,
            safe_context_json: cmd.safe_context_json,
            created_by: cmd.created_by,
            created_at: now,
            updated_at: now,
        };
        if let Some(ref key) = cmd.idempotency_key {
            s.by_key.insert(key.clone(), id);
        }
        s.jobs.insert(id, job.clone());
        Ok(job)
    }

    async fn get_job(&self, id: JobId) -> JobResult<Option<Job>> {
        let s = self.state.read().await;
        Ok(s.jobs.get(&id).cloned())
    }

    async fn list_jobs(
        &self,
        kind: Option<&str>,
        status: Option<JobStatus>,
        limit: u32,
    ) -> JobResult<Vec<Job>> {
        let s = self.state.read().await;
        let mut result: Vec<Job> = s
            .jobs
            .values()
            .filter(|j| kind.is_none_or(|k| j.kind == k))
            .filter(|j| status.is_none_or(|st| j.status == st))
            .cloned()
            .collect();
        result.sort_by_key(|j| j.created_at);
        result.reverse();
        result.truncate(limit as usize);
        Ok(result)
    }

    async fn acquire_next(
        &self,
        worker_id: &str,
        kinds: &[String],
        lease_secs: u64,
    ) -> JobResult<Option<Job>> {
        let mut s = self.state.write().await;
        let now = chrono::Utc::now();

        // 先回收过期 lease（与 reclaim 一致，保证抢占原子性）。
        for job in s.jobs.values_mut() {
            if job.status == JobStatus::Running
                && let Some(until) = job.locked_until
                && until <= now
            {
                job.status = JobStatus::Queued;
                job.locked_by = None;
                job.locked_until = None;
                job.updated_at = now;
            }
        }

        let candidate_id = s
            .jobs
            .values()
            .filter(|j| {
                matches!(
                    j.status,
                    JobStatus::Queued | JobStatus::RetryWait | JobStatus::Cancelling
                )
            })
            .filter(|j| kinds.is_empty() || kinds.contains(&j.kind))
            .filter(|j| j.next_run_at.is_none_or(|t| t <= now))
            .filter(|j| j.locked_until.is_none_or(|t| t <= now))
            .min_by(|a, b| {
                // 高优先级先；同优先级按 created_at
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.created_at.cmp(&b.created_at))
            })
            .map(|j| j.id);

        if let Some(id) = candidate_id {
            let job = s.jobs.get_mut(&id).expect("just found");
            let was_cancelling = job.status == JobStatus::Cancelling;
            let lock_until = now + chrono::Duration::seconds(lease_secs as i64);
            // 取消中的任务仍需被 worker 取走以落成 cancelled；不计入 attempt。
            if !was_cancelling {
                job.status = JobStatus::Running;
                job.attempt_count += 1;
            }
            job.locked_by = Some(worker_id.to_string());
            job.locked_until = Some(lock_until);
            job.lease_heartbeat_at = Some(now);
            job.updated_at = now;
            return Ok(Some(job.clone()));
        }
        Ok(None)
    }

    async fn update_progress(
        &self,
        id: JobId,
        worker_id: &str,
        percent: f64,
        message: &str,
        stage: &str,
    ) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            if percent > 0.0 {
                job.progress_percent = percent;
            }
            if !message.is_empty() {
                job.progress_message = message.to_string();
            }
            if !stage.is_empty() {
                job.progress_stage = stage.to_string();
            }
            job.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn update_result(
        &self,
        id: JobId,
        worker_id: &str,
        result: serde_json::Value,
    ) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            job.result_json = Some(result);
            job.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn transition(
        &self,
        id: JobId,
        worker_id: &str,
        from: JobStatus,
        to: JobStatus,
    ) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
            && job.status == from
        {
            job.status = to;
            job.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn mark_cancelled(&self, id: JobId, worker_id: &str) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            let now = chrono::Utc::now();
            job.status = JobStatus::Cancelled;
            job.cancelled_at = Some(now);
            job.updated_at = now;
            job.locked_by = None;
            job.locked_until = None;
        }
        Ok(())
    }

    async fn request_cancel(&self, id: JobId) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && job.status.can_cancel()
        {
            let now = chrono::Utc::now();
            job.cancel_requested_at = Some(now);
            job.status = JobStatus::Cancelling;
            job.updated_at = now;
        }
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
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            let now = chrono::Utc::now();
            job.last_error_code = Some(code.to_string());
            job.last_error_message = Some(message.to_string());
            if retryable && job.attempt_count < job.max_attempts {
                job.status = JobStatus::RetryWait;
            } else {
                job.status = JobStatus::Failed;
            }
            job.updated_at = now;
        }
        Ok(())
    }

    async fn mark_needs_action(
        &self,
        id: JobId,
        worker_id: &str,
        code: &str,
        message: &str,
    ) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            let now = chrono::Utc::now();
            job.status = JobStatus::NeedsAction;
            job.last_error_code = Some(code.to_string());
            job.last_error_message = Some(message.to_string());
            job.action_code = Some(code.to_string());
            job.action_message = Some(message.to_string());
            job.updated_at = now;
            job.locked_by = None;
            job.locked_until = None;
        }
        Ok(())
    }

    async fn schedule_retry(&self, id: JobId, worker_id: &str, delay_secs: u64) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            let now = chrono::Utc::now();
            job.status = JobStatus::RetryWait;
            job.next_run_at = Some(now + chrono::Duration::seconds(delay_secs as i64));
            job.locked_by = None;
            job.locked_until = None;
            job.updated_at = now;
        }
        Ok(())
    }

    async fn heartbeat(&self, id: JobId, worker_id: &str, lease_secs: u64) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && (worker_id.is_empty() || job.locked_by.as_deref() == Some(worker_id))
        {
            let now = chrono::Utc::now();
            job.lease_heartbeat_at = Some(now);
            job.locked_until = Some(now + chrono::Duration::seconds(lease_secs as i64));
            job.updated_at = now;
        }
        Ok(())
    }

    async fn release_lease(&self, id: JobId, worker_id: &str) -> JobResult<()> {
        let mut s = self.state.write().await;
        if let Some(job) = s.jobs.get_mut(&id)
            && job.locked_by.as_deref() == Some(worker_id)
        {
            // 终态或 retry/needs_action 只清锁；仍 Running 则退回 Queued（异常路径）。
            if job.status == JobStatus::Running {
                job.status = JobStatus::Queued;
            }
            job.locked_by = None;
            job.locked_until = None;
            job.updated_at = chrono::Utc::now();
        }
        Ok(())
    }

    async fn reclaim_expired_leases(&self) -> JobResult<u64> {
        let mut s = self.state.write().await;
        let now = chrono::Utc::now();
        let mut n = 0u64;
        for job in s.jobs.values_mut() {
            if job.status == JobStatus::Running
                && let Some(until) = job.locked_until
                && until <= now
            {
                job.status = JobStatus::Queued;
                job.locked_by = None;
                job.locked_until = None;
                job.updated_at = now;
                n += 1;
            }
        }
        Ok(n)
    }
}
