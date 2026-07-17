//! ArticleCursorService：组内文章列表/详情与 since 增量。

use std::collections::HashSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use server_core::AppError;
use uuid::Uuid;

use crate::domain::article::{Article, ArticleAsset, ArticleChangeEvent, ChangeType};
use crate::domain::delivery::ApiToken;
use crate::repository::article::ArticleRepository;
use crate::repository::delivery::DeliveryRepository;

pub struct ArticleCursorService {
    articles: Arc<dyn ArticleRepository>,
    delivery: Arc<dyn DeliveryRepository>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryArticleSummary {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub original_title: String,
    pub author: Option<String>,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub current_revision: i32,
    pub status: String,
    pub withdrawn_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryArticleDetail {
    pub schema_version: u32,
    pub article: DeliveryArticleSummary,
    pub content_md: String,
    pub assets: Vec<DeliveryAssetRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryAssetRef {
    pub id: Uuid,
    pub storage_key: String,
    pub asset_kind: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinceEventDto {
    pub event_id: Uuid,
    pub change_sequence: i64,
    pub article_id: Uuid,
    pub article_revision: i32,
    pub change_type: String,
    pub schema_version: i32,
    /// 撤回时为 tombstone；其它为精简 payload。
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SinceResponse {
    pub schema_version: u32,
    pub events: Vec<SinceEventDto>,
    pub next_sequence: i64,
}

impl ArticleCursorService {
    pub fn new(
        articles: Arc<dyn ArticleRepository>,
        delivery: Arc<dyn DeliveryRepository>,
    ) -> Self {
        Self { articles, delivery }
    }

    async fn group_source_set(&self, group_id: Uuid) -> Result<HashSet<Uuid>, AppError> {
        let bindings = self
            .delivery
            .list_group_sources(group_id)
            .await
            .map_err(AppError::internal)?;
        Ok(bindings.into_iter().map(|b| b.source_account_id).collect())
    }

    fn summary(a: &Article) -> DeliveryArticleSummary {
        DeliveryArticleSummary {
            id: a.id,
            source_account_id: a.source_account_id,
            original_title: a.original_title.clone(),
            author: a.author.clone(),
            published_at: a.published_at,
            fetched_at: a.fetched_at,
            current_revision: a.current_revision,
            status: a.status.as_str().to_string(),
            withdrawn_at: a.withdrawn_at,
        }
    }

    fn asset_ref(a: &ArticleAsset) -> DeliveryAssetRef {
        DeliveryAssetRef {
            id: a.id,
            storage_key: a.storage_key.clone(),
            asset_kind: a.asset_kind.clone(),
            width: a.width,
            height: a.height,
            mime_type: a.mime_type.clone(),
        }
    }

    pub async fn list_articles(
        &self,
        token: &ApiToken,
        limit: usize,
    ) -> Result<Vec<DeliveryArticleSummary>, AppError> {
        let sources = self.group_source_set(token.delivery_group_id).await?;
        let ids: Vec<_> = sources.into_iter().collect();
        let rows = self
            .articles
            .list_articles_for_sources(&ids, limit.clamp(1, 200))
            .await
            .map_err(AppError::internal)?;
        Ok(rows.iter().map(Self::summary).collect())
    }

    pub async fn get_article(
        &self,
        token: &ApiToken,
        article_id: Uuid,
    ) -> Result<DeliveryArticleDetail, AppError> {
        let sources = self.group_source_set(token.delivery_group_id).await?;
        let article = self
            .articles
            .get_article(article_id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("article.not_found", "文章不存在"))?;
        if !sources.contains(&article.source_account_id) {
            return Err(AppError::not_found(
                "article.not_in_group",
                "文章不在 Token 绑定分组内",
            ));
        }
        let assets = self
            .articles
            .list_assets(article_id)
            .await
            .map_err(AppError::internal)?;
        let content_md = if article.status.as_str() == "withdrawn" {
            String::new()
        } else {
            article.content_md.clone()
        };
        Ok(DeliveryArticleDetail {
            schema_version: 1,
            article: Self::summary(&article),
            content_md,
            assets: assets.iter().map(Self::asset_ref).collect(),
        })
    }

    pub async fn since(
        &self,
        token: &ApiToken,
        after_sequence: i64,
        limit: usize,
    ) -> Result<SinceResponse, AppError> {
        if after_sequence < 0 {
            return Err(AppError::cursor_invalid("change_sequence 不能为负"));
        }
        let sources = self.group_source_set(token.delivery_group_id).await?;
        let ids: Vec<_> = sources.into_iter().collect();
        let events = self
            .articles
            .list_change_events_since(after_sequence, &ids, limit.clamp(1, 500))
            .await
            .map_err(AppError::internal)?;
        let next_sequence = events
            .last()
            .map(|e| e.change_sequence)
            .unwrap_or(after_sequence);
        Ok(SinceResponse {
            schema_version: 1,
            events: events.into_iter().map(Self::event_dto).collect(),
            next_sequence,
        })
    }

    fn event_dto(e: ArticleChangeEvent) -> SinceEventDto {
        let payload = if e.change_type == ChangeType::ArticleWithdrawn {
            serde_json::json!({
                "tombstone": true,
                "article_id": e.article_id,
                "article_revision": e.article_revision,
                "change_type": e.change_type.as_str(),
            })
        } else {
            // 仅稳定 ID / revision / change_type / schema / 资产引用
            serde_json::json!({
                "article_id": e.article_id,
                "article_revision": e.article_revision,
                "change_type": e.change_type.as_str(),
                "schema_version": e.schema_version,
                "asset_refs": e.payload_json.get("asset_refs").cloned().unwrap_or(serde_json::json!([])),
            })
        };
        SinceEventDto {
            event_id: e.event_id,
            change_sequence: e.change_sequence,
            article_id: e.article_id,
            article_revision: e.article_revision,
            change_type: e.change_type.as_str().to_string(),
            schema_version: e.schema_version,
            payload,
        }
    }
}
