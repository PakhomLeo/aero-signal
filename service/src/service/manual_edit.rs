//! ManualEditService：人工编辑 / 恢复版本 / reocr / refetch。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use job_pipeline::kinds::idempotency;
use job_pipeline::{CreateJob, JobRepository, KIND_REFETCH_ARTICLE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::audit::{ACTOR_SYSTEM_ADMINISTRATOR, AuditWriter};
use crate::domain::article::{
    Article, ArticleRevision, ArticleStatus, ChangeType, EditorKind, OcrBlockStatus,
};
use crate::repository::article::ArticleRepository;
use crate::service::ocr_planner::OcrJobPlanner;
use crate::service::revision::{PublishRevisionInput, RevisionService};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchArticleContentCommand {
    pub content_md: String,
    pub change_reason: Option<String>,
    /// Optimistic concurrency guard supplied by the editor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleDetailDto {
    pub article: Article,
    pub revisions: Vec<ArticleRevision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReocrResponse {
    pub job_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReocrCommand {
    pub asset_id: Option<Uuid>,
    #[serde(default)]
    pub failed_only: bool,
    /// Client supplied command key. Retries must reuse this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefetchCommand {
    /// Client supplied command key. Retries must reuse this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefetchResponse {
    pub job_id: Uuid,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefetchArticlePayload {
    pub article_id: Uuid,
    pub original_url: String,
    pub source_account_id: Uuid,
    pub requested_at: String,
}

pub struct ManualEditService {
    articles: Arc<dyn ArticleRepository>,
    revisions: Arc<RevisionService>,
    ocr_planner: Arc<OcrJobPlanner>,
    jobs: Arc<dyn JobRepository>,
    clock: Arc<dyn Clock>,
    audit: Option<AuditWriter>,
}

impl ManualEditService {
    pub fn new(
        articles: Arc<dyn ArticleRepository>,
        revisions: Arc<RevisionService>,
        ocr_planner: Arc<OcrJobPlanner>,
        jobs: Arc<dyn JobRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            articles,
            revisions,
            ocr_planner,
            jobs,
            clock,
            audit: None,
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    pub async fn get(&self, article_id: Uuid) -> Result<ArticleDetailDto, AppError> {
        let article = self
            .articles
            .get_article(article_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("article.not_found", "文章不存在"))?;
        let revisions = self
            .articles
            .list_revisions(article_id)
            .await
            .map_err(AppError::internal)?;
        Ok(ArticleDetailDto { article, revisions })
    }

    /// 人工编辑：新 revision（administrator），保留全部历史含 crawler 版。
    pub async fn patch_content(
        &self,
        article_id: Uuid,
        cmd: PatchArticleContentCommand,
    ) -> Result<ArticleDetailDto, AppError> {
        let article = self
            .articles
            .get_article(article_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("article.not_found", "文章不存在"))?;

        let now = DateTime::<Utc>::from(self.clock.now());
        if let Some(expected) = cmd.expected_revision
            && expected != article.current_revision
        {
            return Err(AppError::new(
                "article.revision_conflict",
                "article changed since it was loaded; refresh before saving",
            ));
        }
        let assets = self
            .articles
            .list_assets(article_id)
            .await
            .map_err(AppError::internal)?;
        let asset_refs: Vec<_> = assets
            .iter()
            .filter(|a| a.asset_kind == "body_image")
            .map(|a| a.id)
            .collect();

        self.revisions
            .publish(PublishRevisionInput {
                article_id,
                editor_kind: EditorKind::Administrator,
                content_md: cmd.content_md,
                asset_manifest_hash: article.asset_manifest_hash.clone(),
                change_reason: cmd.change_reason.unwrap_or_else(|| "manual_edit".into()),
                change_type: ChangeType::ArticleEdited,
                status: match article.status {
                    ArticleStatus::Captured => ArticleStatus::Ready,
                    other => other,
                },
                asset_refs,
                force_new_revision: true,
                now,
            })
            .await
            .map_err(AppError::internal)?;

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "article.edited",
                    Some("article"),
                    Some(&article_id.to_string()),
                    json!({
                        "change_type": "article_edited",
                    }),
                    now,
                )
                .await;
        }

        self.get(article_id).await
    }

    /// 恢复历史版本：复制该 revision 内容为新 administrator revision，不删除旧版。
    pub async fn restore_revision(
        &self,
        article_id: Uuid,
        revision: i32,
    ) -> Result<ArticleDetailDto, AppError> {
        let rev = self
            .articles
            .get_revision(article_id, revision)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("revision.not_found", "版本不存在"))?;
        self.patch_content(
            article_id,
            PatchArticleContentCommand {
                content_md: rev.content_md,
                change_reason: Some(format!("restore_revision:{revision}")),
                expected_revision: None,
            },
        )
        .await
    }

    pub async fn reocr(
        &self,
        article_id: Uuid,
        command: ReocrCommand,
    ) -> Result<ReocrResponse, AppError> {
        let article = self
            .articles
            .get_article(article_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("article.not_found", "文章不存在"))?;
        let mut assets = self
            .articles
            .list_assets(article_id)
            .await
            .map_err(AppError::internal)?;
        assets.retain(|asset| asset.asset_kind == "body_image" && asset.asset_hash.is_some());
        if let Some(asset_id) = command.asset_id {
            assets.retain(|asset| asset.id == asset_id);
            if assets.is_empty() {
                return Err(AppError::not_found(
                    "article.asset_not_found",
                    "文章图片不存在",
                ));
            }
        }
        let existing_blocks = self
            .articles
            .list_ocr_blocks(article_id)
            .await
            .map_err(AppError::internal)?;
        if command.failed_only {
            let failed: std::collections::HashSet<Uuid> = existing_blocks
                .iter()
                .filter(|block| block.status == crate::domain::article::OcrBlockStatus::Failed)
                .map(|block| block.asset_id)
                .collect();
            assets.retain(|asset| failed.contains(&asset.id));
        }
        let selected: std::collections::HashSet<Uuid> = assets.iter().map(|a| a.id).collect();
        let selected_blocks: Vec<_> = existing_blocks
            .iter()
            .filter(|block| selected.contains(&block.asset_id))
            .cloned()
            .collect();
        if selected_blocks.len() != assets.len() {
            return Err(AppError::new(
                "ocr.block_not_found",
                "部分文章图片缺少 OCR block，需先重新抓取文章",
            ));
        }
        let reset_at = DateTime::<Utc>::from(self.clock.now());
        for mut block in selected_blocks.clone() {
            block.status = OcrBlockStatus::Pending;
            block.error_code = None;
            block.updated_at = reset_at;
            self.articles
                .upsert_ocr_block(block)
                .await
                .map_err(AppError::internal)?;
        }
        // 显式重跑创建新的 run；不会复用已经终止的旧 job。
        let run_id = command.request_id.clone().unwrap_or_else(|| {
            let mut asset_keys: Vec<_> = assets
                .iter()
                .filter_map(|asset| asset.asset_hash.as_deref())
                .collect();
            asset_keys.sort_unstable();
            format!(
                "article-revision:{}:{}:{}",
                article.current_revision,
                command.failed_only,
                asset_keys.join(",")
            )
        });
        let planned = self
            .ocr_planner
            .plan_rerun_for_assets(article_id, &assets, &run_id)
            .await;
        let job_ids = match planned {
            Ok(ids) => ids,
            Err(error) => {
                for block in selected_blocks {
                    let _ = self.articles.upsert_ocr_block(block).await;
                }
                return Err(AppError::internal(error));
            }
        };
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "article.reocr",
                    Some("article"),
                    Some(&article_id.to_string()),
                    json!({
                        "job_count": job_ids.len(),
                        "run_id": run_id,
                        "asset_id": command.asset_id,
                        "failed_only": command.failed_only,
                    }),
                    DateTime::<Utc>::from(self.clock.now()),
                )
                .await;
        }
        Ok(ReocrResponse { job_ids })
    }

    pub async fn refetch(
        &self,
        article_id: Uuid,
        command: RefetchCommand,
    ) -> Result<RefetchResponse, AppError> {
        let article = self
            .articles
            .get_article(article_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("article.not_found", "文章不存在"))?;
        let now = DateTime::<Utc>::from(self.clock.now());
        let request_key = command
            .request_id
            .unwrap_or_else(|| format!("article-revision-{}", article.current_revision));
        let key = idempotency::refetch(&article_id.to_string(), &request_key);
        let requested_at = now.timestamp_millis().to_string();
        let payload = RefetchArticlePayload {
            article_id,
            original_url: article.original_url.clone(),
            source_account_id: article.source_account_id,
            requested_at: requested_at.clone(),
        };
        let job = self
            .jobs
            .create_job(CreateJob::new(
                KIND_REFETCH_ARTICLE,
                key.clone(),
                serde_json::to_value(&payload).map_err(|e| AppError::internal(e.to_string()))?,
            ))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;
        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "article.refetch",
                    Some("article"),
                    Some(&article_id.to_string()),
                    json!({
                        "job_id": job.id.as_uuid().to_string(),
                        "source_account_id": article.source_account_id.to_string(),
                    }),
                    now,
                )
                .await;
        }
        Ok(RefetchResponse {
            job_id: *job.id.as_uuid(),
            idempotency_key: key,
        })
    }
}
