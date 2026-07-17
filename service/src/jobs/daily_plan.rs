//! build_daily_plan job handler。

use std::sync::Arc;

use async_trait::async_trait;
use job_pipeline::{
    JobContext, JobError, JobHandler, JobId, JobResult, KIND_BUILD_DAILY_PLAN, ProgressUpdate,
};

use crate::service::daily_plan::{BuildDailyPlanPayload, DailyPlanService};

pub struct BuildDailyPlanHandler {
    pub service: Arc<DailyPlanService>,
}

#[async_trait]
impl JobHandler for BuildDailyPlanHandler {
    fn kind(&self) -> &str {
        KIND_BUILD_DAILY_PLAN
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "build_daily_plan".into(),
        });
        let payload: BuildDailyPlanPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
                JobError::permanent("job_pipeline.invalid_payload", e.to_string())
                    .with_job_id(job_id)
            })?;

        let result =
            self.service.build(&payload).await.map_err(|e| {
                JobError::permanent("daily_plan.build_failed", e).with_job_id(job_id)
            })?;

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "plan_ready".into(),
            stage: "complete".into(),
        });
        Ok(result)
    }
}
