//! daily_preflight / collector_probe（立即检查）job handlers。

use std::sync::Arc;

use async_trait::async_trait;
use job_pipeline::{
    JobContext, JobError, JobHandler, JobId, JobResult, KIND_COLLECTOR_PROBE, KIND_DAILY_PREFLIGHT,
    ProgressUpdate,
};

use crate::service::daily_preflight::{DailyPreflightPayload, DailyPreflightService};

pub struct DailyPreflightHandler {
    pub service: Arc<DailyPreflightService>,
}

#[async_trait]
impl JobHandler for DailyPreflightHandler {
    fn kind(&self) -> &str {
        KIND_DAILY_PREFLIGHT
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "preflight".into(),
        });
        let mut payload: DailyPreflightPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
            JobError::permanent("job_pipeline.invalid_payload", e.to_string()).with_job_id(job_id)
        })?;
        // 日任务强制非 immediate，避免误覆盖语义
        payload.immediate_check = false;

        let result =
            self.service.run(&payload).await.map_err(|e| {
                JobError::permanent("health.preflight_failed", e).with_job_id(job_id)
            })?;

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "snapshot_ready".into(),
            stage: "complete".into(),
        });
        Ok(result)
    }
}

/// 管理员立即检查：写独立 run，不写快照。
pub struct ImmediateHealthCheckHandler {
    pub service: Arc<DailyPreflightService>,
}

#[async_trait]
impl JobHandler for ImmediateHealthCheckHandler {
    fn kind(&self) -> &str {
        KIND_COLLECTOR_PROBE
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "immediate_check".into(),
        });
        let mut payload: DailyPreflightPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
            JobError::permanent("job_pipeline.invalid_payload", e.to_string()).with_job_id(job_id)
        })?;
        payload.immediate_check = true;

        let result = self.service.run(&payload).await.map_err(|e| {
            JobError::permanent("health.immediate_check_failed", e).with_job_id(job_id)
        })?;

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "check_run_recorded".into(),
            stage: "complete".into(),
        });
        Ok(result)
    }
}
