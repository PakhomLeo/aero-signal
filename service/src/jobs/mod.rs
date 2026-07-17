//! 后台任务：Signal job kind 注册表与 worker 装配。

use std::sync::Arc;

use crawler_capture::ObjectStore;
use job_pipeline::{
    JobHandlerRegistry, KIND_BUILD_DAILY_PLAN, KIND_COLLECTOR_PROBE, KIND_DAILY_PREFLIGHT,
    KIND_FETCH_ARTICLE_DETAIL, KIND_FETCH_SOURCE, KIND_HISTORY_BACKFILL, KIND_OCR_ARTICLE_IMAGE,
    KIND_REFETCH_ARTICLE, KIND_RETENTION_CLEANUP, KIND_RETRY_ARTICLE_ASSETS,
    KIND_SOURCE_VALIDATION, KIND_SOURCE_VALIDATION_AGGREGATE, NotImplementedHandler,
    SIGNAL_JOB_KINDS,
};
use provider_hub::OcrProvider;
use server_core::Clock;

pub mod article_capture;
pub mod daily_plan;
pub mod daily_preflight;
pub mod ocr;
pub mod retention;
pub mod source_validation;

pub use article_capture::{
    FetchArticleDetailHandler, FetchSourceHandler, HistoryBackfillHandler,
    RetryArticleAssetsHandler,
};
pub use daily_plan::BuildDailyPlanHandler;
pub use daily_preflight::{DailyPreflightHandler, ImmediateHealthCheckHandler};
pub use ocr::{OcrArticleImageHandler, RefetchArticleHandler};
pub use retention::{RetentionCleanupHandler, RetentionCleanupPayload};
pub use source_validation::{SourceValidationAggregateHandler, SourceValidationHandler};

use crate::repository::article::ArticleRepository;
use crate::service::article_capture::ArticleCaptureService;
use crate::service::content_normalizer::ContentNormalizerService;
use crate::service::daily_plan::DailyPlanService;
use crate::service::daily_preflight::DailyPreflightService;
use crate::service::retention::RetentionCleanupService;
use crate::service::revision::RevisionService;
use crate::service::source_validation::SourceValidationService;

/// 构建 Signal 全部 kind 的注册表。
pub fn signal_job_registry() -> JobHandlerRegistry {
    signal_job_registry_with(None, None, None, None, None, None)
}

/// 可注入 ArticleCaptureService 的注册表。
pub fn signal_job_registry_with_capture(
    capture: Option<Arc<ArticleCaptureService>>,
) -> JobHandlerRegistry {
    signal_job_registry_with(capture, None, None, None, None, None)
}

/// 可注入 capture / preflight / daily-plan / normalizer / retention 的注册表。
pub fn signal_job_registry_with(
    capture: Option<Arc<ArticleCaptureService>>,
    preflight: Option<Arc<DailyPreflightService>>,
    daily_plan: Option<Arc<DailyPlanService>>,
    normalizer: Option<NormalizerJobDeps>,
    retention: Option<Arc<RetentionCleanupService>>,
    source_validation: Option<Arc<SourceValidationService>>,
) -> JobHandlerRegistry {
    let mut reg = JobHandlerRegistry::new();
    for kind in SIGNAL_JOB_KINDS {
        if *kind == KIND_FETCH_ARTICLE_DETAIL
            && let Some(ref svc) = capture
        {
            reg.register(Arc::new(FetchArticleDetailHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_FETCH_SOURCE
            && let Some(ref svc) = capture
        {
            reg.register(Arc::new(FetchSourceHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_HISTORY_BACKFILL
            && let Some(ref svc) = capture
        {
            reg.register(Arc::new(HistoryBackfillHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_RETRY_ARTICLE_ASSETS
            && let Some(ref svc) = capture
        {
            reg.register(Arc::new(RetryArticleAssetsHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_DAILY_PREFLIGHT
            && let Some(ref svc) = preflight
        {
            reg.register(Arc::new(DailyPreflightHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_COLLECTOR_PROBE
            && let Some(ref svc) = preflight
        {
            reg.register(Arc::new(ImmediateHealthCheckHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_BUILD_DAILY_PLAN
            && let Some(ref svc) = daily_plan
        {
            reg.register(Arc::new(BuildDailyPlanHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_OCR_ARTICLE_IMAGE
            && let Some(ref deps) = normalizer
            && let Some(ref ocr) = deps.ocr
        {
            reg.register(Arc::new(OcrArticleImageHandler {
                articles: deps.articles.clone(),
                normalizer: deps.normalizer.clone(),
                revisions: deps.revisions.clone(),
                ocr: ocr.clone(),
                object_store: deps.object_store.clone(),
                concurrency: deps.ocr_concurrency.clone(),
                timeout: deps.ocr_timeout,
                clock: deps.clock.clone(),
            }));
            continue;
        }
        if *kind == KIND_REFETCH_ARTICLE
            && let Some(ref deps) = normalizer
            && let Some(ref cap) = capture
        {
            reg.register(Arc::new(RefetchArticleHandler {
                capture: cap.clone(),
                normalizer: deps.normalizer.clone(),
            }));
            continue;
        }
        if *kind == KIND_RETENTION_CLEANUP
            && let Some(ref svc) = retention
        {
            reg.register(Arc::new(RetentionCleanupHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_SOURCE_VALIDATION
            && let Some(ref svc) = source_validation
        {
            reg.register(Arc::new(SourceValidationHandler {
                service: svc.clone(),
            }));
            continue;
        }
        if *kind == KIND_SOURCE_VALIDATION_AGGREGATE
            && let Some(ref svc) = source_validation
        {
            reg.register(Arc::new(SourceValidationAggregateHandler {
                service: svc.clone(),
            }));
            continue;
        }
        reg.register(Arc::new(NotImplementedHandler {
            kind: (*kind).to_string(),
        }));
    }
    reg
}

/// OCR / refetch handler 依赖包。
#[derive(Clone)]
pub struct NormalizerJobDeps {
    pub articles: Arc<dyn ArticleRepository>,
    pub normalizer: Arc<ContentNormalizerService>,
    pub revisions: Arc<RevisionService>,
    pub ocr: Option<Arc<dyn OcrProvider>>,
    pub object_store: Arc<dyn ObjectStore>,
    pub ocr_concurrency: Arc<tokio::sync::Semaphore>,
    pub ocr_timeout: std::time::Duration,
    pub clock: Arc<dyn Clock>,
}

impl NormalizerJobDeps {
    pub fn new(
        articles: Arc<dyn ArticleRepository>,
        normalizer: Arc<ContentNormalizerService>,
        revisions: Arc<RevisionService>,
        ocr: Option<Arc<dyn OcrProvider>>,
        object_store: Arc<dyn ObjectStore>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            articles,
            normalizer,
            revisions,
            ocr,
            object_store,
            ocr_concurrency: Arc::new(tokio::sync::Semaphore::new(4)),
            ocr_timeout: std::time::Duration::from_secs(120),
            clock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_all_signal_kinds() {
        let reg = signal_job_registry();
        assert!(reg.assert_signal_kinds_registered().is_ok());
        assert_eq!(reg.len(), SIGNAL_JOB_KINDS.len());
    }

    #[tokio::test]
    async fn missing_dependencies_never_produce_fake_success() {
        use job_pipeline::{JobContext, JobId, ProgressSender};

        let reg = signal_job_registry();
        let handler = reg.get(job_pipeline::KIND_SOURCE_VALIDATION).unwrap();
        let (tx, _rx) = ProgressSender::channel(8);
        let result = handler
            .execute(
                JobId::new(),
                JobContext {
                    worker_id: "test".into(),
                    payload_json: serde_json::json!({}),
                    progress_tx: tx,
                    cancel_requested: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
                        false,
                    )),
                },
            )
            .await;
        assert_eq!(result.unwrap_err().code, "job_pipeline.not_implemented");
    }
}
