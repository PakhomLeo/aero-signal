//! ContentNormalizer：抓取后规范 Markdown、发 crawler revision、规划 OCR。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use server_core::Clock;
use uuid::Uuid;

use crate::domain::article::{Article, ArticleStatus, ChangeType, EditorKind, OcrBlockStatus};
use crate::repository::article::ArticleRepository;
use crate::service::markdown_builder::{ContentBlock, MarkdownBuilder};
use crate::service::ocr_planner::OcrJobPlanner;
use crate::service::revision::{PublishRevisionInput, RevisionService};

#[derive(Debug, Clone)]
pub struct NormalizeOutcome {
    pub article: Article,
    pub ocr_job_ids: Vec<Uuid>,
    pub content_md: String,
}

pub struct ContentNormalizerService {
    articles: Arc<dyn ArticleRepository>,
    revisions: Arc<RevisionService>,
    ocr_planner: Arc<OcrJobPlanner>,
    markdown: MarkdownBuilder,
    clock: Arc<dyn Clock>,
    low_confidence_threshold: f64,
}

impl ContentNormalizerService {
    pub fn new(
        articles: Arc<dyn ArticleRepository>,
        revisions: Arc<RevisionService>,
        ocr_planner: Arc<OcrJobPlanner>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            articles,
            revisions,
            ocr_planner,
            markdown: MarkdownBuilder::new(),
            clock,
            low_confidence_threshold: 0.7,
        }
    }

    pub fn low_confidence_threshold(&self) -> f64 {
        self.low_confidence_threshold
    }

    /// 将 captured 文章规范为对外 Markdown，写 crawler revision，并规划 OCR。
    pub async fn normalize_captured(&self, article_id: Uuid) -> Result<NormalizeOutcome, String> {
        let article = self
            .articles
            .get_article(article_id)
            .await?
            .ok_or_else(|| "article.not_found: 文章不存在".to_string())?;

        let assets = self.articles.list_assets(article_id).await?;
        let body_images: Vec<_> = assets
            .iter()
            .filter(|a| a.asset_kind == "body_image")
            .cloned()
            .collect();

        let asset_tuples: Vec<_> = body_images
            .iter()
            .map(|a| (a.id, a.storage_key.clone(), a.original_url.clone()))
            .collect();
        let blocks = self
            .markdown
            .from_captured_markdown(&article.content_md, &asset_tuples);
        let content_md = self.markdown.render(&blocks);

        let hashes: Vec<String> = body_images
            .iter()
            .filter_map(|a| a.asset_hash.clone())
            .collect();
        // 空集合也有稳定 hash，才能表达“资产从有到无”，而不是误解为保持旧值。
        let manifest = Some(RevisionService::asset_manifest_hash(&hashes));
        let asset_refs: Vec<Uuid> = body_images.iter().map(|a| a.id).collect();

        let now = DateTime::<Utc>::from(self.clock.now());
        let status = if body_images.is_empty() {
            ArticleStatus::Ready
        } else {
            // 有图则等 OCR；暂保持 captured，OCR 完成后升 ready / warning。
            // 若无 OCR job 可立即 ready。
            ArticleStatus::Captured
        };

        let published = self
            .revisions
            .publish(PublishRevisionInput {
                article_id,
                editor_kind: EditorKind::Crawler,
                content_md: content_md.clone(),
                asset_manifest_hash: manifest,
                change_reason: "normalize_captured".into(),
                change_type: ChangeType::ArticleCreated,
                status: if body_images.is_empty() {
                    ArticleStatus::Ready
                } else {
                    status
                },
                asset_refs: asset_refs.clone(),
                force_new_revision: false,
                now,
            })
            .await?;

        // 为每张图建 pending OCR block + job
        for img in &body_images {
            let anchor = MarkdownBuilder::image_anchor(img.id);
            self.articles
                .upsert_ocr_block(crate::domain::article::ArticleOcrBlock {
                    id: Uuid::new_v4(),
                    article_id,
                    asset_id: img.id,
                    block_anchor: anchor,
                    recognized_text: None,
                    confidence: None,
                    status: OcrBlockStatus::Pending,
                    provider_ref: None,
                    provider: None,
                    model: None,
                    provider_version: None,
                    ocr_version: None,
                    provider_call_id: None,
                    error_code: None,
                    attempt: 0,
                    created_at: now,
                    updated_at: now,
                })
                .await?;
        }

        let ocr_job_ids = self
            .ocr_planner
            .plan_for_assets(article_id, &body_images)
            .await?;

        // 无 OCR 任务时已 ready；有任务则保持 captured 直至 OCR 收尾。
        if ocr_job_ids.is_empty() && published.article.status == ArticleStatus::Captured {
            let again = self
                .revisions
                .publish(PublishRevisionInput {
                    article_id,
                    editor_kind: EditorKind::Crawler,
                    content_md: content_md.clone(),
                    asset_manifest_hash: published.article.asset_manifest_hash.clone(),
                    change_reason: "normalize_no_ocr".into(),
                    change_type: ChangeType::ArticleUpdated,
                    status: ArticleStatus::Ready,
                    asset_refs,
                    force_new_revision: false,
                    now,
                })
                .await?;
            return Ok(NormalizeOutcome {
                article: again.article,
                ocr_job_ids,
                content_md,
            });
        }

        Ok(NormalizeOutcome {
            article: published.article,
            ocr_job_ids,
            content_md,
        })
    }

    /// 根据当前 OCR blocks 重建 content_md。
    pub async fn rebuild_content_with_ocr(
        &self,
        article_id: Uuid,
    ) -> Result<(String, Vec<ContentBlock>), String> {
        let article = self
            .articles
            .get_article(article_id)
            .await?
            .ok_or_else(|| "article.not_found: 文章不存在".to_string())?;
        let assets = self.articles.list_assets(article_id).await?;
        let body_images: Vec<_> = assets
            .iter()
            .filter(|a| a.asset_kind == "body_image")
            .cloned()
            .collect();

        // 从 crawler 版重建，避免人工编辑被 OCR 覆盖结构；若无 crawler 则用当前。
        let base_md = {
            let revs = self.articles.list_revisions(article_id).await?;
            revs.iter()
                .rev()
                .find(|r| r.editor_kind == EditorKind::Crawler)
                .map(|r| r.content_md.clone())
                .unwrap_or_else(|| article.content_md.clone())
        };

        // 从 crawler/当前正文抽段落，再按资产列表重建图片块（保证 OCR anchor 稳定）。
        let paragraphs: Vec<String> = strip_image_lines(&base_md);
        let images: Vec<(Uuid, String)> = body_images
            .iter()
            .map(|a| (a.id, a.storage_key.clone()))
            .collect();
        let mut blocks = self.markdown.build_blocks(&paragraphs, &images);

        let ocr_blocks = self.articles.list_ocr_blocks(article_id).await?;
        for ob in ocr_blocks {
            if let Some(text) = ob.recognized_text
                && (ob.status == OcrBlockStatus::Succeeded
                    || ob.status == OcrBlockStatus::ManualOverride)
            {
                self.markdown.insert_ocr(
                    &mut blocks,
                    &ob.block_anchor,
                    &text,
                    ob.confidence,
                    self.low_confidence_threshold,
                );
            }
        }
        let md = self.markdown.render(&blocks);
        Ok((md, blocks))
    }
}

fn strip_image_lines(md: &str) -> Vec<String> {
    let mut paras = Vec::new();
    let mut buf = String::new();
    for line in md.lines() {
        let t = line.trim();
        if t.starts_with("![") || t.starts_with("<!-- ocr-") || t.starts_with('>') {
            if !buf.trim().is_empty() {
                paras.push(buf.trim().to_string());
                buf.clear();
            }
            continue;
        }
        if t.is_empty() {
            if !buf.trim().is_empty() {
                paras.push(buf.trim().to_string());
                buf.clear();
            }
        } else {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(t);
        }
    }
    if !buf.trim().is_empty() {
        paras.push(buf.trim().to_string());
    }
    paras
}
