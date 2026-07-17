//! M10 content-normalizer 集成测试。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crawler_capture::{FakeObjectStore, ObjectStore};
use job_pipeline::{JobContext, JobHandler, JobId, MemoryRepo, ProgressSender};
use provider_hub::{
    FakeOcrProvider, OcrProvider, OcrRequest, OcrResponse, ProviderError, ProviderResult,
};
use server_core::Clock;
use signal_service::domain::article::{
    Article, ArticleAsset, ArticleImageDecision, ArticleStatus, ChangeType, EditorKind,
    OcrBlockStatus,
};
use signal_service::jobs::ocr::OcrArticleImageHandler;
use signal_service::repository::article::{
    ArticleRepository, CapturedArticleCommit, CommitRevisionCommand,
};
use signal_service::repository::article_memory::MemoryArticleStore;
use signal_service::service::content_normalizer::ContentNormalizerService;
use signal_service::service::manual_edit::{
    ManualEditService, PatchArticleContentCommand, ReocrCommand,
};
use signal_service::service::markdown_builder::MarkdownBuilder;
use signal_service::service::ocr_planner::{OCR_VERSION, OcrArticleImagePayload, OcrJobPlanner};
use signal_service::service::revision::{PublishRevisionInput, RevisionService};
use uuid::Uuid;

struct RetryOcr;

#[async_trait]
impl OcrProvider for RetryOcr {
    fn id(&self) -> &str {
        "retry-ocr"
    }

    async fn recognize(&self, request: OcrRequest) -> ProviderResult<OcrResponse> {
        assert_eq!(
            request.image_data.as_deref(),
            Some(b"actual-image".as_slice())
        );
        assert!(request.file_url.is_none());
        Err(ProviderError::retryable("ocr.vendor_busy", "busy", 1))
    }

    async fn cancel(&self) -> ProviderResult<()> {
        Ok(())
    }
}

struct SlowOcr {
    cancelled: Arc<AtomicBool>,
}

#[async_trait]
impl OcrProvider for SlowOcr {
    fn id(&self) -> &str {
        "slow-ocr"
    }

    async fn recognize(&self, _request: OcrRequest) -> ProviderResult<OcrResponse> {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Err(ProviderError::retryable("ocr.slow", "slow", 1))
    }

    async fn cancel(&self) -> ProviderResult<()> {
        self.cancelled.store(true, Ordering::SeqCst);
        Ok(())
    }
}

struct FixedClock(SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

fn now_dt() -> DateTime<Utc> {
    DateTime::<Utc>::from(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
}

async fn seed_article(store: &MemoryArticleStore, with_image: bool) -> (Uuid, Option<Uuid>) {
    let source = Uuid::new_v4();
    store
        .ensure_source_stub(source, "fixture-source")
        .await
        .unwrap();
    let article_id = Uuid::new_v4();
    let md = if with_image {
        "正文段落\n\n![](https://cdn.example/a.jpg)"
    } else {
        "纯文本正文，无图片。"
    };
    let article = Article {
        id: article_id,
        source_account_id: source,
        article_link_id: None,
        original_url: "https://mp.weixin.qq.com/s/x".into(),
        canonical_url: "https://mp.weixin.qq.com/s/x".into(),
        original_title: "t".into(),
        author: None,
        published_at: None,
        fetched_at: now_dt(),
        current_revision: 1,
        content_md: md.into(),
        content_hash: RevisionService::content_hash(md),
        asset_manifest_hash: None,
        status: ArticleStatus::Captured,
        withdrawn_at: None,
    };
    store.insert_article(article).await.unwrap();

    let asset_id = if with_image {
        let id = Uuid::new_v4();
        store
            .insert_asset(ArticleAsset {
                id,
                article_id,
                original_url: Some("https://cdn.example/a.jpg".into()),
                storage_key: format!("articles/{article_id}/a.jpg"),
                asset_hash: Some("hash-a".into()),
                width: Some(800),
                height: Some(600),
                byte_size: Some(100),
                mime_type: Some("image/jpeg".into()),
                asset_kind: "body_image".into(),
                created_at: now_dt(),
            })
            .await
            .unwrap();
        Some(id)
    } else {
        None
    };
    (article_id, asset_id)
}

#[allow(clippy::type_complexity)]
fn stack(
    store: Arc<MemoryArticleStore>,
) -> (
    Arc<RevisionService>,
    Arc<ContentNormalizerService>,
    Arc<ManualEditService>,
    Arc<OcrJobPlanner>,
    Arc<MemoryRepo>,
    Arc<dyn Clock>,
) {
    let jobs = Arc::new(MemoryRepo::new());
    let clock: Arc<dyn Clock> = Arc::new(FixedClock(
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    ));
    let revisions = Arc::new(RevisionService::new(
        store.clone() as Arc<dyn ArticleRepository>
    ));
    let planner = Arc::new(OcrJobPlanner::new(
        jobs.clone() as Arc<dyn job_pipeline::JobRepository>
    ));
    let normalizer = Arc::new(ContentNormalizerService::new(
        store.clone() as Arc<dyn ArticleRepository>,
        revisions.clone(),
        planner.clone(),
        clock.clone(),
    ));
    let manual = Arc::new(ManualEditService::new(
        store.clone() as Arc<dyn ArticleRepository>,
        revisions.clone(),
        planner.clone(),
        jobs.clone() as Arc<dyn job_pipeline::JobRepository>,
        clock.clone(),
    ));
    (revisions, normalizer, manual, planner, jobs, clock)
}

#[tokio::test]
async fn ocr_anchor_in_content_md() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());

    let outcome = normalizer.normalize_captured(article_id).await.unwrap();
    assert!(outcome.content_md.contains("signal-asset://"));
    assert!(outcome.content_md.contains("ocr-anchor:"));
    assert!(!outcome.ocr_job_ids.is_empty());

    let now = DateTime::<Utc>::from(clock.now());
    store
        .upsert_ocr_block(signal_service::domain::article::ArticleOcrBlock {
            id: Uuid::new_v4(),
            article_id,
            asset_id,
            block_anchor: MarkdownBuilder::image_anchor(asset_id),
            recognized_text: Some("图上识别文字".into()),
            confidence: Some(0.95),
            status: OcrBlockStatus::Succeeded,
            provider_ref: Some("fake-ocr".into()),
            provider: Some("fake-ocr".into()),
            model: None,
            provider_version: None,
            ocr_version: Some("v1".into()),
            provider_call_id: None,
            error_code: None,
            attempt: 1,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let (md, _) = normalizer
        .rebuild_content_with_ocr(article_id)
        .await
        .unwrap();
    assert!(md.contains("图上识别文字"));
    assert!(md.contains("[图片文字]"));

    let _ = revisions;
}

#[tokio::test]
async fn explicit_reocr_resets_failed_block_and_creates_new_jobs() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (_, normalizer, manual, _, _, _) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();
    let mut block = store
        .list_ocr_blocks(article_id)
        .await
        .unwrap()
        .into_iter()
        .find(|block| block.asset_id == asset_id)
        .unwrap();
    block.status = OcrBlockStatus::Failed;
    block.error_code = Some("ocr.failed".into());
    store.upsert_ocr_block(block).await.unwrap();

    let first = manual
        .reocr(
            article_id,
            ReocrCommand {
                asset_id: Some(asset_id),
                failed_only: true,
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.job_ids.len(), 1);
    let reset = store
        .list_ocr_blocks(article_id)
        .await
        .unwrap()
        .into_iter()
        .find(|block| block.asset_id == asset_id)
        .unwrap();
    assert_eq!(reset.status, OcrBlockStatus::Pending);
    assert_eq!(reset.error_code, None);

    let second = manual
        .reocr(
            article_id,
            ReocrCommand {
                asset_id: Some(asset_id),
                failed_only: false,
                request_id: None,
            },
        )
        .await
        .unwrap();
    assert_ne!(first.job_ids, second.job_ids);
}

#[tokio::test]
async fn manual_edit_keeps_crawler_revision() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, _) = seed_article(&store, false).await;
    let (_, normalizer, manual, _, _, _) = stack(store.clone());

    normalizer.normalize_captured(article_id).await.unwrap();
    let before = store.list_revisions(article_id).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].editor_kind, EditorKind::Crawler);
    let crawler_md = before[0].content_md.clone();

    manual
        .patch_content(
            article_id,
            PatchArticleContentCommand {
                content_md: "管理员改写后的正文".into(),
                change_reason: Some("fix".into()),
                expected_revision: None,
            },
        )
        .await
        .unwrap();

    let revs = store.list_revisions(article_id).await.unwrap();
    assert_eq!(revs.len(), 2);
    assert_eq!(revs[0].editor_kind, EditorKind::Crawler);
    assert_eq!(revs[0].content_md, crawler_md);
    assert_eq!(revs[1].editor_kind, EditorKind::Administrator);
    assert_eq!(revs[1].content_md, "管理员改写后的正文");

    let events = store.list_change_events(article_id).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.change_type == ChangeType::ArticleEdited)
    );
    let outbox = store.list_outbox_for_article(article_id).await.unwrap();
    assert_eq!(events.len(), outbox.len());
}

#[tokio::test]
async fn asset_manifest_change_emits_event() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, _) = seed_article(&store, false).await;
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();

    let now = DateTime::<Utc>::from(clock.now());
    let new_manifest = RevisionService::asset_manifest_hash(&["new-hash".into()]);
    let article = store.get_article(article_id).await.unwrap().unwrap();
    revisions
        .publish(PublishRevisionInput {
            article_id,
            editor_kind: EditorKind::Crawler,
            content_md: article.content_md.clone(),
            asset_manifest_hash: Some(new_manifest),
            change_reason: "assets_changed".into(),
            change_type: ChangeType::ArticleAssetsChanged,
            status: ArticleStatus::Ready,
            asset_refs: vec![],
            force_new_revision: false,
            now,
        })
        .await
        .unwrap();

    let events = store.list_change_events(article_id).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.change_type == ChangeType::ArticleAssetsChanged)
    );
    let outbox = store.list_outbox_for_article(article_id).await.unwrap();
    assert!(
        outbox
            .iter()
            .any(|o| o.event_type == ChangeType::ArticleAssetsChanged)
    );
}

#[tokio::test]
async fn ocr_rebuild_uses_latest_crawler_revision() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, _) = seed_article(&store, false).await;
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();
    let article = store.get_article(article_id).await.unwrap().unwrap();
    revisions
        .publish(PublishRevisionInput {
            article_id,
            editor_kind: EditorKind::Crawler,
            content_md: format!("{}\nlatest crawler body", article.content_md),
            asset_manifest_hash: article.asset_manifest_hash.clone(),
            change_reason: "latest_crawler_revision".into(),
            change_type: ChangeType::ArticleUpdated,
            status: ArticleStatus::Ready,
            asset_refs: vec![],
            force_new_revision: true,
            now: DateTime::<Utc>::from(clock.now()),
        })
        .await
        .unwrap();
    let (rebuilt, _) = normalizer
        .rebuild_content_with_ocr(article_id)
        .await
        .unwrap();
    assert!(rebuilt.contains("latest crawler body"));
}

#[tokio::test]
async fn missing_outbox_is_rejected() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, _) = seed_article(&store, false).await;
    let cmd = CommitRevisionCommand {
        article_id,
        editor_kind: EditorKind::Administrator,
        content_md: "x".into(),
        content_hash: "h".into(),
        asset_manifest_hash: None,
        change_reason: "test".into(),
        change_type: ChangeType::ArticleEdited,
        status: ArticleStatus::Ready,
        asset_refs: vec![],
        now: now_dt(),
        force_new_revision: true,
    };
    store
        .commit_revision_without_outbox_for_test(cmd)
        .await
        .expect("漏写 outbox 应被拒绝");
}

#[tokio::test]
async fn ocr_failure_sets_ready_with_ocr_warning() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();

    let fake = FakeOcrProvider::new("fake-ocr");
    fake.fail_next();
    let object_store = Arc::new(FakeObjectStore::new());
    let stored = object_store
        .put_bytes(
            "articles/test",
            b"real-image-bytes",
            Some("image/jpeg"),
            None,
            None,
        )
        .await
        .unwrap();
    let handler = OcrArticleImageHandler {
        articles: store.clone() as Arc<dyn ArticleRepository>,
        normalizer: normalizer.clone(),
        revisions: revisions.clone(),
        ocr: Arc::new(fake),
        object_store,
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        timeout: Duration::from_secs(1),
        clock: clock.clone(),
    };

    let (tx, _rx) = ProgressSender::channel(8);
    let payload = OcrArticleImagePayload {
        article_id,
        asset_id,
        asset_hash: "hash-a".into(),
        block_anchor: MarkdownBuilder::image_anchor(asset_id),
        storage_key: stored.storage_key,
        ocr_version: OCR_VERSION.into(),
    };
    let ctx = JobContext {
        worker_id: "test".into(),
        payload_json: serde_json::to_value(&payload).unwrap(),
        progress_tx: tx,
        cancel_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let result = handler.execute(JobId::new(), ctx).await.unwrap();
    assert_eq!(result["article_status"], "ready_with_ocr_warning");

    let article = store.get_article(article_id).await.unwrap().unwrap();
    assert_eq!(article.status, ArticleStatus::ReadyWithOcrWarning);
    let revs = store.list_revisions(article_id).await.unwrap();
    assert!(revs.iter().any(|r| r.editor_kind == EditorKind::Crawler));
}

#[tokio::test]
async fn retryable_ocr_uses_real_bytes_and_keeps_block_pending() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();
    let object_store = Arc::new(FakeObjectStore::new());
    let stored = object_store
        .put_bytes(
            "articles/test",
            b"actual-image",
            Some("image/png"),
            None,
            None,
        )
        .await
        .unwrap();
    let handler = OcrArticleImageHandler {
        articles: store.clone(),
        normalizer,
        revisions,
        ocr: Arc::new(RetryOcr),
        object_store,
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        timeout: Duration::from_secs(1),
        clock,
    };
    let (tx, _rx) = ProgressSender::channel(8);
    let result = handler
        .execute(
            JobId::new(),
            JobContext {
                worker_id: "test".into(),
                payload_json: serde_json::to_value(OcrArticleImagePayload {
                    article_id,
                    asset_id,
                    asset_hash: "hash-a".into(),
                    block_anchor: MarkdownBuilder::image_anchor(asset_id),
                    storage_key: stored.storage_key,
                    ocr_version: OCR_VERSION.into(),
                })
                .unwrap(),
                progress_tx: tx,
                cancel_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
        )
        .await
        .unwrap_err();
    assert!(result.retryable);
    assert_eq!(result.code, "ocr.vendor_busy");
    let blocks = store.list_ocr_blocks(article_id).await.unwrap();
    let block = blocks
        .iter()
        .find(|block| block.asset_id == asset_id)
        .unwrap();
    assert_eq!(block.status, OcrBlockStatus::Pending);
    assert_eq!(block.error_code.as_deref(), Some("ocr.vendor_busy"));
    assert_eq!(block.attempt, 1);
    assert_eq!(
        store.get_article(article_id).await.unwrap().unwrap().status,
        ArticleStatus::Captured
    );
}

#[tokio::test]
async fn ocr_timeout_is_retryable_and_cancels_provider() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    normalizer.normalize_captured(article_id).await.unwrap();
    let object_store = Arc::new(FakeObjectStore::new());
    let stored = object_store
        .put_bytes(
            "articles/test",
            b"actual-image",
            Some("image/png"),
            None,
            None,
        )
        .await
        .unwrap();
    let cancelled = Arc::new(AtomicBool::new(false));
    let handler = OcrArticleImageHandler {
        articles: store,
        normalizer,
        revisions,
        ocr: Arc::new(SlowOcr {
            cancelled: cancelled.clone(),
        }),
        object_store,
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        timeout: Duration::from_millis(10),
        clock,
    };
    let (tx, _rx) = ProgressSender::channel(8);
    let error = handler
        .execute(
            JobId::new(),
            JobContext {
                worker_id: "test".into(),
                payload_json: serde_json::to_value(OcrArticleImagePayload {
                    article_id,
                    asset_id,
                    asset_hash: "hash-a".into(),
                    block_anchor: MarkdownBuilder::image_anchor(asset_id),
                    storage_key: stored.storage_key,
                    ocr_version: OCR_VERSION.into(),
                })
                .unwrap(),
                progress_tx: tx,
                cancel_requested: Arc::new(AtomicBool::new(false)),
            },
        )
        .await
        .unwrap_err();
    assert!(error.retryable);
    assert_eq!(error.code, "ocr.timeout");
    assert!(cancelled.load(Ordering::SeqCst));
}

#[tokio::test]
async fn pre_cancelled_ocr_never_calls_recognize_and_cancels_provider() {
    let store = Arc::new(MemoryArticleStore::new());
    let (article_id, asset_id) = seed_article(&store, true).await;
    let asset_id = asset_id.unwrap();
    let (revisions, normalizer, _, _, _, clock) = stack(store.clone());
    let cancelled = Arc::new(AtomicBool::new(false));
    let handler = OcrArticleImageHandler {
        articles: store,
        normalizer,
        revisions,
        ocr: Arc::new(SlowOcr {
            cancelled: cancelled.clone(),
        }),
        object_store: Arc::new(FakeObjectStore::new()),
        concurrency: Arc::new(tokio::sync::Semaphore::new(1)),
        timeout: Duration::from_secs(1),
        clock,
    };
    let (tx, _rx) = ProgressSender::channel(8);
    let error = handler
        .execute(
            JobId::new(),
            JobContext {
                worker_id: "test".into(),
                payload_json: serde_json::to_value(OcrArticleImagePayload {
                    article_id,
                    asset_id,
                    asset_hash: "hash-a".into(),
                    block_anchor: MarkdownBuilder::image_anchor(asset_id),
                    storage_key: "unused".into(),
                    ocr_version: OCR_VERSION.into(),
                })
                .unwrap(),
                progress_tx: tx,
                cancel_requested: Arc::new(AtomicBool::new(true)),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "job_pipeline.cancelled");
    assert!(cancelled.load(Ordering::SeqCst));
}

fn atomic_capture(source: Uuid, version: &str, with_asset: bool) -> CapturedArticleCommit {
    let article_id = Uuid::new_v4();
    let asset_id = Uuid::new_v4();
    CapturedArticleCommit {
        article: Article {
            id: article_id,
            source_account_id: source,
            article_link_id: None,
            original_url: "https://mp.weixin.qq.com/s/atomic".into(),
            canonical_url: "https://mp.weixin.qq.com/s/atomic".into(),
            original_title: "atomic".into(),
            author: None,
            published_at: None,
            fetched_at: now_dt(),
            current_revision: 0,
            content_md: format!("body-{version}"),
            content_hash: format!("content-{version}"),
            asset_manifest_hash: with_asset.then(|| format!("assets-{version}")),
            status: ArticleStatus::Captured,
            withdrawn_at: None,
        },
        assets: with_asset
            .then(|| ArticleAsset {
                id: asset_id,
                article_id,
                original_url: Some("https://cdn.example/a.jpg".into()),
                storage_key: format!("articles/{article_id}/a.jpg"),
                asset_hash: Some(format!("asset-{version}")),
                width: Some(800),
                height: Some(600),
                byte_size: Some(100),
                mime_type: Some("image/jpeg".into()),
                asset_kind: "body_image".into(),
                created_at: now_dt(),
            })
            .into_iter()
            .collect(),
        image_decisions: with_asset
            .then(|| ArticleImageDecision {
                id: Uuid::new_v4(),
                article_id,
                candidate_url: "https://cdn.example/a.jpg".into(),
                decision: "kept".into(),
                measured_width: Some(800),
                measured_height: Some(600),
                reason_json: serde_json::json!({}),
                asset_id: Some(asset_id),
                created_at: now_dt(),
            })
            .into_iter()
            .collect(),
        change_reason: "capture".into(),
        now: now_dt(),
    }
}

#[tokio::test]
async fn atomic_capture_is_idempotent_and_replaces_assets() {
    let store = Arc::new(MemoryArticleStore::new());
    let source = Uuid::new_v4();
    store.ensure_source_stub(source, "source").await.unwrap();
    let first = store
        .commit_captured_article(atomic_capture(source, "v1", true))
        .await
        .unwrap();
    assert!(first.created && first.changed);
    assert_eq!(first.article.current_revision, 1);
    let same = store
        .commit_captured_article(atomic_capture(source, "v1", true))
        .await
        .unwrap();
    assert!(!same.created && !same.changed);
    assert_eq!(
        store.list_revisions(first.article.id).await.unwrap().len(),
        1
    );
    let changed = store
        .commit_captured_article(atomic_capture(source, "v2", false))
        .await
        .unwrap();
    assert!(!changed.created && changed.changed);
    assert_eq!(changed.article.current_revision, 2);
    assert!(
        store
            .list_assets(first.article.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .list_decisions(first.article.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .list_change_events(first.article.id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        store
            .list_outbox_for_article(first.article.id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn atomic_capture_failure_and_missing_outbox_leave_no_partial_state() {
    let store = MemoryArticleStore::new();
    let source = Uuid::new_v4();
    store.ensure_source_stub(source, "source").await.unwrap();
    let mut bad = atomic_capture(source, "bad", false);
    bad.image_decisions.push(ArticleImageDecision {
        id: Uuid::new_v4(),
        article_id: bad.article.id,
        candidate_url: "x".into(),
        decision: "kept".into(),
        measured_width: None,
        measured_height: None,
        reason_json: serde_json::json!({}),
        asset_id: Some(Uuid::new_v4()),
        created_at: now_dt(),
    });
    assert!(store.commit_captured_article(bad).await.is_err());
    assert_eq!(store.count_articles_for_source(source).await.unwrap(), 0);

    let committed = store
        .commit_captured_article(atomic_capture(source, "v1", false))
        .await
        .unwrap();
    let before = store
        .get_article(committed.article.id)
        .await
        .unwrap()
        .unwrap();
    let result = store
        .commit_revision_without_outbox_for_test(CommitRevisionCommand {
            article_id: before.id,
            editor_kind: EditorKind::Administrator,
            content_md: "bad".into(),
            content_hash: "bad".into(),
            asset_manifest_hash: None,
            change_reason: "test".into(),
            change_type: ChangeType::ArticleEdited,
            status: ArticleStatus::Ready,
            asset_refs: vec![],
            now: now_dt(),
            force_new_revision: true,
        })
        .await;
    assert!(result.is_ok());
    let after = store.get_article(before.id).await.unwrap().unwrap();
    assert_eq!(after.current_revision, before.current_revision);
    assert_eq!(store.list_revisions(before.id).await.unwrap().len(), 1);
    assert_eq!(store.list_change_events(before.id).await.unwrap().len(), 1);
    assert_eq!(
        store
            .list_outbox_for_article(before.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn concurrent_atomic_capture_creates_one_identity() {
    let store = Arc::new(MemoryArticleStore::new());
    let source = Uuid::new_v4();
    store.ensure_source_stub(source, "source").await.unwrap();
    let (left, right) = tokio::join!(
        store.commit_captured_article(atomic_capture(source, "same", false)),
        store.commit_captured_article(atomic_capture(source, "same", false)),
    );
    let rows = [left.unwrap(), right.unwrap()];
    assert_eq!(rows.iter().filter(|row| row.created).count(), 1);
    assert_eq!(rows.iter().filter(|row| row.changed).count(), 1);
    assert_eq!(store.count_articles_for_source(source).await.unwrap(), 1);
}
