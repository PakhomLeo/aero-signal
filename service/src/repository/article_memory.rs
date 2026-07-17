//! 内存文章仓储（测试与无 PG 场景；revision/event/outbox 同锁原子语义）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::domain::article::{
    Article, ArticleAsset, ArticleChangeEvent, ArticleImageDecision, ArticleLink, ArticleOcrBlock,
    ArticleRevision, CHANGE_SCHEMA_VERSION, DeliveryOutbox, OutboxStatus,
};
use crate::repository::article::{
    ArticleRepository, CapturedArticleCommit, CapturedArticleCommitResult, CommitRevisionCommand,
    CommitRevisionResult, UpsertArticleLink, UpsertArticleLinkResult,
};

#[derive(Default, Clone)]
struct State {
    sources: HashMap<Uuid, String>,
    links: HashMap<Uuid, ArticleLink>,
    by_wechat: HashMap<(Uuid, String, String, String), Uuid>,
    by_url: HashMap<(Uuid, String), Uuid>,
    articles: HashMap<Uuid, Article>,
    assets: HashMap<Uuid, ArticleAsset>,
    decisions: HashMap<Uuid, ArticleImageDecision>,
    revisions: HashMap<Uuid, ArticleRevision>,
    ocr_blocks: HashMap<Uuid, ArticleOcrBlock>,
    /// (article_id, asset_id, block_anchor) -> block id
    ocr_keys: HashMap<(Uuid, Uuid, String), Uuid>,
    change_events: HashMap<Uuid, ArticleChangeEvent>,
    outbox: HashMap<Uuid, DeliveryOutbox>,
    next_sequence: i64,
}

#[derive(Default, Clone)]
pub struct MemoryArticleStore {
    state: Arc<RwLock<State>>,
}

impl MemoryArticleStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn commit_locked(
        s: &mut State,
        cmd: CommitRevisionCommand,
        write_outbox: bool,
    ) -> Result<CommitRevisionResult, String> {
        let article = s
            .articles
            .get_mut(&cmd.article_id)
            .ok_or_else(|| "article.not_found: 文章不存在".to_string())?;

        let hash_changed = article.content_hash != cmd.content_hash
            || article.asset_manifest_hash != cmd.asset_manifest_hash;
        let create_rev = cmd.force_new_revision || hash_changed;

        if !create_rev {
            // 仅允许状态推进（如 OCR 警告），不发下游事件。
            if article.status != cmd.status {
                article.status = cmd.status;
            }
            return Ok(CommitRevisionResult {
                article: article.clone(),
                revision: None,
                event: None,
                outbox: None,
                created_new_revision: false,
            });
        }

        let has_any = s.revisions.values().any(|r| r.article_id == cmd.article_id);
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

        article.current_revision = revision_no;
        article.content_md = cmd.content_md;
        article.content_hash = cmd.content_hash;
        article.asset_manifest_hash = cmd.asset_manifest_hash;
        article.status = cmd.status;

        s.revisions.insert(rev.id, rev.clone());

        s.next_sequence += 1;
        let seq = s.next_sequence;
        let event_id = Uuid::new_v4();
        let payload = serde_json::json!({
            "article_id": cmd.article_id,
            "article_revision": revision_no,
            "change_type": cmd.change_type.as_str(),
            "schema_version": CHANGE_SCHEMA_VERSION,
            "asset_refs": cmd.asset_refs,
            "editor_kind": cmd.editor_kind.as_str(),
        });

        let event = ArticleChangeEvent {
            id: Uuid::new_v4(),
            event_id,
            change_sequence: seq,
            article_id: cmd.article_id,
            article_revision: revision_no,
            change_type: cmd.change_type,
            schema_version: CHANGE_SCHEMA_VERSION,
            payload_json: payload.clone(),
            created_at: cmd.now,
        };
        s.change_events.insert(event.id, event.clone());

        let outbox = if write_outbox {
            let row = DeliveryOutbox {
                id: Uuid::new_v4(),
                event_id,
                change_sequence: seq,
                event_type: cmd.change_type,
                article_id: cmd.article_id,
                article_revision: revision_no,
                payload_schema_version: CHANGE_SCHEMA_VERSION,
                payload_json: payload,
                status: OutboxStatus::Pending,
                attempt_count: 0,
                next_attempt_at: Some(cmd.now),
                last_error_code: None,
                created_at: cmd.now,
                delivered_at: None,
            };
            s.outbox.insert(row.id, row.clone());
            Some(row)
        } else {
            None
        };

        if !write_outbox {
            return Err("outbox.missing: 变更未写入 delivery_outbox".into());
        }

        Ok(CommitRevisionResult {
            article: article.clone(),
            revision: Some(rev),
            event: Some(event),
            outbox,
            created_new_revision: true,
        })
    }
}

#[async_trait]
impl ArticleRepository for MemoryArticleStore {
    async fn commit_captured_article(
        &self,
        cmd: CapturedArticleCommit,
    ) -> Result<CapturedArticleCommitResult, String> {
        let mut guard = self.state.write().await;
        let mut staged = guard.clone();
        if !staged.sources.contains_key(&cmd.article.source_account_id) {
            return Err("source.not_found: monitored source does not exist".into());
        }

        let existing_id = staged
            .articles
            .values()
            .find(|article| {
                article.source_account_id == cmd.article.source_account_id
                    && (cmd.article.article_link_id.is_some()
                        && article.article_link_id == cmd.article.article_link_id
                        || article.canonical_url == cmd.article.canonical_url)
            })
            .map(|article| article.id);
        let created = existing_id.is_none();
        let article_id = existing_id.unwrap_or(cmd.article.id);
        let existing = existing_id.and_then(|id| staged.articles.get(&id).cloned());
        let changed = existing.as_ref().is_none_or(|article| {
            article.content_hash != cmd.article.content_hash
                || article.asset_manifest_hash != cmd.article.asset_manifest_hash
        });

        if !changed {
            return Ok(CapturedArticleCommitResult {
                article: existing.expect("unchanged capture has existing article"),
                revision: None,
                event: None,
                outbox: None,
                created: false,
                changed: false,
            });
        }

        let mut article = cmd.article.clone();
        article.id = article_id;
        article.current_revision = existing.as_ref().map_or(0, |a| a.current_revision);
        staged.articles.insert(article_id, article.clone());

        staged.assets.retain(|_, row| row.article_id != article_id);
        staged
            .decisions
            .retain(|_, row| row.article_id != article_id);
        let mut asset_refs = Vec::with_capacity(cmd.assets.len());
        for mut asset in cmd.assets {
            asset.article_id = article_id;
            if staged.assets.contains_key(&asset.id) {
                return Err("article_capture.duplicate_asset_id".into());
            }
            asset_refs.push(asset.id);
            staged.assets.insert(asset.id, asset);
        }
        for mut decision in cmd.image_decisions {
            decision.article_id = article_id;
            if staged.decisions.contains_key(&decision.id) {
                return Err("article_capture.duplicate_decision_id".into());
            }
            if let Some(asset_id) = decision.asset_id
                && !staged.assets.contains_key(&asset_id)
            {
                return Err("article_capture.decision_asset_missing".into());
            }
            staged.decisions.insert(decision.id, decision);
        }

        let revision_result = Self::commit_locked(
            &mut staged,
            CommitRevisionCommand {
                article_id,
                editor_kind: crate::domain::article::EditorKind::Crawler,
                content_md: article.content_md,
                content_hash: article.content_hash,
                asset_manifest_hash: article.asset_manifest_hash,
                change_reason: cmd.change_reason,
                change_type: if created {
                    crate::domain::article::ChangeType::ArticleCreated
                } else {
                    crate::domain::article::ChangeType::ArticleRefetched
                },
                status: article.status,
                asset_refs,
                now: cmd.now,
                force_new_revision: true,
            },
            true,
        )?;
        *guard = staged;
        Ok(CapturedArticleCommitResult {
            article: revision_result.article,
            revision: revision_result.revision,
            event: revision_result.event,
            outbox: revision_result.outbox,
            created,
            changed: true,
        })
    }
    async fn ensure_source_stub(
        &self,
        source_account_id: Uuid,
        display_name: &str,
    ) -> Result<(), String> {
        let mut s = self.state.write().await;
        s.sources
            .entry(source_account_id)
            .or_insert_with(|| display_name.to_string());
        Ok(())
    }

    async fn upsert_article_link(
        &self,
        cmd: UpsertArticleLink,
    ) -> Result<UpsertArticleLinkResult, String> {
        let mut s = self.state.write().await;
        if !s.sources.contains_key(&cmd.source_account_id) {
            return Err("source.not_found: 监测公众号不存在".into());
        }

        if let (Some(biz), Some(mid), Some(idx)) =
            (&cmd.wechat_biz, &cmd.wechat_mid, &cmd.wechat_idx)
        {
            let key = (cmd.source_account_id, biz.clone(), mid.clone(), idx.clone());
            if let Some(id) = s.by_wechat.get(&key).copied()
                && let Some(link) = s.links.get_mut(&id)
            {
                link.discovered_at = cmd.discovered_at;
                if cmd.title_hint.is_some() {
                    link.title_hint = cmd.title_hint.clone();
                }
                return Ok(UpsertArticleLinkResult {
                    link: link.clone(),
                    created: false,
                });
            }
        }

        let url_key = (cmd.source_account_id, cmd.canonical_url.clone());
        if let Some(id) = s.by_url.get(&url_key).copied()
            && let Some(link) = s.links.get_mut(&id)
        {
            link.discovered_at = cmd.discovered_at;
            if cmd.title_hint.is_some() {
                link.title_hint = cmd.title_hint.clone();
            }
            return Ok(UpsertArticleLinkResult {
                link: link.clone(),
                created: false,
            });
        }

        let id = Uuid::new_v4();
        let link = ArticleLink {
            id,
            source_account_id: cmd.source_account_id,
            canonical_url: cmd.canonical_url.clone(),
            wechat_biz: cmd.wechat_biz.clone(),
            wechat_mid: cmd.wechat_mid.clone(),
            wechat_idx: cmd.wechat_idx.clone(),
            wechat_sn: cmd.wechat_sn,
            title_hint: cmd.title_hint,
            discovered_via: cmd.discovered_via,
            discovered_at: cmd.discovered_at,
        };
        if let (Some(biz), Some(mid), Some(idx)) =
            (&link.wechat_biz, &link.wechat_mid, &link.wechat_idx)
        {
            s.by_wechat.insert(
                (
                    link.source_account_id,
                    biz.clone(),
                    mid.clone(),
                    idx.clone(),
                ),
                id,
            );
        }
        s.by_url.insert(url_key, id);
        s.links.insert(id, link.clone());
        Ok(UpsertArticleLinkResult {
            link,
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
        let s = self.state.read().await;
        if let (Some(biz), Some(mid), Some(idx)) = (wechat_biz, wechat_mid, wechat_idx)
            && let Some(link_id) =
                s.by_wechat
                    .get(&(source_account_id, biz.into(), mid.into(), idx.into()))
            && let Some(article) = s
                .articles
                .values()
                .find(|a| a.article_link_id == Some(*link_id))
        {
            return Ok(Some(article.clone()));
        }
        Ok(s.articles
            .values()
            .find(|a| a.source_account_id == source_account_id && a.canonical_url == canonical_url)
            .cloned())
    }

    async fn get_article(&self, article_id: Uuid) -> Result<Option<Article>, String> {
        let s = self.state.read().await;
        Ok(s.articles.get(&article_id).cloned())
    }

    async fn insert_article(&self, article: Article) -> Result<Article, String> {
        let mut s = self.state.write().await;
        if s.articles.values().any(|a| {
            a.source_account_id == article.source_account_id
                && a.canonical_url == article.canonical_url
        }) {
            return Err("article.duplicate: 文章已存在，拒绝双写".into());
        }
        s.articles.insert(article.id, article.clone());
        Ok(article)
    }

    async fn insert_asset(&self, asset: ArticleAsset) -> Result<ArticleAsset, String> {
        let mut s = self.state.write().await;
        if let Some(ref hash) = asset.asset_hash
            && let Some(existing) = s.assets.values().find(|a| {
                a.article_id == asset.article_id && a.asset_hash.as_deref() == Some(hash.as_str())
            })
        {
            return Ok(existing.clone());
        }
        s.assets.insert(asset.id, asset.clone());
        Ok(asset)
    }

    async fn insert_image_decision(
        &self,
        decision: ArticleImageDecision,
    ) -> Result<ArticleImageDecision, String> {
        let mut s = self.state.write().await;
        s.decisions.insert(decision.id, decision.clone());
        Ok(decision)
    }

    async fn list_decisions(&self, article_id: Uuid) -> Result<Vec<ArticleImageDecision>, String> {
        let s = self.state.read().await;
        Ok(s.decisions
            .values()
            .filter(|d| d.article_id == article_id)
            .cloned()
            .collect())
    }

    async fn list_assets(&self, article_id: Uuid) -> Result<Vec<ArticleAsset>, String> {
        let s = self.state.read().await;
        Ok(s.assets
            .values()
            .filter(|a| a.article_id == article_id)
            .cloned()
            .collect())
    }

    async fn count_articles_for_source(&self, source_account_id: Uuid) -> Result<u64, String> {
        let s = self.state.read().await;
        Ok(s.articles
            .values()
            .filter(|a| a.source_account_id == source_account_id)
            .count() as u64)
    }

    async fn commit_revision(
        &self,
        cmd: CommitRevisionCommand,
    ) -> Result<CommitRevisionResult, String> {
        let mut s = self.state.write().await;
        Self::commit_locked(&mut s, cmd, true)
    }

    async fn list_revisions(&self, article_id: Uuid) -> Result<Vec<ArticleRevision>, String> {
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .revisions
            .values()
            .filter(|r| r.article_id == article_id)
            .cloned()
            .collect();
        rows.sort_by_key(|r| r.revision);
        Ok(rows)
    }

    async fn get_revision(
        &self,
        article_id: Uuid,
        revision: i32,
    ) -> Result<Option<ArticleRevision>, String> {
        let s = self.state.read().await;
        Ok(s.revisions
            .values()
            .find(|r| r.article_id == article_id && r.revision == revision)
            .cloned())
    }

    async fn upsert_ocr_block(&self, block: ArticleOcrBlock) -> Result<ArticleOcrBlock, String> {
        let mut s = self.state.write().await;
        let key = (block.article_id, block.asset_id, block.block_anchor.clone());
        if let Some(existing_id) = s.ocr_keys.get(&key).copied()
            && let Some(existing) = s.ocr_blocks.get_mut(&existing_id)
        {
            existing.recognized_text = block.recognized_text;
            existing.confidence = block.confidence;
            existing.status = block.status;
            existing.provider_ref = block.provider_ref;
            existing.provider = block.provider;
            existing.model = block.model;
            existing.provider_version = block.provider_version;
            existing.ocr_version = block.ocr_version;
            existing.provider_call_id = block.provider_call_id;
            existing.error_code = block.error_code;
            existing.attempt = block.attempt;
            existing.updated_at = block.updated_at;
            return Ok(existing.clone());
        }
        s.ocr_keys.insert(key, block.id);
        s.ocr_blocks.insert(block.id, block.clone());
        Ok(block)
    }

    async fn list_ocr_blocks(&self, article_id: Uuid) -> Result<Vec<ArticleOcrBlock>, String> {
        let s = self.state.read().await;
        Ok(s.ocr_blocks
            .values()
            .filter(|b| b.article_id == article_id)
            .cloned()
            .collect())
    }

    async fn list_change_events(
        &self,
        article_id: Uuid,
    ) -> Result<Vec<ArticleChangeEvent>, String> {
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .change_events
            .values()
            .filter(|e| e.article_id == article_id)
            .cloned()
            .collect();
        rows.sort_by_key(|e| e.change_sequence);
        Ok(rows)
    }

    async fn list_outbox_for_article(
        &self,
        article_id: Uuid,
    ) -> Result<Vec<DeliveryOutbox>, String> {
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .outbox
            .values()
            .filter(|o| o.article_id == article_id)
            .cloned()
            .collect();
        rows.sort_by_key(|o| o.change_sequence);
        Ok(rows)
    }

    async fn list_articles_for_sources(
        &self,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<Article>, String> {
        let allowed: std::collections::HashSet<_> = source_ids.iter().copied().collect();
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .articles
            .values()
            .filter(|a| allowed.contains(&a.source_account_id))
            .cloned()
            .collect();
        rows.sort_by(|a, b| b.fetched_at.cmp(&a.fetched_at).then(b.id.cmp(&a.id)));
        rows.truncate(limit);
        Ok(rows)
    }

    async fn list_change_events_since(
        &self,
        after_sequence: i64,
        source_ids: &[Uuid],
        limit: usize,
    ) -> Result<Vec<ArticleChangeEvent>, String> {
        let allowed: std::collections::HashSet<_> = source_ids.iter().copied().collect();
        let s = self.state.read().await;
        let mut rows: Vec<_> = s
            .change_events
            .values()
            .filter(|e| e.change_sequence > after_sequence)
            .filter(|e| {
                s.articles
                    .get(&e.article_id)
                    .map(|a| allowed.contains(&a.source_account_id))
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        rows.sort_by_key(|e| e.change_sequence);
        rows.truncate(limit);
        Ok(rows)
    }

    async fn commit_revision_without_outbox_for_test(
        &self,
        cmd: CommitRevisionCommand,
    ) -> Result<(), String> {
        let s = self.state.read().await;
        let mut staged = s.clone();
        match Self::commit_locked(&mut staged, cmd, false) {
            Ok(_) => Err("outbox.guard_failed: 漏写 outbox 未被拒绝".into()),
            Err(e) if e.starts_with("outbox.missing") => Ok(()),
            Err(e) => Err(e),
        }
    }
}
