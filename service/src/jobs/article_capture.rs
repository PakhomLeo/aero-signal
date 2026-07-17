//! fetch_article_detail / fetch_source job handlers。

use std::sync::Arc;

use async_trait::async_trait;
use job_pipeline::{
    JobContext, JobError, JobHandler, JobId, JobResult, KIND_FETCH_ARTICLE_DETAIL,
    KIND_FETCH_SOURCE, KIND_HISTORY_BACKFILL, KIND_RETRY_ARTICLE_ASSETS, ProgressUpdate,
};

use crate::domain::source::HeartbeatOutcome;
use crate::service::article_capture::{
    ArticleCaptureService, FetchArticleDetailPayload, FetchSourcePayload, HistoryBackfillPayload,
    RetryArticleAssetsPayload,
};
use crate::service::capture_failure::{AccountEffect, CaptureFailure, SourceEffect};

pub struct FetchArticleDetailHandler {
    pub service: Arc<ArticleCaptureService>,
}

#[async_trait]
impl JobHandler for FetchArticleDetailHandler {
    fn kind(&self) -> &str {
        KIND_FETCH_ARTICLE_DETAIL
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "fetch_html".into(),
        });
        let payload: FetchArticleDetailPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
            JobError::permanent("job_pipeline.invalid_payload", e.to_string()).with_job_id(job_id)
        })?;

        let outcome = self
            .service
            .capture_detail_with_cancel(&payload, Some(ctx.cancel_requested.as_ref()))
            .await
            .map_err(|e| map_capture_err(job_id, e))?;

        let heartbeat = self
            .service
            .apply_detail_success_heartbeat(&payload)
            .await
            .map_err(|e| map_capture_err(job_id, e))?;

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "captured".into(),
            stage: "complete".into(),
        });

        Ok(serde_json::json!({
            "ok": true,
            "job_id": job_id.to_string(),
            "article_id": outcome.article_id,
            "identity_key": outcome.identity_key,
            "created": outcome.created,
            "kept_assets": outcome.kept_assets,
            "decisions": outcome.decisions,
            "asset_warnings": outcome.asset_warnings,
            "asset_retry_job_id": outcome.asset_retry_job_id,
            "heartbeat": heartbeat,
        }))
    }
}

pub struct FetchSourceHandler {
    pub service: Arc<ArticleCaptureService>,
}

pub struct HistoryBackfillHandler {
    pub service: Arc<ArticleCaptureService>,
}

#[async_trait]
impl JobHandler for HistoryBackfillHandler {
    fn kind(&self) -> &str {
        KIND_HISTORY_BACKFILL
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let payload: HistoryBackfillPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
                JobError::permanent("job_pipeline.invalid_payload", e.to_string())
                    .with_job_id(job_id)
            })?;
        self.service
            .run_history_backfill_page(&payload, Some(ctx.cancel_requested.as_ref()))
            .await
            .map_err(|e| map_capture_err(job_id, e))
    }
}

pub struct RetryArticleAssetsHandler {
    pub service: Arc<ArticleCaptureService>,
}

#[async_trait]
impl JobHandler for RetryArticleAssetsHandler {
    fn kind(&self) -> &str {
        KIND_RETRY_ARTICLE_ASSETS
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let payload: RetryArticleAssetsPayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
            JobError::permanent("job_pipeline.invalid_payload", e.to_string()).with_job_id(job_id)
        })?;
        let outcome = self
            .service
            .retry_article_assets(&payload, Some(ctx.cancel_requested.as_ref()))
            .await
            .map_err(|e| map_capture_err(job_id, e))?;
        if outcome.asset_warnings > 0 {
            return Err(JobError::retryable(
                "crawler.asset_retry_incomplete",
                format!(
                    "{} article assets still unavailable",
                    outcome.asset_warnings
                ),
            )
            .with_job_id(job_id));
        }
        Ok(serde_json::json!({
            "ok": true,
            "article_id": outcome.article_id,
            "asset_warnings": 0,
        }))
    }
}

#[async_trait]
impl JobHandler for FetchSourceHandler {
    fn kind(&self) -> &str {
        KIND_FETCH_SOURCE
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "discover".into(),
        });
        let payload: FetchSourcePayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
                JobError::permanent("job_pipeline.invalid_payload", e.to_string())
                    .with_job_id(job_id)
            })?;

        let result = match self.service.fetch_source(&payload).await {
            Ok(v) => v,
            Err(e) => {
                let failure = Some(heartbeat_failure(&e));
                let _ = self
                    .service
                    .apply_fetch_heartbeat(&payload, false, failure)
                    .await;
                return Err(map_capture_err(job_id, e));
            }
        };

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "detail_jobs_enqueued".into(),
            stage: "complete".into(),
        });

        Ok(result)
    }
}

fn map_capture_err(job_id: JobId, failure: CaptureFailure) -> JobError {
    let mut err = match failure.account_effect {
        AccountEffect::NeedsRelogin | AccountEffect::NeedsAction => {
            JobError::needs_action(failure.code.clone(), failure.message.clone())
        }
        _ if failure.retryable => {
            JobError::retryable(failure.code.clone(), failure.message.clone())
        }
        _ => JobError::permanent(failure.code.clone(), failure.message.clone()),
    };
    err = err.with_job_id(job_id);
    err
}

fn heartbeat_failure(failure: &CaptureFailure) -> HeartbeatOutcome {
    match failure.source_effect {
        SourceEffect::AccessBlocked => HeartbeatOutcome::AccessBlocked,
        _ => HeartbeatOutcome::TechnicalFailure,
    }
}

#[cfg(test)]
mod tests {
    use super::heartbeat_failure;
    use crate::domain::source::HeartbeatOutcome;
    use crate::service::capture_failure::CaptureFailure;

    #[test]
    fn task_mapping_reads_structured_failure_fields() {
        let failure = CaptureFailure::from_safe_message("crawler.fetch_failed: timeout");
        assert_eq!(failure.code, "crawler.fetch_failed");
        assert!(failure.retryable);
    }

    #[test]
    fn platform_codes_are_not_counted_as_empty_fetches() {
        assert_eq!(
            heartbeat_failure(&CaptureFailure::from_safe_message(
                "crawler.rate_limited: wait"
            )),
            HeartbeatOutcome::AccessBlocked
        );
        assert_eq!(
            heartbeat_failure(&CaptureFailure::from_safe_message(
                "egress.network_unreachable: down"
            )),
            HeartbeatOutcome::TechnicalFailure
        );
    }
}
