//! OcrJobPlanner：为每张保留正文图创建幂等 ocr_article_image job。

use std::sync::Arc;

use job_pipeline::kinds::idempotency;
use job_pipeline::{CreateJob, JobRepository, KIND_OCR_ARTICLE_IMAGE};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::article::ArticleAsset;
use crate::service::markdown_builder::MarkdownBuilder;

/// 当前 OCR 算法/提示版本；变更会使幂等键失效并允许重跑。
pub const OCR_VERSION: &str = "v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrArticleImagePayload {
    pub article_id: Uuid,
    pub asset_id: Uuid,
    pub asset_hash: String,
    pub block_anchor: String,
    pub storage_key: String,
    pub ocr_version: String,
}

pub struct OcrJobPlanner {
    jobs: Arc<dyn JobRepository>,
}

impl OcrJobPlanner {
    pub fn new(jobs: Arc<dyn JobRepository>) -> Self {
        Self { jobs }
    }

    /// 仅为 body_image 且带 asset_hash 的资产建 job。
    pub async fn plan_for_assets(
        &self,
        article_id: Uuid,
        assets: &[ArticleAsset],
    ) -> Result<Vec<Uuid>, String> {
        let mut job_ids = Vec::new();
        for asset in assets.iter().filter(|a| a.asset_kind == "body_image") {
            let Some(hash) = asset.asset_hash.as_deref() else {
                continue;
            };
            let anchor = MarkdownBuilder::image_anchor(asset.id);
            let key = idempotency::ocr(
                &article_id.to_string(),
                &asset.id.to_string(),
                hash,
                OCR_VERSION,
            );
            let payload = OcrArticleImagePayload {
                article_id,
                asset_id: asset.id,
                asset_hash: hash.to_string(),
                block_anchor: anchor,
                storage_key: asset.storage_key.clone(),
                ocr_version: OCR_VERSION.to_string(),
            };
            let job = self
                .jobs
                .create_job(CreateJob::new(
                    KIND_OCR_ARTICLE_IMAGE,
                    key,
                    serde_json::to_value(&payload).map_err(|e| e.to_string())?,
                ))
                .await
                .map_err(|e| e.to_string())?;
            job_ids.push(*job.id.as_uuid());
        }
        Ok(job_ids)
    }

    /// 显式重跑必须创建新 job；同一次请求内仍以 run_id 保持幂等。
    pub async fn plan_rerun_for_assets(
        &self,
        article_id: Uuid,
        assets: &[ArticleAsset],
        run_id: impl ToString,
    ) -> Result<Vec<Uuid>, String> {
        let run_key = run_id.to_string();
        let mut job_ids = Vec::new();
        for asset in assets.iter().filter(|a| a.asset_kind == "body_image") {
            let Some(hash) = asset.asset_hash.as_deref() else {
                continue;
            };
            let payload = OcrArticleImagePayload {
                article_id,
                asset_id: asset.id,
                asset_hash: hash.to_string(),
                block_anchor: MarkdownBuilder::image_anchor(asset.id),
                storage_key: asset.storage_key.clone(),
                ocr_version: OCR_VERSION.to_string(),
            };
            let key = idempotency::ocr_rerun(
                &article_id.to_string(),
                &asset.id.to_string(),
                hash,
                OCR_VERSION,
                &run_key,
            );
            let job = self
                .jobs
                .create_job(CreateJob::new(
                    KIND_OCR_ARTICLE_IMAGE,
                    key,
                    serde_json::to_value(&payload).map_err(|e| e.to_string())?,
                ))
                .await
                .map_err(|e| e.to_string())?;
            job_ids.push(*job.id.as_uuid());
        }
        Ok(job_ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use job_pipeline::{JobId, JobRepository, MemoryRepo};

    fn asset(article_id: Uuid, id: Uuid) -> ArticleAsset {
        ArticleAsset {
            id,
            article_id,
            original_url: Some(format!("https://example.test/{id}.png")),
            storage_key: format!("articles/{article_id}/{id}.png"),
            asset_hash: Some("same-content-hash".into()),
            width: Some(100),
            height: Some(100),
            byte_size: Some(10),
            mime_type: Some("image/png".into()),
            asset_kind: "body_image".into(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn same_hash_in_different_articles_never_reuses_payload() {
        let repo = Arc::new(MemoryRepo::new());
        let planner = OcrJobPlanner::new(repo.clone());
        let article_a = Uuid::new_v4();
        let article_b = Uuid::new_v4();
        let ids_a = planner
            .plan_for_assets(article_a, &[asset(article_a, Uuid::new_v4())])
            .await
            .unwrap();
        let ids_b = planner
            .plan_for_assets(article_b, &[asset(article_b, Uuid::new_v4())])
            .await
            .unwrap();
        assert_ne!(ids_a, ids_b);
        let job_a = repo
            .get_job(JobId::from_raw(ids_a[0]))
            .await
            .unwrap()
            .unwrap();
        let job_b = repo
            .get_job(JobId::from_raw(ids_b[0]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job_a.payload_json["article_id"], article_a.to_string());
        assert_eq!(job_b.payload_json["article_id"], article_b.to_string());
    }

    #[tokio::test]
    async fn explicit_reruns_create_distinct_jobs() {
        let repo = Arc::new(MemoryRepo::new());
        let planner = OcrJobPlanner::new(repo);
        let article_id = Uuid::new_v4();
        let assets = [asset(article_id, Uuid::new_v4())];
        let first = planner
            .plan_rerun_for_assets(article_id, &assets, Uuid::new_v4())
            .await
            .unwrap();
        let second = planner
            .plan_rerun_for_assets(article_id, &assets, Uuid::new_v4())
            .await
            .unwrap();
        assert_ne!(first, second);
    }
}
