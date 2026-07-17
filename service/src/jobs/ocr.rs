//! ocr_article_image / refetch_article job handlers。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crawler_capture::ObjectStore;
use job_pipeline::{
    JobContext, JobError, JobHandler, JobId, JobResult, KIND_OCR_ARTICLE_IMAGE,
    KIND_REFETCH_ARTICLE, ProgressUpdate,
};
use provider_hub::{OcrCallOptions, OcrProvider, OcrRequest, ProviderErrorClass};
use server_core::Clock;
use tokio::sync::Semaphore;
use uuid::Uuid;

use crate::domain::article::{ArticleStatus, ChangeType, EditorKind, OcrBlockStatus};
use crate::repository::article::ArticleRepository;
use crate::service::article_capture::{ArticleCaptureService, FetchArticleDetailPayload};
use crate::service::content_normalizer::ContentNormalizerService;
use crate::service::manual_edit::RefetchArticlePayload;
use crate::service::ocr_planner::OcrArticleImagePayload;
use crate::service::revision::{PublishRevisionInput, RevisionService};

pub struct OcrArticleImageHandler {
    pub articles: Arc<dyn ArticleRepository>,
    pub normalizer: Arc<ContentNormalizerService>,
    pub revisions: Arc<RevisionService>,
    pub ocr: Arc<dyn OcrProvider>,
    pub object_store: Arc<dyn ObjectStore>,
    pub concurrency: Arc<Semaphore>,
    pub timeout: Duration,
    pub clock: Arc<dyn Clock>,
}

#[async_trait]
impl JobHandler for OcrArticleImageHandler {
    fn kind(&self) -> &str {
        KIND_OCR_ARTICLE_IMAGE
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let _ = ctx.progress_tx.send(ProgressUpdate::Stage {
            stage: "ocr_recognize".into(),
        });
        let payload: OcrArticleImagePayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
                JobError::permanent("job_pipeline.invalid_payload", e.to_string())
                    .with_job_id(job_id)
            })?;

        if ctx.is_cancel_requested() {
            let _ = self.ocr.cancel().await;
            return Err(
                JobError::permanent("job_pipeline.cancelled", "OCR job cancelled")
                    .with_job_id(job_id),
            );
        }

        let image_data = self
            .object_store
            .get_bytes(&payload.storage_key)
            .await
            .map_err(|e| {
                JobError::retryable("ocr.object_read_failed", e.to_string()).with_job_id(job_id)
            })?
            .ok_or_else(|| {
                JobError::permanent(
                    "ocr.asset_not_found",
                    "OCR asset is absent from object storage",
                )
                .with_job_id(job_id)
            })?;
        if image_data.is_empty() {
            return Err(
                JobError::permanent("ocr.asset_empty", "OCR asset is empty").with_job_id(job_id)
            );
        }

        let _permit = self.concurrency.acquire().await.map_err(|_| {
            JobError::retryable(
                "ocr.concurrency_closed",
                "OCR concurrency limiter is closed",
            )
            .with_job_id(job_id)
        })?;
        if ctx.is_cancel_requested() {
            let _ = self.ocr.cancel().await;
            return Err(
                JobError::permanent("job_pipeline.cancelled", "OCR job cancelled")
                    .with_job_id(job_id),
            );
        }

        let now = DateTime::<Utc>::from(self.clock.now());
        let request = OcrRequest {
            image_data: Some(image_data),
            // Storage keys are internal references, not URLs consumable by a vendor.
            file_url: None,
        };
        let call = self.ocr.recognize_with_options(
            request,
            OcrCallOptions {
                timeout: self.timeout,
            },
        );
        tokio::pin!(call);
        let mut cancel_poll = tokio::time::interval(Duration::from_millis(50));
        let ocr_result = tokio::select! {
            result = tokio::time::timeout(self.timeout, &mut call) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        let _ = self.ocr.cancel().await;
                        return Err(JobError::retryable("ocr.timeout", "OCR provider call timed out")
                            .with_job_id(job_id));
                    }
                }
            }
            _ = async {
                loop {
                    cancel_poll.tick().await;
                    if ctx.is_cancel_requested() { break; }
                }
            } => {
                let _ = self.ocr.cancel().await;
                return Err(JobError::permanent("job_pipeline.cancelled", "OCR job cancelled")
                    .with_job_id(job_id));
            }
        };

        let existing_attempt = self
            .articles
            .list_ocr_blocks(payload.article_id)
            .await
            .map_err(|e| JobError::retryable("ocr.list_failed", e).with_job_id(job_id))?
            .into_iter()
            .find(|block| {
                block.asset_id == payload.asset_id && block.block_anchor == payload.block_anchor
            })
            .map_or(0, |block| block.attempt);
        let attempt = existing_attempt.saturating_add(1);

        let (
            status,
            text,
            confidence,
            provider,
            model,
            provider_version,
            provider_call_id,
            error_code,
        ) = match ocr_result {
            Ok(resp) => (
                OcrBlockStatus::Succeeded,
                Some(resp.markdown_text),
                resp.confidence,
                Some(resp.trace.provider),
                Some(resp.trace.model),
                resp.trace.version,
                resp.trace.request_id,
                None,
            ),
            Err(e) => {
                if e.class() == ProviderErrorClass::Retryable {
                    self.articles
                        .upsert_ocr_block(crate::domain::article::ArticleOcrBlock {
                            id: Uuid::new_v4(),
                            article_id: payload.article_id,
                            asset_id: payload.asset_id,
                            block_anchor: payload.block_anchor.clone(),
                            recognized_text: None,
                            confidence: None,
                            status: OcrBlockStatus::Pending,
                            provider_ref: Some(self.ocr.id().to_string()),
                            provider: Some(self.ocr.id().to_string()),
                            model: None,
                            provider_version: None,
                            ocr_version: Some(payload.ocr_version.clone()),
                            provider_call_id: None,
                            error_code: Some(e.code().to_string()),
                            attempt,
                            created_at: now,
                            updated_at: now,
                        })
                        .await
                        .map_err(|persist_error| {
                            JobError::retryable("ocr.persist_failed", persist_error)
                                .with_job_id(job_id)
                        })?;
                    return Err(JobError::retryable(e.code(), e.to_string()).with_job_id(job_id));
                }
                tracing::warn!(
                    code = e.code(),
                    article_id = %payload.article_id,
                    asset_id = %payload.asset_id,
                    "OCR failed; article will be ready_with_ocr_warning"
                );
                (
                    OcrBlockStatus::Failed,
                    None,
                    None,
                    Some(self.ocr.id().to_string()),
                    None,
                    None,
                    None,
                    Some(e.code().to_string()),
                )
            }
        };

        self.articles
            .upsert_ocr_block(crate::domain::article::ArticleOcrBlock {
                id: Uuid::new_v4(),
                article_id: payload.article_id,
                asset_id: payload.asset_id,
                block_anchor: payload.block_anchor.clone(),
                recognized_text: text.clone(),
                confidence,
                status,
                provider_ref: provider.clone(),
                provider,
                model,
                provider_version,
                ocr_version: Some(payload.ocr_version.clone()),
                provider_call_id,
                error_code,
                attempt,
                created_at: now,
                updated_at: now,
            })
            .await
            .map_err(|e| JobError::retryable("ocr.persist_failed", e).with_job_id(job_id))?;

        // 汇总全部 OCR 块，决定文章状态；失败不回滚已有文章/revision。
        let blocks = self
            .articles
            .list_ocr_blocks(payload.article_id)
            .await
            .map_err(|e| JobError::retryable("ocr.list_failed", e).with_job_id(job_id))?;

        let any_pending = blocks.iter().any(|b| b.status == OcrBlockStatus::Pending);
        let any_failed = blocks.iter().any(|b| b.status == OcrBlockStatus::Failed);

        if any_pending {
            return Ok(serde_json::json!({
                "ok": true,
                "article_id": payload.article_id,
                "asset_id": payload.asset_id,
                "ocr_status": status.as_str(),
                "pending_remaining": true,
            }));
        }

        let (content_md, _) = self
            .normalizer
            .rebuild_content_with_ocr(payload.article_id)
            .await
            .map_err(|e| JobError::retryable("ocr.rebuild_failed", e).with_job_id(job_id))?;

        let article = self
            .articles
            .get_article(payload.article_id)
            .await
            .map_err(|e| JobError::retryable("ocr.get_article", e).with_job_id(job_id))?
            .ok_or_else(|| {
                JobError::permanent("article.not_found", "文章不存在").with_job_id(job_id)
            })?;

        let final_status = if any_failed {
            ArticleStatus::ReadyWithOcrWarning
        } else {
            ArticleStatus::Ready
        };

        let assets = self
            .articles
            .list_assets(payload.article_id)
            .await
            .map_err(|e| JobError::retryable("ocr.list_assets", e).with_job_id(job_id))?;
        let asset_refs: Vec<Uuid> = assets
            .iter()
            .filter(|a| a.asset_kind == "body_image")
            .map(|a| a.id)
            .collect();

        let published = self
            .revisions
            .publish(PublishRevisionInput {
                article_id: payload.article_id,
                editor_kind: EditorKind::Ocr,
                content_md,
                asset_manifest_hash: article.asset_manifest_hash.clone(),
                change_reason: if any_failed {
                    "ocr_complete_with_warning".into()
                } else {
                    "ocr_complete".into()
                },
                change_type: ChangeType::ArticleUpdated,
                status: final_status,
                asset_refs,
                force_new_revision: false,
                now,
            })
            .await
            .map_err(|e| JobError::retryable("ocr.publish_failed", e).with_job_id(job_id))?;

        let _ = ctx.progress_tx.send(ProgressUpdate::Progress {
            percent: 100.0,
            message: "ocr_done".into(),
            stage: "complete".into(),
        });

        Ok(serde_json::json!({
            "ok": true,
            "article_id": payload.article_id,
            "asset_id": payload.asset_id,
            "ocr_status": status.as_str(),
            "article_status": published.article.status.as_str(),
            "revision_created": published.created_new_revision,
        }))
    }
}

pub struct RefetchArticleHandler {
    pub capture: Arc<ArticleCaptureService>,
    pub normalizer: Arc<ContentNormalizerService>,
}

#[async_trait]
impl JobHandler for RefetchArticleHandler {
    fn kind(&self) -> &str {
        KIND_REFETCH_ARTICLE
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let payload: RefetchArticlePayload = serde_json::from_value(ctx.payload_json.clone())
            .map_err(|e| {
                JobError::permanent("job_pipeline.invalid_payload", e.to_string())
                    .with_job_id(job_id)
            })?;

        // 按原文 URL 进入详情抓取（不走 discover）。
        let detail = FetchArticleDetailPayload {
            source_account_id: payload.source_account_id,
            collector_account_id: None,
            canonical_url: payload.original_url.clone(),
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
            wechat_sn: None,
            title_hint: None,
            candidate_published_at: None,
            candidate_author: None,
            candidate_cover_url: None,
            candidate_evidence: None,
            source_heartbeat_event_id: None,
            heartbeat_manual: false,
            heartbeat_recalculate_next: false,
            discovered_via: "refetch".into(),
        };

        let outcome = self
            .capture
            .capture_detail(&detail)
            .await
            .map_err(|error| {
                JobError::retryable("refetch.capture_failed", error.to_string()).with_job_id(job_id)
            })?;

        Ok(serde_json::json!({
            "ok": true,
            "article_id": outcome.article_id,
            "created": outcome.created,
        }))
    }
}
