//! job-pipeline 集成/单元测试（默认走 MemoryRepo，不依赖外网/PG）。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::error::{JobError, JobResult};
use crate::handler::{JobContext, JobHandler, ProgressUpdate};
use crate::kinds::SIGNAL_JOB_KINDS;
use crate::registry::{FakeHandler, JobHandlerRegistry};
use crate::types::{CreateJob, JobId, JobStatus};
use crate::worker::{JobRepository, MemoryRepo, Worker};

struct OkHandler;
#[async_trait]
impl JobHandler for OkHandler {
    fn kind(&self) -> &str {
        "test_ok"
    }
    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        ctx.progress_tx
            .send(ProgressUpdate::Progress {
                percent: 100.0,
                message: "done".into(),
                stage: "complete".into(),
            })
            .ok();
        Ok(json!({"ok": true, "job_id": job_id.to_string()}))
    }
}

struct FailHandler;
#[async_trait]
impl JobHandler for FailHandler {
    fn kind(&self) -> &str {
        "test_fail"
    }
    async fn execute(&self, _: JobId, _: JobContext) -> JobResult<serde_json::Value> {
        Err(JobError::permanent(
            "test_fail.always",
            "this handler always fails",
        ))
    }
}

struct RetryHandler;
#[async_trait]
impl JobHandler for RetryHandler {
    fn kind(&self) -> &str {
        "test_retry"
    }
    async fn execute(&self, _: JobId, _: JobContext) -> JobResult<serde_json::Value> {
        Err(JobError::retryable("test_retry.try_again", "please retry"))
    }
}

struct SlowHandler;
#[async_trait]
impl JobHandler for SlowHandler {
    fn kind(&self) -> &str {
        "test_slow"
    }
    async fn execute(&self, _: JobId, _: JobContext) -> JobResult<serde_json::Value> {
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        Ok(json!({"ok": true}))
    }
}

struct CancellableHandler;
#[async_trait]
impl JobHandler for CancellableHandler {
    fn kind(&self) -> &str {
        "test_cancellable"
    }

    async fn execute(&self, _: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        for _ in 0..40 {
            if ctx.is_cancel_requested() {
                return Err(JobError::permanent("job.cancelled", "cancelled"));
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(25)).await;
        }
        Ok(json!({"ok": true}))
    }
}

#[tokio::test]
async fn progress_channel_is_bounded_and_nonblocking() {
    let (sender, mut receiver) = crate::handler::ProgressSender::channel(1);
    sender
        .send(ProgressUpdate::Stage {
            stage: "first".into(),
        })
        .unwrap();
    assert!(matches!(
        sender.send(ProgressUpdate::Stage {
            stage: "second".into(),
        }),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_))
    ));
    assert!(receiver.recv().await.is_some());
    sender
        .send(ProgressUpdate::Stage {
            stage: "third".into(),
        })
        .unwrap();
}

#[tokio::test]
async fn test_create_and_get_job() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob {
            kind: "test".into(),
            payload_json: json!({"key": "value"}),
            idempotency_key: None,
            max_attempts: None,
            priority: None,
            dependency_keys: vec![],
            cost_budget_limit: None,
            resume_token: None,
            safe_context_json: None,
            created_by: None,
            next_run_at: None,
        })
        .await
        .unwrap();
    assert_eq!(job.status, JobStatus::Queued);
    let fetched = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, job.id);
}

#[tokio::test]
async fn test_idempotency_key_returns_same_id() {
    let repo = MemoryRepo::new();
    let cmd = CreateJob::new("test", "dup-key", json!({"x": 1}));
    let a = repo.create_job(cmd.clone()).await.unwrap();
    let b = repo.create_job(cmd).await.unwrap();
    assert_eq!(a.id, b.id);
}

#[tokio::test]
async fn test_idempotency_key_conflicts_on_kind_or_payload_change() {
    let repo = MemoryRepo::new();
    repo.create_job(CreateJob::new(
        "kind_a",
        "same-key",
        serde_json::json!({"value": 1}),
    ))
    .await
    .unwrap();

    let error = repo
        .create_job(CreateJob::new(
            "kind_a",
            "same-key",
            serde_json::json!({"value": 2}),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, "job_pipeline.idempotency_conflict");

    let error = repo
        .create_job(CreateJob::new(
            "kind_b",
            "same-key",
            serde_json::json!({"value": 1}),
        ))
        .await
        .unwrap_err();
    assert_eq!(error.code, "job_pipeline.idempotency_conflict");
}

#[tokio::test]
async fn test_worker_success() {
    let repo = MemoryRepo::new();
    repo.create_job(CreateJob::new("test_ok", "ok-1", json!({})))
        .await
        .unwrap();

    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(OkHandler));
    worker.run_once().await.unwrap();

    let jobs = repo.list_jobs(Some("test_ok"), None, 10).await.unwrap();
    assert_eq!(jobs[0].status, JobStatus::Succeeded);
    assert!(jobs[0].result_json.is_some());
}

#[tokio::test]
async fn test_worker_fail_permanent() {
    let repo = MemoryRepo::new();
    let mut cmd = CreateJob::new("test_fail", "fail-1", json!({}));
    cmd.max_attempts = Some(1);
    repo.create_job(cmd).await.unwrap();

    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(FailHandler));
    worker.run_once().await.unwrap();

    let jobs = repo.list_jobs(Some("test_fail"), None, 10).await.unwrap();
    assert_eq!(jobs[0].status, JobStatus::Failed);
}

#[tokio::test]
async fn test_worker_retry() {
    let repo = MemoryRepo::new();
    let mut cmd = CreateJob::new("test_retry", "retry-1", json!({}));
    cmd.max_attempts = Some(3);
    repo.create_job(cmd).await.unwrap();

    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(RetryHandler));
    worker.run_once().await.unwrap();

    let jobs = repo.list_jobs(Some("test_retry"), None, 10).await.unwrap();
    assert_eq!(jobs[0].status, JobStatus::RetryWait);

    // next_run_at 在未来，不应再次抢占。
    worker.run_once().await.unwrap();
    let jobs = repo.list_jobs(Some("test_retry"), None, 10).await.unwrap();
    assert_eq!(jobs[0].status, JobStatus::RetryWait);
}

#[tokio::test]
async fn test_cancel() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob::new("test_ok", "cancel-1", json!({})))
        .await
        .unwrap();
    repo.request_cancel(job.id).await.unwrap();
    let j = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Cancelling);

    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(OkHandler));
    worker.run_once().await.unwrap();
    let j = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Cancelled);
}

#[tokio::test]
async fn running_handler_observes_persisted_cancellation() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob::new(
            "test_cancellable",
            "cancel-running",
            json!({}),
        ))
        .await
        .unwrap();
    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(CancellableHandler));
    let task = tokio::spawn(async move { worker.run_once().await });
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    repo.request_cancel(job.id).await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        repo.get_job(job.id).await.unwrap().unwrap().status,
        JobStatus::Cancelled
    );
}

#[tokio::test]
async fn worker_enforces_persisted_execution_timeout() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob::new("test_slow", "timeout-1", json!({})))
        .await
        .unwrap();
    repo.set_execution_timeout(
        job.id,
        chrono::Utc::now() + chrono::Duration::milliseconds(20),
    )
    .await
    .unwrap();

    let worker = Worker::new("w1", Arc::new(repo.clone())).register(Arc::new(SlowHandler));
    worker.run_once().await.unwrap();

    let job = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::RetryWait);
    assert_eq!(
        job.last_error_code.as_deref(),
        Some("job.execution_timeout")
    );
}

#[tokio::test]
async fn test_dual_worker_no_double_execution() {
    let repo = MemoryRepo::new();
    repo.create_job(CreateJob::new("test_slow", "slow-1", json!({})))
        .await
        .unwrap();

    let repo_a = Arc::new(repo.clone());
    let repo_b = Arc::new(repo.clone());
    let w1 = Worker::new("w1", repo_a).register(Arc::new(SlowHandler));
    let w2 = Worker::new("w2", repo_b).register(Arc::new(SlowHandler));

    let t1 = tokio::spawn(async move { w1.run_once().await });
    let t2 = tokio::spawn(async move { w2.run_once().await });
    t1.await.unwrap().unwrap();
    t2.await.unwrap().unwrap();

    let jobs = repo.list_jobs(Some("test_slow"), None, 10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].status, JobStatus::Succeeded);
    // 只应被执行一次（attempt_count == 1）。
    assert_eq!(jobs[0].attempt_count, 1);
}

#[tokio::test]
async fn stale_worker_cannot_commit_after_lease_reclaimed() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob::new("test_ok", "fenced-1", json!({})))
        .await
        .unwrap();
    let first = repo
        .acquire_next("old-worker", &[], 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.id, job.id);
    tokio::time::sleep(tokio::time::Duration::from_millis(1_100)).await;
    let second = repo
        .acquire_next("new-worker", &[], 1)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.id, job.id);

    // The old worker's writes are fenced by the current lease owner.
    repo.update_result(job.id, "old-worker", json!({"stale": true}))
        .await
        .unwrap();
    repo.transition(
        job.id,
        "old-worker",
        JobStatus::Running,
        JobStatus::Succeeded,
    )
    .await
    .unwrap();
    let current = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(current.status, JobStatus::Running);
    assert!(current.result_json.is_none());

    repo.update_result(job.id, "new-worker", json!({"fresh": true}))
        .await
        .unwrap();
    repo.transition(
        job.id,
        "new-worker",
        JobStatus::Running,
        JobStatus::Succeeded,
    )
    .await
    .unwrap();
    assert_eq!(
        repo.get_job(job.id).await.unwrap().unwrap().status,
        JobStatus::Succeeded
    );
}

#[tokio::test]
async fn test_lease_reclaim_after_crash() {
    let repo = MemoryRepo::new();
    let job = repo
        .create_job(CreateJob::new("test_ok", "crash-1", json!({})))
        .await
        .unwrap();

    // 模拟 worker 抢占后崩溃：手动设短 lease 并过期。
    {
        let acquired = repo
            .acquire_next("dead-worker", &["test_ok".into()], 1)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(acquired.id, job.id);
        assert_eq!(acquired.status, JobStatus::Running);
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(1100)).await;
    let n = repo.reclaim_expired_leases().await.unwrap();
    assert!(n >= 1);

    let j = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Queued);
    assert!(j.locked_by.is_none());

    let worker = Worker::new("w-recover", Arc::new(repo.clone())).register(Arc::new(OkHandler));
    worker.run_once().await.unwrap();
    let j = repo.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Succeeded);
}

#[tokio::test]
async fn test_fake_handler_success_fail_cancel_paths() {
    let repo = MemoryRepo::new();
    let mut reg = JobHandlerRegistry::new();
    for kind in SIGNAL_JOB_KINDS {
        reg.register(Arc::new(FakeHandler {
            kind: (*kind).to_string(),
        }));
    }
    assert!(reg.assert_signal_kinds_registered().is_ok());

    // success
    let ok_job = repo
        .create_job(CreateJob::new(
            "daily_preflight",
            "fake-ok",
            json!({"mode": "success"}),
        ))
        .await
        .unwrap();
    let worker = Worker::new("w-fake", Arc::new(repo.clone())).with_registry(&reg);
    worker.run_once().await.unwrap();
    let j = repo.get_job(ok_job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Succeeded);

    // fail
    let fail_job = repo
        .create_job(CreateJob::new(
            "build_daily_plan",
            "fake-fail",
            json!({"mode": "fail"}),
        ))
        .await
        .unwrap();
    worker.run_once().await.unwrap();
    let j = repo.get_job(fail_job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Failed);

    // cancel path via request_cancel before run
    let cancel_job = repo
        .create_job(CreateJob::new(
            "collector_probe",
            "fake-cancel",
            json!({"mode": "success"}),
        ))
        .await
        .unwrap();
    repo.request_cancel(cancel_job.id).await.unwrap();
    worker.run_once().await.unwrap();
    let j = repo.get_job(cancel_job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::Cancelled);

    // needs_action
    let na_job = repo
        .create_job(CreateJob::new(
            "source_validation",
            "fake-na",
            json!({"mode": "needs_action"}),
        ))
        .await
        .unwrap();
    worker.run_once().await.unwrap();
    let j = repo.get_job(na_job.id).await.unwrap().unwrap();
    assert_eq!(j.status, JobStatus::NeedsAction);
}

#[tokio::test]
async fn test_create_rejects_unsafe_context() {
    let repo = MemoryRepo::new();
    let mut cmd = CreateJob::new("daily_preflight", "unsafe-1", json!({}));
    cmd.safe_context_json = Some(json!({"cookie": "x"}));
    let err = repo.create_job(cmd).await.unwrap_err();
    assert_eq!(err.code, "job_pipeline.unsafe_context");
}

#[tokio::test]
async fn test_list_filtered() {
    let repo = MemoryRepo::new();
    for i in 0..5 {
        repo.create_job(CreateJob {
            kind: format!("kind_{}", i % 2),
            payload_json: json!({"i": i}),
            idempotency_key: Some(format!("list-{i}")),
            max_attempts: None,
            priority: None,
            dependency_keys: vec![],
            cost_budget_limit: None,
            resume_token: None,
            safe_context_json: None,
            created_by: None,
            next_run_at: None,
        })
        .await
        .unwrap();
    }
    assert_eq!(repo.list_jobs(None, None, 100).await.unwrap().len(), 5);
    assert_eq!(
        repo.list_jobs(Some("kind_0"), None, 100)
            .await
            .unwrap()
            .len(),
        3
    );
}
