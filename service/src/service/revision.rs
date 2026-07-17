//! RevisionService：内容/资产 hash 变化才新 revision；同事务写 event + outbox。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::article::{
    Article, ArticleChangeEvent, ArticleRevision, ArticleStatus, ChangeType, DeliveryOutbox,
    EditorKind,
};
use crate::repository::article::{ArticleRepository, CommitRevisionCommand, CommitRevisionResult};

#[derive(Debug, Clone)]
pub struct PublishRevisionInput {
    pub article_id: Uuid,
    pub editor_kind: EditorKind,
    pub content_md: String,
    pub asset_manifest_hash: Option<String>,
    pub change_reason: String,
    pub change_type: ChangeType,
    pub status: ArticleStatus,
    pub asset_refs: Vec<Uuid>,
    pub force_new_revision: bool,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PublishRevisionOutcome {
    pub article: Article,
    pub revision: Option<ArticleRevision>,
    pub event: Option<ArticleChangeEvent>,
    pub outbox: Option<DeliveryOutbox>,
    pub created_new_revision: bool,
}

pub struct RevisionService {
    articles: Arc<dyn ArticleRepository>,
}

impl RevisionService {
    pub fn new(articles: Arc<dyn ArticleRepository>) -> Self {
        Self { articles }
    }

    pub fn content_hash(content_md: &str) -> String {
        hex_sha256(content_md.as_bytes())
    }

    pub fn asset_manifest_hash(asset_hashes: &[String]) -> String {
        let mut sorted = asset_hashes.to_vec();
        sorted.sort();
        hex_sha256(sorted.join("\n").as_bytes())
    }

    pub async fn publish(
        &self,
        input: PublishRevisionInput,
    ) -> Result<PublishRevisionOutcome, String> {
        let existing = self
            .articles
            .get_article(input.article_id)
            .await?
            .ok_or_else(|| "article.not_found: 文章不存在".to_string())?;

        let revs = self.articles.list_revisions(input.article_id).await?;
        // 尚无任何 revision 时必须落 crawler/首版，即使 hash 碰巧相同。
        let force = input.force_new_revision || revs.is_empty();

        let content_hash = Self::content_hash(&input.content_md);
        let result: CommitRevisionResult = self
            .articles
            .commit_revision(CommitRevisionCommand {
                article_id: input.article_id,
                editor_kind: input.editor_kind,
                content_md: input.content_md,
                content_hash,
                asset_manifest_hash: input.asset_manifest_hash.or(existing.asset_manifest_hash),
                change_reason: input.change_reason,
                change_type: input.change_type,
                status: input.status,
                asset_refs: input.asset_refs,
                now: input.now,
                force_new_revision: force,
            })
            .await?;

        Ok(PublishRevisionOutcome {
            article: result.article,
            revision: result.revision,
            event: result.event,
            outbox: result.outbox,
            created_new_revision: result.created_new_revision,
        })
    }
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
