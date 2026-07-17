//! 首次验证编排与聚合 job。

use std::sync::Arc;

use async_trait::async_trait;
use job_pipeline::{
    JobContext, JobError, JobHandler, JobId, JobResult, KIND_SOURCE_VALIDATION,
    KIND_SOURCE_VALIDATION_AGGREGATE, ProgressUpdate,
};

use crate::service::source_validation::{
    SourceValidationAggregatePayload, SourceValidationJobPayload, SourceValidationService,
};

pub struct SourceValidationHandler {
    pub service: Arc<SourceValidationService>,
}

#[async_trait]
impl JobHandler for SourceValidationHandler {
    fn kind(&self) -> &str {
        KIND_SOURCE_VALIDATION
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let payload: SourceValidationJobPayload =
            serde_json::from_value(ctx.payload_json).map_err(|e| invalid_payload(job_id, e))?;
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "discover_and_enqueue".into(),
        });
        match self
            .service
            .start_validation_job(payload.validation_run_id)
            .await
        {
            Ok(result) => Ok(result),
            Err(error) => {
                let _ = self
                    .service
                    .fail_validation_job(payload.validation_run_id, &error.code)
                    .await;
                Err(JobError::permanent(error.code, error.message).with_job_id(job_id))
            }
        }
    }
}

pub struct SourceValidationAggregateHandler {
    pub service: Arc<SourceValidationService>,
}

#[async_trait]
impl JobHandler for SourceValidationAggregateHandler {
    fn kind(&self) -> &str {
        KIND_SOURCE_VALIDATION_AGGREGATE
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let payload: SourceValidationAggregatePayload =
            serde_json::from_value(ctx.payload_json).map_err(|e| invalid_payload(job_id, e))?;
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "aggregate_details".into(),
        });
        match self
            .service
            .aggregate_validation_job(payload.validation_run_id)
            .await
        {
            Ok(run) => serde_json::to_value(run).map_err(|e| {
                JobError::permanent("source.validation_serialize_failed", e.to_string())
            }),
            Err(e) if e.code == "source.validation_details_pending" => {
                Err(JobError::retryable(e.code, e.message).with_job_id(job_id))
            }
            Err(e) => Err(JobError::permanent(e.code, e.message).with_job_id(job_id)),
        }
    }
}

fn invalid_payload(job_id: JobId, error: serde_json::Error) -> JobError {
    JobError::permanent("job_pipeline.invalid_payload", error.to_string()).with_job_id(job_id)
}
