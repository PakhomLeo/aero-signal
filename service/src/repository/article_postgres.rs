//! PostgreSQL 文章仓储（revision / OCR / change_events / outbox）。

use async_trait::async_trait;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::article::{
    Article, ArticleAsset, ArticleChangeEvent, ArticleImageDecision, ArticleLink, ArticleOcrBlock,
    ArticleRevision, CHANGE_SCHEMA_VERSION, ChangeType, DeliveryOutbox, EditorKind,
};
use crate::repository::article::{
    ArticleRepository, CapturedArticleCommit, CapturedArticleCommitResult, CommitRevisionCommand,
    CommitRevisionResult, UpsertArticleLink, UpsertArticleLinkResult,
};
use crate::repository::article_postgres_codec::{
    parse_article_status, parse_change_type, parse_editor_kind, parse_ocr_status,
    parse_outbox_status,
};

#[derive(Clone)]
pub struct PgArticleRepository {
    pool: PgPool,
}

impl PgArticleRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_link(row: &PgRow) -> Result<ArticleLink, String> {
    Ok(ArticleLink {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        canonical_url: row.try_get("canonical_url").map_err(|e| e.to_string())?,
        wechat_biz: row.try_get("wechat_biz").map_err(|e| e.to_string())?,
        wechat_mid: row.try_get("wechat_mid").map_err(|e| e.to_string())?,
        wechat_idx: row.try_get("wechat_idx").map_err(|e| e.to_string())?,
        wechat_sn: row.try_get("wechat_sn").map_err(|e| e.to_string())?,
        title_hint: row.try_get("title_hint").map_err(|e| e.to_string())?,
        discovered_via: row.try_get("discovered_via").map_err(|e| e.to_string())?,
        discovered_at: row.try_get("discovered_at").map_err(|e| e.to_string())?,
    })
}

fn map_article(row: &PgRow) -> Result<Article, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(Article {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        article_link_id: row.try_get("article_link_id").map_err(|e| e.to_string())?,
        original_url: row.try_get("original_url").map_err(|e| e.to_string())?,
        canonical_url: row.try_get("canonical_url").map_err(|e| e.to_string())?,
        original_title: row.try_get("original_title").map_err(|e| e.to_string())?,
        author: row.try_get("author").map_err(|e| e.to_string())?,
        published_at: row.try_get("published_at").map_err(|e| e.to_string())?,
        fetched_at: row.try_get("fetched_at").map_err(|e| e.to_string())?,
        current_revision: row.try_get("current_revision").map_err(|e| e.to_string())?,
        content_md: row.try_get("content_md").map_err(|e| e.to_string())?,
        content_hash: row.try_get("content_hash").map_err(|e| e.to_string())?,
        asset_manifest_hash: row
            .try_get("asset_manifest_hash")
            .map_err(|e| e.to_string())?,
        status: parse_article_status(&status_s)?,
        withdrawn_at: row.try_get("withdrawn_at").map_err(|e| e.to_string())?,
    })
}

fn map_asset(row: &PgRow) -> Result<ArticleAsset, String> {
    Ok(ArticleAsset {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        original_url: row.try_get("original_url").map_err(|e| e.to_string())?,
        storage_key: row.try_get("storage_key").map_err(|e| e.to_string())?,
        asset_hash: row.try_get("asset_hash").map_err(|e| e.to_string())?,
        width: row.try_get("width").map_err(|e| e.to_string())?,
        height: row.try_get("height").map_err(|e| e.to_string())?,
        byte_size: row.try_get("byte_size").map_err(|e| e.to_string())?,
        mime_type: row.try_get("mime_type").map_err(|e| e.to_string())?,
        asset_kind: row.try_get("asset_kind").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn map_decision(row: &PgRow) -> Result<ArticleImageDecision, String> {
    Ok(ArticleImageDecision {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        candidate_url: row.try_get("candidate_url").map_err(|e| e.to_string())?,
        decision: row.try_get("decision").map_err(|e| e.to_string())?,
        measured_width: row.try_get("measured_width").map_err(|e| e.to_string())?,
        measured_height: row.try_get("measured_height").map_err(|e| e.to_string())?,
        reason_json: row.try_get("reason_json").map_err(|e| e.to_string())?,
        asset_id: row.try_get("asset_id").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn map_revision(row: &PgRow) -> Result<ArticleRevision, String> {
    let editor_s: String = row.try_get("editor_kind").map_err(|e| e.to_string())?;
    Ok(ArticleRevision {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        revision: row.try_get("revision").map_err(|e| e.to_string())?,
        editor_kind: parse_editor_kind(&editor_s)?,
        content_md: row.try_get("content_md").map_err(|e| e.to_string())?,
        content_hash: row.try_get("content_hash").map_err(|e| e.to_string())?,
        change_reason: row.try_get("change_reason").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn map_ocr_block(row: &PgRow) -> Result<ArticleOcrBlock, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(ArticleOcrBlock {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        asset_id: row.try_get("asset_id").map_err(|e| e.to_string())?,
        block_anchor: row.try_get("block_anchor").map_err(|e| e.to_string())?,
        recognized_text: row.try_get("recognized_text").map_err(|e| e.to_string())?,
        confidence: row.try_get("confidence").map_err(|e| e.to_string())?,
        status: parse_ocr_status(&status_s)?,
        provider_ref: row.try_get("provider_ref").map_err(|e| e.to_string())?,
        provider: row.try_get("provider").map_err(|e| e.to_string())?,
        model: row.try_get("model").map_err(|e| e.to_string())?,
        provider_version: row.try_get("provider_version").map_err(|e| e.to_string())?,
        ocr_version: row.try_get("ocr_version").map_err(|e| e.to_string())?,
        provider_call_id: row.try_get("provider_call_id").map_err(|e| e.to_string())?,
        error_code: row.try_get("error_code").map_err(|e| e.to_string())?,
        attempt: row.try_get("attempt").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_change_event(row: &PgRow) -> Result<ArticleChangeEvent, String> {
    let change_s: String = row.try_get("change_type").map_err(|e| e.to_string())?;
    Ok(ArticleChangeEvent {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        event_id: row.try_get("event_id").map_err(|e| e.to_string())?,
        change_sequence: row.try_get("change_sequence").map_err(|e| e.to_string())?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        article_revision: row.try_get("article_revision").map_err(|e| e.to_string())?,
        change_type: parse_change_type(&change_s)?,
        schema_version: row.try_get("schema_version").map_err(|e| e.to_string())?,
        payload_json: row.try_get("payload_json").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn map_outbox(row: &PgRow) -> Result<DeliveryOutbox, String> {
    let event_s: String = row.try_get("event_type").map_err(|e| e.to_string())?;
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(DeliveryOutbox {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        event_id: row.try_get("event_id").map_err(|e| e.to_string())?,
        change_sequence: row.try_get("change_sequence").map_err(|e| e.to_string())?,
        event_type: parse_change_type(&event_s)?,
        article_id: row.try_get("article_id").map_err(|e| e.to_string())?,
        article_revision: row.try_get("article_revision").map_err(|e| e.to_string())?,
        payload_schema_version: row
            .try_get("payload_schema_version")
            .map_err(|e| e.to_string())?,
        payload_json: row.try_get("payload_json").map_err(|e| e.to_string())?,
        status: parse_outbox_status(&status_s)?,
        attempt_count: row.try_get("attempt_count").map_err(|e| e.to_string())?,
        next_attempt_at: row.try_get("next_attempt_at").map_err(|e| e.to_string())?,
        last_error_code: row.try_get("last_error_code").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        delivered_at: row.try_get("delivered_at").map_err(|e| e.to_string())?,
    })
}

const ARTICLE_COLS: &str = r#"
    id, source_account_id, article_link_id, original_url, canonical_url,
    original_title, author, published_at, fetched_at, current_revision,
    content_md, content_hash, asset_manifest_hash, status, withdrawn_at
"#;

const LINK_COLS: &str = r#"
    id, source_account_id, canonical_url, wechat_biz, wechat_mid, wechat_idx,
    wechat_sn, title_hint, discovered_via, discovered_at
"#;

#[async_trait]
impl ArticleRepository for PgArticleRepository {
    async fn commit_captured_article(
        &self,
        cmd: CapturedArticleCommit,
    ) -> Result<CapturedArticleCommitResult, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let identity = format!(
            "{}:{}",
            cmd.article.source_account_id,
            cmd.article
                .article_link_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| cmd.article.canonical_url.clone())
        );
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(identity)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        let existing_row = sqlx::query(&format!(
            "SELECT {ARTICLE_COLS} FROM articles
             WHERE source_account_id=$1
               AND (($2::uuid IS NOT NULL AND article_link_id=$2) OR canonical_url=$3)
             FOR UPDATE"
        ))
        .bind(cmd.article.source_account_id)
        .bind(cmd.article.article_link_id)
        .bind(&cmd.article.canonical_url)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let existing = existing_row.as_ref().map(map_article).transpose()?;
        let created = existing.is_none();
        let changed = existing.as_ref().is_none_or(|a| {
            a.content_hash != cmd.article.content_hash
                || a.asset_manifest_hash != cmd.article.asset_manifest_hash
        });
        if !changed {
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(CapturedArticleCommitResult {
                article: existing.expect("unchanged capture has article"),
                revision: None,
                event: None,
                outbox: None,
                created: false,
                changed: false,
            });
        }

        let article_id = existing.as_ref().map_or(cmd.article.id, |a| a.id);
        let previous_revision = existing.as_ref().map_or(0, |a| a.current_revision);
        let revision_no = previous_revision + 1;
        if created {
            sqlx::query(r#"INSERT INTO articles (id,source_account_id,article_link_id,original_url,canonical_url,original_title,author,published_at,fetched_at,current_revision,content_md,content_hash,asset_manifest_hash,status,withdrawn_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)"#)
                .bind(article_id).bind(cmd.article.source_account_id).bind(cmd.article.article_link_id)
                .bind(&cmd.article.original_url).bind(&cmd.article.canonical_url).bind(&cmd.article.original_title)
                .bind(&cmd.article.author).bind(cmd.article.published_at).bind(cmd.article.fetched_at)
                .bind(revision_no).bind(&cmd.article.content_md).bind(&cmd.article.content_hash)
                .bind(&cmd.article.asset_manifest_hash).bind(cmd.article.status.as_str()).bind(cmd.article.withdrawn_at)
                .execute(&mut *tx).await.map_err(|e| e.to_string())?;
        } else {
            sqlx::query(r#"UPDATE articles SET article_link_id=$2,original_url=$3,original_title=$4,author=$5,published_at=$6,fetched_at=$7,current_revision=$8,content_md=$9,content_hash=$10,asset_manifest_hash=$11,status=$12,withdrawn_at=$13 WHERE id=$1"#)
                .bind(article_id).bind(cmd.article.article_link_id).bind(&cmd.article.original_url)
                .bind(&cmd.article.original_title).bind(&cmd.article.author).bind(cmd.article.published_at)
                .bind(cmd.article.fetched_at).bind(revision_no).bind(&cmd.article.content_md)
                .bind(&cmd.article.content_hash).bind(&cmd.article.asset_manifest_hash)
                .bind(cmd.article.status.as_str()).bind(cmd.article.withdrawn_at)
                .execute(&mut *tx).await.map_err(|e| e.to_string())?;
            sqlx::query("DELETE FROM article_image_decisions WHERE article_id=$1")
                .bind(article_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            sqlx::query("DELETE FROM article_assets WHERE article_id=$1")
                .bind(article_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        }
        let mut asset_refs = Vec::with_capacity(cmd.assets.len());
        for asset in &cmd.assets {
            asset_refs.push(asset.id);
            sqlx::query(r#"INSERT INTO article_assets (id,article_id,original_url,storage_key,asset_hash,width,height,byte_size,mime_type,asset_kind,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#)
                .bind(asset.id).bind(article_id).bind(&asset.original_url).bind(&asset.storage_key).bind(&asset.asset_hash)
                .bind(asset.width).bind(asset.height).bind(asset.byte_size).bind(&asset.mime_type).bind(&asset.asset_kind).bind(asset.created_at)
                .execute(&mut *tx).await.map_err(|e| e.to_string())?;
        }
        for decision in &cmd.image_decisions {
            sqlx::query(r#"INSERT INTO article_image_decisions (id,article_id,candidate_url,decision,measured_width,measured_height,reason_json,asset_id,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)"#)
                .bind(decision.id).bind(article_id).bind(&decision.candidate_url).bind(&decision.decision)
                .bind(decision.measured_width).bind(decision.measured_height).bind(&decision.reason_json).bind(decision.asset_id).bind(decision.created_at)
                .execute(&mut *tx).await.map_err(|e| e.to_string())?;
        }
        let revision = ArticleRevision {
            id: Uuid::new_v4(),
            article_id,
            revision: revision_no,
            editor_kind: EditorKind::Crawler,
            content_md: cmd.article.content_md.clone(),
            content_hash: cmd.article.content_hash.clone(),
            change_reason: cmd.change_reason,
            created_at: cmd.now,
        };
        sqlx::query(r#"INSERT INTO article_revisions (id,article_id,revision,editor_kind,content_md,content_hash,change_reason,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)"#)
            .bind(revision.id).bind(article_id).bind(revision_no).bind(revision.editor_kind.as_str()).bind(&revision.content_md).bind(&revision.content_hash).bind(&revision.change_reason).bind(cmd.now)
            .execute(&mut *tx).await.map_err(|e| e.to_string())?;
        let change_type = if created {
            ChangeType::ArticleCreated
        } else {
            ChangeType::ArticleRefetched
        };
        let event_id = Uuid::new_v4();
        let payload = serde_json::json!({"article_id": article_id, "article_revision": revision_no, "change_type": change_type.as_str(), "schema_version": CHANGE_SCHEMA_VERSION, "asset_refs": asset_refs, "editor_kind": EditorKind::Crawler.as_str()});
        let event_row = sqlx::query(r#"INSERT INTO article_change_events (id,event_id,article_id,article_revision,change_type,schema_version,payload_json,created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id,event_id,change_sequence,article_id,article_revision,change_type,schema_version,payload_json,created_at"#)
            .bind(Uuid::new_v4()).bind(event_id).bind(article_id).bind(revision_no).bind(change_type.as_str()).bind(CHANGE_SCHEMA_VERSION).bind(&payload).bind(cmd.now)
            .fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
        let event = map_change_event(&event_row)?;
        let outbox_row = sqlx::query(r#"INSERT INTO delivery_outbox (id,event_id,change_sequence,event_type,article_id,article_revision,payload_schema_version,payload_json,status,attempt_count,next_attempt_at,last_error_code,created_at,delivered_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,'pending',0,$9,NULL,$10,NULL) RETURNING id,event_id,change_sequence,event_type,article_id,article_revision,payload_schema_version,payload_json,status,attempt_count,next_attempt_at,last_error_code,created_at,delivered_at"#)
            .bind(Uuid::new_v4()).bind(event_id).bind(event.change_sequence).bind(change_type.as_str()).bind(article_id).bind(revision_no).bind(CHANGE_SCHEMA_VERSION).bind(&payload).bind(cmd.now).bind(cmd.now)
            .fetch_one(&mut *tx).await.map_err(|e| e.to_string())?;
        let outbox = map_outbox(&outbox_row)?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let mut article = cmd.article;
        article.id = article_id;
        article.current_revision = revision_no;
        Ok(CapturedArticleCommitResult {
            article,
            revision: Some(revision),
            event: Some(event),
            outbox: Some(outbox),
            created,
            changed: true,
        })
    }
    async fn ensure_source_stub(
        &self,
        source_account_id: Uuid,
        display_name: &str,
    ) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO source_accounts (id, display_name, status)
            VALUES ($1, $2, 'active')
            ON CONFLICT (id) DO NOTHING
            "#,
        )
        .bind(source_account_id)
        .bind(display_name)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn upsert_article_link(
        &self,
        cmd: UpsertArticleLink,
    ) -> Result<UpsertArticleLinkResult, String> {
        let exists: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM source_accounts WHERE id = $1)"#)
                .bind(cmd.source_account_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        if !exists {
            return Err("source.not_found: 监测公众号不存在".into());
        }

        if let (Some(biz), Some(mid), Some(idx)) =
            (&cmd.wechat_biz, &cmd.wechat_mid, &cmd.wechat_idx)
        {
            let row = sqlx::query(&format!(
                "SELECT {LINK_COLS} FROM article_links
                 WHERE source_account_id = $1
                   AND wechat_biz = $2 AND wechat_mid = $3 AND wechat_idx = $4"
            ))
            .bind(cmd.source_account_id)
            .bind(biz)
            .bind(mid)
            .bind(idx)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            if let Some(row) = row {
                let id: Uuid = row.try_get("id").map_err(|e| e.to_string())?;
                let updated = sqlx::query(&format!(
                    "UPDATE article_links SET
                        discovered_at = $2,
                        title_hint = COALESCE($3, title_hint)
                     WHERE id = $1
                     RETURNING {LINK_COLS}"
                ))
                .bind(id)
                .bind(cmd.discovered_at)
                .bind(&cmd.title_hint)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
                return Ok(UpsertArticleLinkResult {
                    link: map_link(&updated)?,
                    created: false,
                });
            }
        }

        let by_url = sqlx::query(&format!(
            "SELECT {LINK_COLS} FROM article_links
             WHERE source_account_id = $1 AND canonical_url = $2"
        ))
        .bind(cmd.source_account_id)
        .bind(&cmd.canonical_url)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if let Some(row) = by_url {
            let id: Uuid = row.try_get("id").map_err(|e| e.to_string())?;
            let updated = sqlx::query(&format!(
                "UPDATE article_links SET
                    discovered_at = $2,
                    title_hint = COALESCE($3, title_hint)
                 WHERE id = $1
                 RETURNING {LINK_COLS}"
            ))
            .bind(id)
            .bind(cmd.discovered_at)
            .bind(&cmd.title_hint)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            return Ok(UpsertArticleLinkResult {
                link: map_link(&updated)?,
                created: false,
            });
        }

        let id = Uuid::new_v4();
        let row = sqlx::query(&format!(
            "INSERT INTO article_links (
                id, source_account_id, canonical_url, wechat_biz, wechat_mid, wechat_idx,
                wechat_sn, title_hint, discovered_via, discovered_at
             ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
             RETURNING {LINK_COLS}"
        ))
        .bind(id)
        .bind(cmd.source_account_id)
        .bind(&cmd.canonical_url)
        .bind(&cmd.wechat_biz)
        .bind(&cmd.wechat_mid)
        .bind(&cmd.wechat_idx)
        .bind(&cmd.wechat_sn)
        .bind(&cmd.title_hint)
        .bind(&cmd.discovered_via)
        .bind(cmd.discovered_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(UpsertArticleLinkResult {
            link: map_link(&row)?,
            created: true,
        })
    }

    async fn find_article_by_identity(
        &self,
        source_account_id: Uuid,
        wechat_biz: Option<&str>,
        wechat_mid: Option<&str>,
        wechat_idx: Option<&str>,
        canonical_url: &str,
    ) -> Result<Option<Article>, String> {
        if let (Some(biz), Some(mid), Some(idx)) = (wechat_biz, wechat_mid, wechat_idx) {
            let row = sqlx::query(&format!(
                "SELECT {ARTICLE_COLS}
                 FROM articles a
                 WHERE a.article_link_id = (
                     SELECT id FROM article_links
                     WHERE source_account_id = $1
                       AND wechat_biz = $2 AND wechat_mid = $3 AND wechat_idx = $4
                     LIMIT 1
                 )"
            ))
            .bind(source_account_id)
            .bind(biz)
            .bind(mid)
            .bind(idx)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            if let Some(row) = row {
                return Ok(Some(map_article(&row)?));
            }
        }

        let row = sqlx::query(&format!(
            "SELECT {ARTICLE_COLS} FROM articles
             WHERE source_account_id = $1 AND canonical_url = $2"
        ))
        .bind(source_account_id)
        .bind(canonical_url)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_article(&r)).transpose()
    }

    async fn get_article(&self, article_id: Uuid) -> Result<Option<Article>, String> {
        let row = sqlx::query(&format!(
            "SELECT {ARTICLE_COLS} FROM articles WHERE id = $1"
        ))
        .bind(article_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_article(&r)).transpose()
    }

    async fn insert_article(&self, article: Article) -> Result<Article, String> {
        let result = sqlx::query(
            r#"
            INSERT INTO articles (
                id, source_account_id, article_link_id, original_url, canonical_url,
                original_title, author, published_at, fetched_at, current_revision,
                content_md, content_hash, asset_manifest_hash, status, withdrawn_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15
            )
            "#,
        )
        .bind(article.id)
        .bind(article.source_account_id)
        .bind(article.article_link_id)
        .bind(&article.original_url)
        .bind(&article.canonical_url)
        .bind(&article.original_title)
        .bind(&article.author)
        .bind(article.published_at)
        .bind(article.fetched_at)
        .bind(article.current_revision)
        .bind(&article.content_md)
        .bind(&article.content_hash)
        .bind(&article.asset_manifest_hash)
        .bind(article.status.as_str())
        .bind(article.withdrawn_at)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(article),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") || msg.contains("23505") {
                    Err("article.duplicate: 文章已存在，拒绝双写".into())
                } else {
                    Err(msg)
                }
            }
        }
    }

    async fn insert_asset(&self, asset: ArticleAsset) -> Result<ArticleAsset, String> {
        if let Some(ref hash) = asset.asset_hash {
            let existing = sqlx::query(
                r#"
                SELECT id, article_id, original_url, storage_key, asset_hash,
                       width, height, byte_size, mime_type, asset_kind, created_at
                FROM article_assets
                WHERE article_id = $1 AND asset_hash = $2
                "#,
            )
            .bind(asset.article_id)
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            if let Some(row) = existing {
                return map_asset(&row);
            }
        }

        sqlx::query(
            r#"
            INSERT INTO article_assets (
                id, article_id, original_url, storage_key, asset_hash,
                width, height, byte_size, mime_type, asset_kind, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(asset.id)
        .bind(asset.article_id)
        .bind(&asset.original_url)
        .bind(&asset.storage_key)
        .bind(&asset.asset_hash)
        .bind(asset.width)
        .bind(asset.height)
        .bind(asset.byte_size)
        .bind(&asset.mime_type)
        .bind(&asset.asset_kind)
        .bind(asset.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(asset)
    }

    async fn insert_image_decision(
        &self,
        decision: ArticleImageDecision,
    ) -> Result<ArticleImageDecision, String> {
        sqlx::query(
            r#"
            INSERT INTO article_image_decisions (
                id, article_id, candidate_url, decision, measured_width, measured_height,
                reason_json, asset_id, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            "#,
        )
        .bind(decision.id)
        .bind(decision.article_id)
        .bind(&decision.candidate_url)
        .bind(&decision.decision)
        .bind(decision.measured_width)
        .bind(decision.measured_height)
        .bind(&decision.reason_json)
        .bind(decision.asset_id)
        .bind(decision.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(decision)
    }

    async fn list_decisions(&self, article_id: Uuid) -> Result<Vec<ArticleImageDecision>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, article_id, candidate_url, decision, measured_width, measured_height,
                   reason_json, asset_id, created_at
            FROM article_image_decisions
            WHERE article_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_decision).collect()
    }

    async fn list_assets(&self, article_id: Uuid) -> Result<Vec<ArticleAsset>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, article_id, original_url, storage_key, asset_hash,
                   width, height, byte_size, mime_type, asset_kind, created_at
            FROM article_assets
            WHERE article_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_asset).collect()
    }

    async fn count_articles_for_source(&self, source_account_id: Uuid) -> Result<u64, String> {
        let n: i64 = sqlx::query_scalar(
            r#"SELECT COUNT(*)::bigint FROM articles WHERE source_account_id = $1"#,
        )
        .bind(source_account_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(n as u64)
    }

    async fn commit_revision(
        &self,
        cmd: CommitRevisionCommand,
    ) -> Result<CommitRevisionResult, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let row = sqlx::query(&format!(
            "SELECT {ARTICLE_COLS} FROM articles WHERE id = $1 FOR UPDATE"
        ))
        .bind(cmd.article_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "article.not_found: 文章不存在".to_string())?;

        let mut article = map_article(&row)?;

        let hash_changed = article.content_hash != cmd.content_hash
            || article.asset_manifest_hash != cmd.asset_manifest_hash;
        let create_rev = cmd.force_new_revision || hash_changed;

        if !create_rev {
            if article.status != cmd.status {
                let next = article.status.transition(cmd.status)?;
                sqlx::query(r#"UPDATE articles SET status = $2 WHERE id = $1"#)
                    .bind(cmd.article_id)
                    .bind(next.as_str())
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| e.to_string())?;
                article.status = next;
            }
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(CommitRevisionResult {
                article,
                revision: None,
                event: None,
                outbox: None,
                created_new_revision: false,
            });
        }

        let has_any: bool = sqlx::query_scalar(
            r#"SELECT EXISTS(SELECT 1 FROM article_revisions WHERE article_id = $1)"#,
        )
        .bind(cmd.article_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let revision_no = if has_any {
            article.current_revision + 1
        } else {
            1
        };

        let rev = ArticleRevision {
            id: Uuid::new_v4(),
            article_id: cmd.article_id,
            revision: revision_no,
            editor_kind: cmd.editor_kind,
            content_md: cmd.content_md.clone(),
            content_hash: cmd.content_hash.clone(),
            change_reason: cmd.change_reason.clone(),
            created_at: cmd.now,
        };

        sqlx::query(
            r#"
            INSERT INTO article_revisions (
                id, article_id, revision, editor_kind, content_md, content_hash,
                change_reason, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(rev.id)
        .bind(rev.article_id)
        .bind(rev.revision)
        .bind(rev.editor_kind.as_str())
        .bind(&rev.content_md)
        .bind(&rev.content_hash)
        .bind(&rev.change_reason)
        .bind(rev.created_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        sqlx::query(
            r#"
            UPDATE articles SET
                current_revision = $2,
                content_md = $3,
                content_hash = $4,
                asset_manifest_hash = $5,
                status = $6
            WHERE id = $1
            "#,
        )
        .bind(cmd.article_id)
        .bind(revision_no)
        .bind(&cmd.content_md)
        .bind(&cmd.content_hash)
        .bind(&cmd.asset_manifest_hash)
        .bind(cmd.status.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        article.current_revision = revision_no;
        article.content_md = cmd.content_md.clone();
        article.content_hash = cmd.content_hash.clone();
        article.asset_manifest_hash = cmd.asset_manifest_hash.clone();
        article.status = cmd.status;

        let event_id = Uuid::new_v4();
        let event_row_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "article_id": cmd.article_id,
            "article_revision": revision_no,
            "change_type": cmd.change_type.as_str(),
            "schema_version": CHANGE_SCHEMA_VERSION,
            "asset_refs": cmd.asset_refs,
            "editor_kind": cmd.editor_kind.as_str(),
        });

        let event_row = sqlx::query(
            r#"
            INSERT INTO article_change_events (
                id, event_id, article_id, article_revision, change_type,
                schema_version, payload_json, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            RETURNING id, event_id, change_sequence, article_id, article_revision,
                      change_type, schema_version, payload_json, created_at
            "#,
        )
        .bind(event_row_id)
        .bind(event_id)
        .bind(cmd.article_id)
        .bind(revision_no)
        .bind(cmd.change_type.as_str())
        .bind(CHANGE_SCHEMA_VERSION)
        .bind(&payload)
        .bind(cmd.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let event = map_change_event(&event_row)?;
        let seq = event.change_sequence;

        let outbox_id = Uuid::new_v4();
        let outbox_row = sqlx::query(
            r#"
            INSERT INTO delivery_outbox (
                id, event_id, change_sequence, event_type, article_id, article_revision,
                payload_schema_version, payload_json, status, attempt_count,
                next_attempt_at, last_error_code, created_at, delivered_at
            ) VALUES (
                $1,$2,$3,$4,$5,$6,$7,$8,'pending',0,$9,NULL,$10,NULL
            )
            RETURNING id, event_id, change_sequence, event_type, article_id, article_revision,
                      payload_schema_version, payload_json, status, attempt_count,
                      next_attempt_at, last_error_code, created_at, delivered_at
            "#,
        )
        .bind(outbox_id)
        .bind(event_id)
        .bind(seq)
        .bind(cmd.change_type.as_str())
        .bind(cmd.article_id)
        .bind(revision_no)
        .bind(CHANGE_SCHEMA_VERSION)
        .bind(&payload)
        .bind(cmd.now)
        .bind(cmd.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let outbox = map_outbox(&outbox_row)?;

        tx.commit().await.map_err(|e| e.to_string())?;

        Ok(CommitRevisionResult {
            article,
            revision: Some(rev),
            event: Some(event),
            outbox: Some(outbox),
            created_new_revision: true,
        })
    }

    async fn list_revisions(&self, article_id: Uuid) -> Result<Vec<ArticleRevision>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, article_id, revision, editor_kind, content_md, content_hash,
                   change_reason, created_at
            FROM article_revisions
            WHERE article_id = $1
            ORDER BY revision ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_revision).collect()
    }

    async fn get_revision(
        &self,
        article_id: Uuid,
        revision: i32,
    ) -> Result<Option<ArticleRevision>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, article_id, revision, editor_kind, content_md, content_hash,
                   change_reason, created_at
            FROM article_revisions
            WHERE article_id = $1 AND revision = $2
            "#,
        )
        .bind(article_id)
        .bind(revision)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_revision(&r)).transpose()
    }

    async fn upsert_ocr_block(&self, block: ArticleOcrBlock) -> Result<ArticleOcrBlock, String> {
        let row = sqlx::query(
            r#"
            INSERT INTO article_ocr_blocks (
                id, article_id, asset_id, block_anchor, recognized_text, confidence,
                status, provider_ref, provider, model, provider_version, ocr_version,
                provider_call_id, error_code, attempt, created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
            ON CONFLICT (article_id, asset_id, block_anchor) DO UPDATE SET
                recognized_text = EXCLUDED.recognized_text,
                confidence = EXCLUDED.confidence,
                status = EXCLUDED.status,
                provider_ref = EXCLUDED.provider_ref,
                provider = EXCLUDED.provider,
                model = EXCLUDED.model,
                provider_version = EXCLUDED.provider_version,
                ocr_version = EXCLUDED.ocr_version,
                provider_call_id = EXCLUDED.provider_call_id,
                error_code = EXCLUDED.error_code,
                attempt = EXCLUDED.attempt,
                updated_at = EXCLUDED.updated_at
            RETURNING id, article_id, asset_id, block_anchor, recognized_text,
                      confidence::float8 AS confidence, status, provider_ref, provider,
                      model, provider_version, ocr_version, provider_call_id, error_code, attempt,
                      created_at, updated_at
            "#,
        )
        .bind(block.id)
        .bind(block.article_id)
        .bind(block.asset_id)
        .bind(&block.block_anchor)
        .bind(&block.recognized_text)
        .bind(block.confidence)
        .bind(block.status.as_str())
        .bind(&block.provider_ref)
        .bind(&block.provider)
        .bind(&block.model)
        .bind(&block.provider_version)
        .bind(&block.ocr_version)
        .bind(&block.provider_call_id)
        .bind(&block.error_code)
        .bind(block.attempt)
        .bind(block.created_at)
        .bind(block.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        map_ocr_block(&row)
    }

    async fn list_ocr_blocks(&self, article_id: Uuid) -> Result<Vec<ArticleOcrBlock>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, article_id, asset_id, block_anchor, recognized_text,
                   confidence::float8 AS confidence, status, provider_ref, provider,
                   model, provider_version, ocr_version, provider_call_id, error_code, attempt,
                   created_at, updated_at
            FROM article_ocr_blocks
            WHERE article_id = $1
            ORDER BY created_at ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_ocr_block).collect()
    }

    async fn list_change_events(
        &self,
        article_id: Uuid,
    ) -> Result<Vec<ArticleChangeEvent>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, event_id, change_sequence, article_id, article_revision,
                   change_type, schema_version, payload_json, created_at
            FROM article_change_events
            WHERE article_id = $1
            ORDER BY change_sequence ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_change_event).collect()
    }

    async fn list_outbox_for_article(
        &self,
        article_id: Uuid,
    ) -> Result<Vec<DeliveryOutbox>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, event_id, change_sequence, event_type, article_id, article_revision,
                   payload_schema_version, payload_json, status, attempt_count,
                   next_attempt_at, last_error_code, created_at, delivered_at
            FROM delivery_outbox
            WHERE article_id = $1
            ORDER BY change_sequence ASC
            "#,
        )
        .bind(article_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_outbox).collect()
    }

    async fn list_articles_for_sources(
        &self,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<Article>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = sqlx::query(&format!(
            "SELECT {ARTICLE_COLS} FROM articles
             WHERE source_account_id = ANY($1)
             ORDER BY fetched_at DESC, id DESC
             LIMIT $2"
        ))
        .bind(source_ids)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_article).collect()
    }

    async fn list_change_events_since(
        &self,
        after_sequence: i64,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<ArticleChangeEvent>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = sqlx::query(
            r#"
            SELECT e.id, e.event_id, e.change_sequence, e.article_id, e.article_revision,
                   e.change_type, e.schema_version, e.payload_json, e.created_at
            FROM article_change_events e
            INNER JOIN articles a ON a.id = e.article_id
            WHERE e.change_sequence > $1
              AND a.source_account_id = ANY($2)
            ORDER BY e.change_sequence ASC
            LIMIT $3
            "#,
        )
        .bind(after_sequence)
        .bind(source_ids)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_change_event).collect()
    }
}
