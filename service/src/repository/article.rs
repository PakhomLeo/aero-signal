//! 文章仓储 trait（M09 + M10 revision/OCR/outbox）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::article::{
    Article, ArticleAsset, ArticleChangeEvent, ArticleImageDecision, ArticleLink, ArticleOcrBlock,
    ArticleRevision, ArticleStatus, DeliveryOutbox, EditorKind,
};

#[derive(Debug, Clone)]
pub struct UpsertArticleLink {
    pub source_account_id: Uuid,
    pub canonical_url: String,
    pub wechat_biz: Option<String>,
    pub wechat_mid: Option<String>,
    pub wechat_idx: Option<String>,
    pub wechat_sn: Option<String>,
    pub title_hint: Option<String>,
    pub discovered_via: String,
    pub discovered_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct UpsertArticleLinkResult {
    pub link: ArticleLink,
    /// true = 新建；false = 仅更新 discovered_at。
    pub created: bool,
}

/// 同事务提交 revision + 更新文章头 + change_event + outbox。
#[derive(Debug, Clone)]
pub struct CommitRevisionCommand {
    pub article_id: Uuid,
    pub editor_kind: EditorKind,
    pub content_md: String,
    pub content_hash: String,
    pub asset_manifest_hash: Option<String>,
    pub change_reason: String,
    pub change_type: crate::domain::article::ChangeType,
    pub status: ArticleStatus,
    pub asset_refs: Vec<Uuid>,
    pub now: DateTime<Utc>,
    /// 若为 true，即使 hash 未变也强制新 revision（人工恢复历史等）。
    pub force_new_revision: bool,
}

#[derive(Debug, Clone)]
pub struct CommitRevisionResult {
    pub article: Article,
    pub revision: Option<ArticleRevision>,
    pub event: Option<ArticleChangeEvent>,
    pub outbox: Option<DeliveryOutbox>,
    pub created_new_revision: bool,
}

/// Complete crawler capture persisted as one atomic unit.
#[derive(Debug, Clone)]
pub struct CapturedArticleCommit {
    pub article: Article,
    pub assets: Vec<ArticleAsset>,
    pub image_decisions: Vec<ArticleImageDecision>,
    pub change_reason: String,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CapturedArticleCommitResult {
    pub article: Article,
    pub revision: Option<ArticleRevision>,
    pub event: Option<ArticleChangeEvent>,
    pub outbox: Option<DeliveryOutbox>,
    pub created: bool,
    pub changed: bool,
}

#[async_trait]
pub trait ArticleRepository: Send + Sync {
    async fn commit_captured_article(
        &self,
        cmd: CapturedArticleCommit,
    ) -> Result<CapturedArticleCommitResult, String>;
    async fn ensure_source_stub(
        &self,
        source_account_id: Uuid,
        display_name: &str,
    ) -> Result<(), String>;

    async fn upsert_article_link(
        &self,
        cmd: UpsertArticleLink,
    ) -> Result<UpsertArticleLinkResult, String>;

    async fn find_article_by_identity(
        &self,
        source_account_id: Uuid,
        wechat_biz: Option<&str>,
        wechat_mid: Option<&str>,
        wechat_idx: Option<&str>,
        canonical_url: &str,
    ) -> Result<Option<Article>, String>;

    async fn get_article(&self, article_id: Uuid) -> Result<Option<Article>, String>;

    async fn insert_article(&self, article: Article) -> Result<Article, String>;

    async fn insert_asset(&self, asset: ArticleAsset) -> Result<ArticleAsset, String>;

    async fn insert_image_decision(
        &self,
        decision: ArticleImageDecision,
    ) -> Result<ArticleImageDecision, String>;

    async fn list_decisions(&self, article_id: Uuid) -> Result<Vec<ArticleImageDecision>, String>;

    async fn list_assets(&self, article_id: Uuid) -> Result<Vec<ArticleAsset>, String>;

    async fn count_articles_for_source(&self, source_account_id: Uuid) -> Result<u64, String>;

    /// 原子写入 revision + event + outbox（内存实现用同一把锁保证语义）。
    async fn commit_revision(
        &self,
        cmd: CommitRevisionCommand,
    ) -> Result<CommitRevisionResult, String>;

    async fn list_revisions(&self, article_id: Uuid) -> Result<Vec<ArticleRevision>, String>;

    async fn get_revision(
        &self,
        article_id: Uuid,
        revision: i32,
    ) -> Result<Option<ArticleRevision>, String>;

    async fn upsert_ocr_block(&self, block: ArticleOcrBlock) -> Result<ArticleOcrBlock, String>;

    async fn list_ocr_blocks(&self, article_id: Uuid) -> Result<Vec<ArticleOcrBlock>, String>;

    async fn list_change_events(&self, article_id: Uuid)
    -> Result<Vec<ArticleChangeEvent>, String>;

    async fn list_outbox_for_article(
        &self,
        article_id: Uuid,
    ) -> Result<Vec<DeliveryOutbox>, String>;

    /// 按监测公众号集合列出文章（delivery 组内过滤）。
    async fn list_articles_for_sources(
        &self,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<Article>, String>;

    /// 增量：`change_sequence > after_sequence`，且文章 source 在集合内。
    async fn list_change_events_since(
        &self,
        after_sequence: i64,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<ArticleChangeEvent>, String>;

    /// 测试守卫：故意只写 revision/event 不写 outbox（应失败或可检测）。
    async fn commit_revision_without_outbox_for_test(
        &self,
        cmd: CommitRevisionCommand,
    ) -> Result<(), String> {
        let _ = cmd;
        Err("not_supported: 仅内存仓储实现漏写守卫".into())
    }
}
