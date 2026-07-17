//! 保留清理 job 骨架（M15）。
//!
//! 生产调度可周期性调用 `RetentionCleanupService::run_once`；
//! 本模块提供可注册的入口与安全上下文约定。

use std::sync::Arc;

use async_trait::async_trait;
use job_pipeline::{JobContext, JobError, JobHandler, JobId, JobResult, ProgressUpdate};
use serde_json::json;

use crate::domain::retention::RetentionTargetKind;
use crate::service::retention::RetentionCleanupService;

pub use job_pipeline::KIND_RETENTION_CLEANUP;

/// 可选按 kind 过滤；缺省清理全部启用策略。
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct RetentionCleanupPayload {
    pub target_kind: Option<String>,
    pub batch_limit: Option<usize>,
}

pub struct RetentionCleanupHandler {
    pub service: Arc<RetentionCleanupService>,
}

#[async_trait]
impl JobHandler for RetentionCleanupHandler {
    fn kind(&self) -> &str {
        KIND_RETENTION_CLEANUP
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "retention_cleanup".into(),
        });
        let payload: RetentionCleanupPayload =
            serde_json::from_value(ctx.payload_json.clone()).unwrap_or_default();
        let only = payload
            .target_kind
            .as_deref()
            .and_then(RetentionTargetKind::parse);
        let limit = payload.batch_limit.unwrap_or(200);
        let summary =
            self.service.run_once(only, limit).await.map_err(|e| {
                JobError::retryable("retention.cleanup_failed", e).with_job_id(job_id)
            })?;

        // safe_context：仅计数，无凭据
        Ok(json!({
            "scanned": summary.scanned,
            "deleted": summary.deleted,
            "skipped_referenced": summary.skipped_referenced,
            "skipped_disabled": summary.skipped_disabled,
        }))
    }
}
