//! Worker 与 JobRepository trait。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::error::{JobError, JobResult};
use crate::handler::{JobContext, JobHandler, ProgressSender, ProgressUpdate};
use crate::registry::JobHandlerRegistry;
use crate::retry::{RetryDecision, RetryPolicy};
use crate::types::{CreateJob, Job, JobId, JobStatus};

/// 存储无关的 job 仓储接口。
#[async_trait]
pub trait JobRepository: Send + Sync {
    async fn create_job(&self, cmd: CreateJob) -> JobResult<Job>;
    async fn get_job(&self, id: JobId) -> JobResult<Option<Job>>;
    async fn list_jobs(
        &self,
        kind: Option<&str>,
        status: Option<JobStatus>,
        limit: u32,
    ) -> JobResult<Vec<Job>>;
    async fn acquire_next(
        &self,
        worker_id: &str,
        kinds: &[String],
        lease_secs: u64,
    ) -> JobResult<Option<Job>>;
    async fn update_progress(
        &self,
        id: JobId,
        worker_id: &str,
        percent: f64,
        message: &str,
        stage: &str,
    ) -> JobResult<()>;
    async fn update_result(
        &self,
        id: JobId,
        worker_id: &str,
        result: serde_json::Value,
    ) -> JobResult<()>;
    async fn transition(
        &self,
        id: JobId,
        worker_id: &str,
        from: JobStatus,
        to: JobStatus,
    ) -> JobResult<()>;
    async fn mark_cancelled(&self, id: JobId, worker_id: &str) -> JobResult<()>;
    async fn request_cancel(&self, id: JobId) -> JobResult<()>;
    async fn fail(
        &self,
        id: JobId,
        worker_id: &str,
        code: &str,
        message: &str,
        retryable: bool,
    ) -> JobResult<()>;
    async fn mark_needs_action(
        &self,
        id: JobId,
        worker_id: &str,
        code: &str,
        message: &str,
    ) -> JobResult<()>;
    async fn schedule_retry(&self, id: JobId, worker_id: &str, delay_secs: u64) -> JobResult<()>;
    async fn heartbeat(&self, id: JobId, worker_id: &str, lease_secs: u64) -> JobResult<()>;
    async fn release_lease(&self, id: JobId, worker_id: &str) -> JobResult<()>;
    /// 将 lease 过期的 running job 回收为 queued（崩溃恢复）。
    async fn reclaim_expired_leases(&self) -> JobResult<u64>;
}

/// 后台 worker：抢占 due job 并执行 handler。
pub struct Worker {
    worker_id: String,
    repo: Arc<dyn JobRepository>,
    handlers: HashMap<String, Arc<dyn JobHandler>>,
    kinds: Vec<String>,
    poll_interval: tokio::time::Duration,
    lease_secs: u64,
    retry_policy: RetryPolicy,
}

impl Worker {
    pub fn new(worker_id: impl Into<String>, repo: Arc<dyn JobRepository>) -> Self {
        Self {
            worker_id: worker_id.into(),
            repo,
            handlers: HashMap::new(),
            kinds: Vec::new(),
            poll_interval: tokio::time::Duration::from_secs(5),
            lease_secs: 60,
            retry_policy: RetryPolicy::new(),
        }
    }

    pub fn with_poll_interval(mut self, secs: u64) -> Self {
        self.poll_interval = tokio::time::Duration::from_secs(secs);
        self
    }

    pub fn with_lease_secs(mut self, secs: u64) -> Self {
        self.lease_secs = secs;
        self
    }

    pub fn with_retry_policy(mut self, policy: RetryPolicy) -> Self {
        self.retry_policy = policy;
        self
    }

    pub fn register(mut self, handler: Arc<dyn JobHandler>) -> Self {
        self.kinds.push(handler.kind().to_string());
        self.handlers.insert(handler.kind().to_string(), handler);
        self
    }

    pub fn with_registry(mut self, registry: &JobHandlerRegistry) -> Self {
        for kind in registry.kinds() {
            if let Some(h) = registry.get(&kind) {
                self.kinds.push(kind.clone());
                self.handlers.insert(kind, h);
            }
        }
        self
    }

    pub async fn run_once(&self) -> JobResult<()> {
        // 先回收过期 lease（崩溃恢复）。
        let _ = self.repo.reclaim_expired_leases().await?;
        // 每次抢占生成独立 lease owner，避免同一 worker 进程在 lease 过期后
        // 重新抢占时，旧 handler 仍能凭旧 owner 写回结果。
        let lease_owner = format!("{}:{}", self.worker_id, Uuid::new_v4());

        let Some(job) = self
            .repo
            .acquire_next(&lease_owner, &self.kinds, self.lease_secs)
            .await?
        else {
            return Ok(());
        };

        // 若已请求取消，直接标记 cancelled。
        if job.cancel_requested_at.is_some() || job.status == JobStatus::Cancelling {
            self.repo.mark_cancelled(job.id, &lease_owner).await?;
            self.repo.release_lease(job.id, &lease_owner).await?;
            return Ok(());
        }

        let kind = job.kind.clone();
        let handler = match self.handlers.get(&kind) {
            Some(h) => h.clone(),
            None => {
                warn!(kind = %kind, "no handler registered for job kind");
                self.repo
                    .mark_needs_action(
                        job.id,
                        &lease_owner,
                        "job_pipeline.handler_missing",
                        &format!("no handler for kind {kind}"),
                    )
                    .await?;
                self.repo.release_lease(job.id, &lease_owner).await?;
                return Ok(());
            }
        };

        let (tx, mut rx) = ProgressSender::channel(256);
        let cancel_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = JobContext {
            worker_id: lease_owner.clone(),
            payload_json: job.payload_json.clone(),
            progress_tx: tx,
            cancel_requested: cancel_requested.clone(),
        };

        let job_id = job.id;
        let repo = self.repo.clone();
        let worker_id = lease_owner.clone();
        let lease_secs = self.lease_secs;

        // 心跳任务：执行期间续租。
        let heartbeat = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs((lease_secs / 3).max(1)));
            loop {
                interval.tick().await;
                if repo
                    .heartbeat(job_id, &worker_id, lease_secs)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });

        let cancel_repo = self.repo.clone();
        let cancel_flag = cancel_requested;
        let mut cancel_monitor = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(250));
            loop {
                interval.tick().await;
                match cancel_repo.get_job(job_id).await {
                    Ok(Some(job))
                        if job.cancel_requested_at.is_some()
                            || job.status == JobStatus::Cancelling =>
                    {
                        cancel_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        return true;
                    }
                    Ok(Some(_)) => {}
                    _ => return false,
                }
            }
        });

        // Drain progress while the handler is running. This keeps the channel
        // bounded in practice and prevents long jobs from retaining an
        // unbounded backlog until completion.
        let progress_repo = self.repo.clone();
        let progress_worker_id = lease_owner.clone();
        let progress_task = tokio::spawn(async move {
            while let Some(update) = rx.recv().await {
                persist_update(progress_repo.as_ref(), job_id, &progress_worker_id, update).await;
            }
        });

        let cancel_ctx = ctx.clone();
        let handler_for_task = handler.clone();
        let mut handler_task =
            tokio::spawn(async move { handler_for_task.execute(job_id, ctx).await });
        // execution_timeout_at 由入队侧按业务 SLA 设置；未设置时使用足够长的哨兵，
        // 保持现有 job 的无限期执行语义，同时避免 select 分支复制。
        let timeout_duration = job
            .execution_timeout_at
            .map(|deadline| {
                deadline
                    .signed_duration_since(Utc::now())
                    .to_std()
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| tokio::time::Duration::from_secs(100 * 365 * 24 * 60 * 60));
        let timeout_sleep = tokio::time::sleep(timeout_duration);
        tokio::pin!(timeout_sleep);
        let handle_result = tokio::select! {
            joined = &mut handler_task => joined.map_err(|e| JobError::retryable("job.handler_join", e.to_string()).with_job_id(job_id))?,
            cancelled = &mut cancel_monitor => {
                if cancelled.unwrap_or(false) {
                    let _ = handler.cancel(job_id, &cancel_ctx).await;
                    handler_task.abort();
                    Err(JobError::permanent("job.cancelled", "job cancellation requested").with_job_id(job_id))
                } else {
                    handler_task.await.map_err(|e| JobError::retryable("job.handler_join", e.to_string()).with_job_id(job_id))?
                }
            },
            _ = &mut timeout_sleep => {
                let _ = handler.cancel(job_id, &cancel_ctx).await;
                handler_task.abort();
                Err(JobError::retryable("job.execution_timeout", "job execution timed out").with_job_id(job_id))
            }
        };
        heartbeat.abort();
        drop(cancel_ctx);
        // The monitor is no longer needed when the handler wins the race.
        cancel_monitor.abort();
        let _ = progress_task.await;

        // 执行后再次检查取消。
        if let Ok(Some(fresh)) = self.repo.get_job(job_id).await
            && (fresh.cancel_requested_at.is_some() || fresh.status == JobStatus::Cancelling)
        {
            self.repo.mark_cancelled(job_id, &lease_owner).await?;
            self.repo.release_lease(job_id, &lease_owner).await?;
            return Ok(());
        }

        match handle_result {
            Ok(result) => {
                self.repo
                    .update_result(job_id, &lease_owner, result)
                    .await?;
                self.repo
                    .transition(
                        job_id,
                        &lease_owner,
                        JobStatus::Running,
                        JobStatus::Succeeded,
                    )
                    .await?;
                info!(job_id = %job_id, kind = %kind, "job succeeded");
            }
            Err(e) => {
                warn!(
                    job_id = %job_id,
                    kind = %kind,
                    code = %e.code,
                    msg = %e.message,
                    retryable = %e.retryable,
                    needs_action = %e.needs_action,
                    "job failed"
                );
                let decision = self
                    .retry_policy
                    .decide(&e, job.attempt_count, job.max_attempts);
                match decision {
                    RetryDecision::Retry => {
                        let delay = self.retry_policy.backoff_secs(job.attempt_count);
                        self.repo
                            .fail(job_id, &lease_owner, &e.code, &e.message, true)
                            .await?;
                        self.repo
                            .schedule_retry(job_id, &lease_owner, delay)
                            .await?;
                    }
                    RetryDecision::NonRetryable => {
                        self.repo
                            .fail(job_id, &lease_owner, &e.code, &e.message, false)
                            .await?;
                    }
                    RetryDecision::NeedsAction => {
                        self.repo
                            .mark_needs_action(job_id, &lease_owner, &e.code, &e.message)
                            .await?;
                    }
                }
            }
        }
        self.repo.release_lease(job_id, &lease_owner).await?;
        Ok(())
    }

    /// 永久轮询。
    pub async fn run_forever(&self) -> ! {
        loop {
            if let Err(e) = self.run_once().await {
                error!(error = %e, "worker run_once error");
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

async fn persist_update(
    repo: &dyn JobRepository,
    job_id: JobId,
    worker_id: &str,
    update: ProgressUpdate,
) {
    let _ = match update {
        ProgressUpdate::Progress {
            percent,
            message,
            stage,
        } => {
            repo.update_progress(job_id, worker_id, percent, &message, &stage)
                .await
        }
        ProgressUpdate::Stage { stage } => {
            repo.update_progress(job_id, worker_id, 0.0, "", &stage)
                .await
        }
        ProgressUpdate::Message { message } => {
            repo.update_progress(job_id, worker_id, 0.0, &message, "")
                .await
        }
        ProgressUpdate::Result(result) => repo.update_result(job_id, worker_id, result).await,
    };
}

pub mod memory_repo;
pub use memory_repo::MemoryRepo;
