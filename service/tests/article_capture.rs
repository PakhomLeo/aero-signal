//! M09 article-capture 集成测试（内存 repo + fixture HTML，无外网）。

use std::sync::Arc;

use crawler_capture::{
    CrawlerError, CrawlerResult, FakeObjectStore, FetchedAsset, MemoryByteFetcher,
    MemoryPageFetcher, ObjectStore, StoredObject, canonicalize_url,
};
use job_pipeline::{
    CreateJob, JobRepository, JobStatus, KIND_FETCH_ARTICLE_DETAIL, KIND_HISTORY_BACKFILL,
    MemoryRepo, Worker, kinds::idempotency,
};
use platform_connectors::{
    ArticleCandidateEvidence, CapabilitySet, ConnectorCapability, FakeConnector, NAME_FAKE,
    default_registry,
};
use server_core::{Clock, SystemClock};
use uuid::Uuid;

use signal_service::adapters::DefaultProxyProviderAdapter;
use signal_service::adapters::secret_store::{MemorySecretStore, SecretStore};
use signal_service::domain::collector::{
    CollectorAccount, CollectorAccountStatus, CollectorEgressBinding, CollectorLoginSession,
};
use signal_service::jobs::signal_job_registry_with_capture;
use signal_service::repository::{
    ArticleRepository, CollectorRepository, EgressRepository, MemoryArticleStore,
    MemoryCollectorStore, MemoryEgressStore,
};
use signal_service::service::{
    ArticleCaptureService, DynamicLeaseManager, EgressResolver, FetchArticleDetailPayload,
    FetchSourcePayload, HistoryBackfillPayload,
};

const FIXTURE: &str = r#"<!DOCTYPE html>
<html><head><title>Fixture 文章</title>
<meta name="author" content="作者甲"/>
</head>
<body>
<div class="header"><img class="icon" src="https://cdn.example.com/icon/logo.png" width="32" height="32"/></div>
<div id="js_content">
  <p>这是一段足够长的正文内容，用于验证静态 HTML fixture 抓取路径。</p>
  <img src="https://cdn.example.com/body/hero.jpg" width="400" height="300" alt="hero"/>
  <img data-src="https://cdn.example.com/body/lazy.jpg" width="200" height="200" class="lazy"/>
  <img src="https://cdn.example.com/body/tiny.png" width="16" height="16"/>
  <img style="display:none" src="https://cdn.example.com/track.gif" width="1" height="1"/>
</div>
</body></html>"#;

async fn setup_service(
    urls: &[&str],
) -> (
    Arc<ArticleCaptureService>,
    Arc<MemoryArticleStore>,
    Arc<MemoryRepo>,
) {
    let (service, articles, jobs, _, _) =
        setup_service_with_store(urls, Arc::new(FakeObjectStore::new())).await;
    (service, articles, jobs)
}

async fn setup_service_with_store(
    urls: &[&str],
    object_store: Arc<dyn ObjectStore>,
) -> (
    Arc<ArticleCaptureService>,
    Arc<MemoryArticleStore>,
    Arc<MemoryRepo>,
    Arc<MemoryPageFetcher>,
    Arc<MemoryByteFetcher>,
) {
    let articles = Arc::new(MemoryArticleStore::new());
    let jobs = Arc::new(MemoryRepo::new());
    let connectors = Arc::new(default_registry());
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let pages = Arc::new(MemoryPageFetcher::default());
    for url in urls {
        pages.insert_html(canonicalize_url(url), FIXTURE);
    }
    let bytes = Arc::new(MemoryByteFetcher::new());
    for (url, width, height, marker) in [
        ("https://cdn.example.com/icon/logo.png", 32, 32, 1),
        ("https://cdn.example.com/body/hero.jpg", 400, 300, 2),
        ("https://cdn.example.com/body/lazy.jpg", 200, 200, 3),
        ("https://cdn.example.com/body/tiny.png", 16, 16, 4),
        ("https://cdn.example.com/track.gif", 1, 1, 5),
    ] {
        bytes
            .insert_asset(
                url,
                FetchedAsset {
                    bytes: vec![marker; 32],
                    content_type: "image/png".into(),
                    final_url: url.into(),
                    width,
                    height,
                },
            )
            .await;
    }
    let egress_repo = Arc::new(MemoryEgressStore::with_local_direct());
    let secrets = Arc::new(MemorySecretStore::new());
    let egress = Arc::new(EgressResolver::new(
        egress_repo as Arc<dyn EgressRepository>,
        Arc::new(DynamicLeaseManager::new()),
        Arc::new(DefaultProxyProviderAdapter::new(secrets)),
    ));
    let svc = Arc::new(
        ArticleCaptureService::new(
            articles.clone() as Arc<dyn signal_service::repository::ArticleRepository>,
            jobs.clone() as Arc<dyn job_pipeline::JobRepository>,
            connectors,
            clock,
            object_store,
        )
        .with_page_fetcher(pages.clone())
        .with_asset_fetcher(bytes.clone())
        .with_egress_resolver(egress),
    );
    (svc, articles, jobs, pages, bytes)
}

struct FailingObjectStore;

#[async_trait::async_trait]
impl ObjectStore for FailingObjectStore {
    async fn put_bytes(
        &self,
        _key_prefix: &str,
        _bytes: &[u8],
        _mime_type: Option<&str>,
        _width: Option<u32>,
        _height: Option<u32>,
    ) -> CrawlerResult<StoredObject> {
        Err(CrawlerError::store_failed("injected object store failure"))
    }

    async fn get_bytes(&self, _storage_key: &str) -> CrawlerResult<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn exists_by_hash(&self, _content_hash: &str) -> CrawlerResult<bool> {
        Ok(false)
    }
}

async fn setup_discovery_service() -> (
    Arc<ArticleCaptureService>,
    Arc<MemoryRepo>,
    Uuid,
    Arc<FakeConnector>,
) {
    let articles = Arc::new(MemoryArticleStore::new());
    let jobs = Arc::new(MemoryRepo::new());
    let fake = Arc::new(FakeConnector::new());
    let mut registry = platform_connectors::ConnectorRegistry::new();
    registry.register(fake.clone());
    let connectors = Arc::new(registry);
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let collectors = Arc::new(MemoryCollectorStore::new());
    let secrets = Arc::new(MemorySecretStore::new());
    let account_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    collectors
        .insert_account(&CollectorAccount {
            id: account_id,
            connector: NAME_FAKE.into(),
            display_name: "fake".into(),
            external_account_ref: None,
            status: CollectorAccountStatus::Active,
            capability_json: serde_json::to_value(CapabilitySet::new([
                ConnectorCapability::DiscoverRecent,
            ]))
            .unwrap(),
            last_probe_at: Some(now),
            last_success_at: Some(now),
            last_failure_at: None,
            last_error_code: None,
            cooldown_until: None,
            daily_budget_remaining: None,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();
    collectors
        .upsert_egress_binding(&CollectorEgressBinding {
            collector_account_id: account_id,
            fixed_egress_id: None,
            bound_at: now,
            changed_at: None,
            requires_relogin: false,
        })
        .await
        .unwrap();
    let secret_ref = secrets.put(r#"{"authorized_mp_ids":[]}"#).await.unwrap();
    collectors
        .insert_login_session(&CollectorLoginSession {
            id: Uuid::new_v4(),
            collector_account_id: account_id,
            credential_secret_ref: secret_ref,
            browser_profile_ref: None,
            authorized_mp_ids: vec!["biz-1".into()],
            expires_at: None,
            confirmed_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    let service = Arc::new(
        ArticleCaptureService::new(
            articles,
            jobs.clone(),
            connectors,
            clock,
            Arc::new(FakeObjectStore::new()),
        )
        .with_collector_credentials(collectors, secrets),
    );
    (service, jobs, account_id, fake)
}

#[tokio::test]
async fn identity_dedup_updates_link_without_duplicate_article() {
    let first_url = "https://mp.weixin.qq.com/s?__biz=MzA1&mid=100&idx=1&sn=x";
    let second_url = "https://mp.weixin.qq.com/s/different-path";
    let (svc, articles, _) = setup_service(&[first_url, second_url]).await;
    let source = Uuid::new_v4();
    let payload = FetchArticleDetailPayload {
        source_account_id: source,
        collector_account_id: None,
        canonical_url: first_url.into(),
        wechat_biz: Some("MzA1".into()),
        wechat_mid: Some("100".into()),
        wechat_idx: Some("1".into()),
        wechat_sn: Some("x".into()),
        title_hint: Some("t1".into()),
        candidate_published_at: None,
        candidate_author: None,
        candidate_cover_url: None,
        candidate_evidence: None,
        source_heartbeat_event_id: None,
        heartbeat_manual: false,
        heartbeat_recalculate_next: false,
        discovered_via: "test".into(),
    };

    let a = svc.capture_detail(&payload).await.expect("first capture");
    assert!(a.created);

    let mut again = payload.clone();
    again.canonical_url = second_url.into();
    again.title_hint = Some("t2".into());
    let b = svc.capture_detail(&again).await.expect("second capture");
    assert!(!b.created);
    assert_eq!(a.article_id, b.article_id);
    assert_eq!(articles.count_articles_for_source(source).await.unwrap(), 1);
}

#[tokio::test]
async fn refetch_content_and_asset_removal_creates_one_new_revision() {
    let url = "https://mp.weixin.qq.com/s/refetch-change";
    let (svc, articles, _, pages, _) =
        setup_service_with_store(&[url], Arc::new(FakeObjectStore::new())).await;
    let source = Uuid::new_v4();
    let payload = FetchArticleDetailPayload {
        source_account_id: source,
        collector_account_id: None,
        canonical_url: url.into(),
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
        discovered_via: "test".into(),
    };
    let first = svc.capture_detail(&payload).await.unwrap();
    pages.insert_html(
        url,
        r#"<html><head><title>更新</title></head><body><div id="js_content"><p>更新后的正文内容足够长，并且已经明确删除全部正文图片。</p></div></body></html>"#,
    );
    let second = svc.capture_detail(&payload).await.unwrap();
    assert!(!second.created);
    assert_eq!(second.article_id, first.article_id);
    assert_eq!(
        articles
            .list_revisions(first.article_id)
            .await
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        articles
            .list_assets(first.article_id)
            .await
            .unwrap()
            .iter()
            .filter(|asset| asset.asset_kind == "body_image")
            .count(),
        0
    );
    assert_eq!(
        articles
            .list_outbox_for_article(first.article_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn article_fixture_extract_classifies_small_and_decorative() {
    let url = "https://mp.weixin.qq.com/s/fixture-1";
    let (svc, articles, _) = setup_service(&[url]).await;
    let source = Uuid::new_v4();
    let outcome = svc
        .capture_detail(&FetchArticleDetailPayload {
            source_account_id: source,
            collector_account_id: None,
            canonical_url: url.into(),
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
            wechat_sn: None,
            title_hint: Some("hint".into()),
            candidate_published_at: None,
            candidate_author: None,
            candidate_cover_url: None,
            candidate_evidence: None,
            source_heartbeat_event_id: None,
            heartbeat_manual: false,
            heartbeat_recalculate_next: false,
            discovered_via: "fixture".into(),
        })
        .await
        .unwrap();

    assert!(outcome.created);
    assert!(outcome.kept_assets >= 1);
    let decisions = articles.list_decisions(outcome.article_id).await.unwrap();
    assert!(decisions.iter().any(|d| d.decision == "kept"));
    assert!(decisions.iter().any(|d| d.decision == "too_small"));
    assert!(decisions.iter().any(|d| d.decision == "decorative"));
    assert!(decisions.iter().any(|d| d.decision == "hidden"));
}

#[tokio::test]
async fn object_store_failure_leaves_no_partial_article() {
    let url = "https://mp.weixin.qq.com/s/store-failure";
    let (svc, articles, _, _, _) =
        setup_service_with_store(&[url], Arc::new(FailingObjectStore)).await;
    let source = Uuid::new_v4();
    let error = svc
        .capture_detail(&FetchArticleDetailPayload {
            source_account_id: source,
            collector_account_id: None,
            canonical_url: url.into(),
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
            discovered_via: "test".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "crawler.store_failed");
    assert_eq!(articles.count_articles_for_source(source).await.unwrap(), 0);
}

#[tokio::test]
async fn one_asset_failure_keeps_article_and_retries_independently() {
    let url = "https://mp.weixin.qq.com/s/asset-partial";
    let (svc, articles, jobs, _, bytes) =
        setup_service_with_store(&[url], Arc::new(FakeObjectStore::new())).await;
    bytes
        .insert_error(
            "https://cdn.example.com/body/hero.jpg",
            CrawlerError::fetch_failed("temporary image timeout"),
        )
        .await;
    let source = Uuid::new_v4();
    let payload = FetchArticleDetailPayload {
        source_account_id: source,
        collector_account_id: None,
        canonical_url: url.into(),
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
        discovered_via: "test".into(),
    };

    let first = svc.capture_detail(&payload).await.unwrap();
    assert!(first.created);
    assert_eq!(first.asset_warnings, 1);
    assert!(first.asset_retry_job_id.is_some());
    assert_eq!(articles.count_articles_for_source(source).await.unwrap(), 1);
    assert!(
        articles
            .list_decisions(first.article_id)
            .await
            .unwrap()
            .iter()
            .any(|decision| decision.decision == "download_failed")
    );

    bytes
        .insert_asset(
            "https://cdn.example.com/body/hero.jpg",
            FetchedAsset {
                bytes: vec![2; 32],
                content_type: "image/png".into(),
                final_url: "https://cdn.example.com/body/hero.jpg".into(),
                width: 400,
                height: 300,
            },
        )
        .await;
    let registry = signal_job_registry_with_capture(Some(svc.clone()));
    let worker = Worker::new(
        "w-asset-retry",
        jobs.clone() as Arc<dyn job_pipeline::JobRepository>,
    )
    .with_registry(&registry);
    worker.run_once().await.unwrap();

    let retry = jobs
        .get_job(job_pipeline::JobId::from_raw(
            first.asset_retry_job_id.unwrap(),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retry.status, JobStatus::Succeeded);
    assert_eq!(
        articles
            .list_revisions(first.article_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn cancelled_capture_does_not_commit_article() {
    let url = "https://mp.weixin.qq.com/s/cancelled";
    let (svc, articles, _) = setup_service(&[url]).await;
    let source = Uuid::new_v4();
    let cancelled = std::sync::atomic::AtomicBool::new(true);
    let error = svc
        .capture_detail_with_cancel(
            &FetchArticleDetailPayload {
                source_account_id: source,
                collector_account_id: None,
                canonical_url: url.into(),
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
                discovered_via: "test".into(),
            },
            Some(&cancelled),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "job.cancelled");
    assert_eq!(articles.count_articles_for_source(source).await.unwrap(), 0);
}

#[tokio::test]
async fn raw_html_and_kept_images_are_readable_from_object_store() {
    let url = "https://mp.weixin.qq.com/s/object-roundtrip";
    let store = Arc::new(FakeObjectStore::new());
    let (svc, articles, _, _, _) = setup_service_with_store(&[url], store.clone()).await;
    let outcome = svc
        .capture_detail(&FetchArticleDetailPayload {
            source_account_id: Uuid::new_v4(),
            collector_account_id: None,
            canonical_url: url.into(),
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
            discovered_via: "test".into(),
        })
        .await
        .unwrap();
    let assets = articles.list_assets(outcome.article_id).await.unwrap();
    assert!(assets.iter().any(|asset| asset.asset_kind == "raw_html"));
    assert!(assets.iter().any(|asset| asset.asset_kind == "body_image"));
    for asset in assets {
        assert!(store.get_bytes(&asset.storage_key).await.unwrap().is_some());
    }
}

#[tokio::test]
async fn candidate_metadata_survives_detail_capture_and_evidence_is_persisted() {
    let url = "https://mp.weixin.qq.com/s/candidate-metadata";
    let store = Arc::new(FakeObjectStore::new());
    let (svc, articles, _, pages, _) = setup_service_with_store(&[url], store.clone()).await;
    pages.insert_html(
        url,
        r#"<html><head><title>详情标题</title></head><body><div id="js_content"><p>一段足够长的正文，用于验证候选元数据回退和证据保存。</p></div></body></html>"#,
    );
    let published_at = chrono::DateTime::parse_from_rfc3339("2026-07-01T08:30:00+08:00")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let outcome = svc
        .capture_detail(&FetchArticleDetailPayload {
            source_account_id: Uuid::new_v4(),
            collector_account_id: None,
            canonical_url: url.into(),
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
            wechat_sn: None,
            title_hint: Some("列表标题".into()),
            candidate_published_at: Some(published_at),
            candidate_author: Some("列表作者".into()),
            candidate_cover_url: None,
            candidate_evidence: Some(ArticleCandidateEvidence {
                connector: NAME_FAKE.into(),
                cursor: 20,
                response_summary_hash: "a".repeat(64),
                observed_at: chrono::Utc::now(),
            }),
            source_heartbeat_event_id: None,
            heartbeat_manual: false,
            heartbeat_recalculate_next: false,
            discovered_via: "test".into(),
        })
        .await
        .unwrap();

    let article = articles
        .get_article(outcome.article_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(article.author.as_deref(), Some("列表作者"));
    assert_eq!(article.published_at, Some(published_at));
    let assets = articles.list_assets(outcome.article_id).await.unwrap();
    assert!(
        assets
            .iter()
            .any(|asset| asset.asset_kind == "sanitized_html")
    );
    assert!(
        assets
            .iter()
            .any(|asset| asset.asset_kind == "candidate_evidence")
    );
    for asset in assets.iter().filter(|asset| {
        matches!(
            asset.asset_kind.as_str(),
            "sanitized_html" | "candidate_evidence"
        )
    }) {
        assert!(store.get_bytes(&asset.storage_key).await.unwrap().is_some());
    }
}

#[tokio::test]
async fn candidate_cover_is_archived_separately_from_body_images() {
    let url = "https://mp.weixin.qq.com/s/candidate-cover";
    let cover = "https://cdn.example.com/covers/main.jpg";
    let (svc, articles, _, _, bytes) =
        setup_service_with_store(&[url], Arc::new(FakeObjectStore::new())).await;
    bytes
        .insert_asset(
            cover,
            FetchedAsset {
                bytes: vec![9; 32],
                content_type: "image/jpeg".into(),
                final_url: cover.into(),
                width: 900,
                height: 383,
            },
        )
        .await;
    let outcome = svc
        .capture_detail(&FetchArticleDetailPayload {
            source_account_id: Uuid::new_v4(),
            collector_account_id: None,
            canonical_url: url.into(),
            wechat_biz: None,
            wechat_mid: None,
            wechat_idx: None,
            wechat_sn: None,
            title_hint: None,
            candidate_published_at: None,
            candidate_author: None,
            candidate_cover_url: Some(cover.into()),
            candidate_evidence: None,
            source_heartbeat_event_id: None,
            heartbeat_manual: false,
            heartbeat_recalculate_next: false,
            discovered_via: "test".into(),
        })
        .await
        .unwrap();
    assert_eq!(outcome.asset_warnings, 0);
    assert!(
        articles
            .list_assets(outcome.article_id)
            .await
            .unwrap()
            .iter()
            .any(|asset| asset.asset_kind == "cover_image"
                && asset.original_url.as_deref() == Some(cover))
    );
}

#[tokio::test]
async fn article_detail_job_idempotent_no_double_write() {
    let url = "https://mp.weixin.qq.com/s?__biz=BizX&mid=9&idx=2";
    let (svc, articles, jobs) = setup_service(&[url]).await;
    let source = Uuid::new_v4();
    let payload = FetchArticleDetailPayload {
        source_account_id: source,
        collector_account_id: None,
        canonical_url: url.into(),
        wechat_biz: Some("BizX".into()),
        wechat_mid: Some("9".into()),
        wechat_idx: Some("2".into()),
        wechat_sn: None,
        title_hint: None,
        candidate_published_at: None,
        candidate_author: None,
        candidate_cover_url: None,
        candidate_evidence: None,
        source_heartbeat_event_id: None,
        heartbeat_manual: false,
        heartbeat_recalculate_next: false,
        discovered_via: "job".into(),
    };

    let key = svc.detail_idempotency_key(&payload);
    assert!(key.starts_with("detail:"));

    let j1 = jobs
        .create_job(CreateJob::new(
            KIND_FETCH_ARTICLE_DETAIL,
            key.clone(),
            serde_json::to_value(&payload).unwrap(),
        ))
        .await
        .unwrap();
    let j2 = jobs
        .create_job(CreateJob::new(
            KIND_FETCH_ARTICLE_DETAIL,
            key,
            serde_json::to_value(&payload).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(j1.id, j2.id);

    let reg = signal_job_registry_with_capture(Some(svc.clone()));
    let worker = Worker::new(
        "w-article",
        jobs.clone() as Arc<dyn job_pipeline::JobRepository>,
    )
    .with_registry(&reg);

    worker.run_once().await.unwrap();
    let after = jobs.get_job(j1.id).await.unwrap().unwrap();
    assert_eq!(after.status, JobStatus::Succeeded);

    // 再次执行 capture（模拟重复消费）不应双写
    let again = svc.capture_detail(&payload).await.unwrap();
    assert!(!again.created);
    assert_eq!(articles.count_articles_for_source(source).await.unwrap(), 1);
    assert_eq!(
        articles
            .list_revisions(again.article_id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn article_fetch_source_enqueues_detail_jobs() {
    let (svc, jobs, account_id, _) = setup_discovery_service().await;
    let source = Uuid::new_v4();
    let heartbeat = Uuid::new_v4().to_string();

    let result = svc
        .fetch_source(&FetchSourcePayload {
            source_account_id: source,
            source_identity: "fake-mp".into(),
            heartbeat_event_id: heartbeat.clone(),
            collector_account_id: account_id,
            connector: NAME_FAKE.into(),
            limit: 3,
            manual: false,
            recalculate_next: false,
            daily_plan_id: None,
        })
        .await
        .unwrap();

    assert_eq!(result["discovered"], 3);
    let ids = result["detail_job_ids"].as_array().unwrap();
    assert_eq!(ids.len(), 3);

    let key = idempotency::source_fetch(&source.to_string(), &heartbeat);
    let job_a = svc
        .enqueue_fetch_source(FetchSourcePayload {
            source_account_id: source,
            source_identity: "fake-mp".into(),
            heartbeat_event_id: heartbeat.clone(),
            collector_account_id: account_id,
            connector: NAME_FAKE.into(),
            limit: 3,
            manual: false,
            recalculate_next: false,
            daily_plan_id: None,
        })
        .await
        .unwrap();
    let job_b = svc
        .enqueue_fetch_source(FetchSourcePayload {
            source_account_id: source,
            source_identity: "fake-mp".into(),
            heartbeat_event_id: heartbeat,
            collector_account_id: account_id,
            connector: NAME_FAKE.into(),
            limit: 3,
            manual: false,
            recalculate_next: false,
            daily_plan_id: None,
        })
        .await
        .unwrap();
    assert_eq!(job_a, job_b);
    let _ = jobs;
    let _ = key;
}

#[tokio::test]
async fn article_handlers_registered_in_registry() {
    let (svc, _, _) = setup_service(&[]).await;
    let reg = signal_job_registry_with_capture(Some(svc));
    assert!(reg.get(KIND_FETCH_ARTICLE_DETAIL).is_some());
    assert!(reg.get(job_pipeline::KIND_FETCH_SOURCE).is_some());
    assert!(reg.get(job_pipeline::KIND_HISTORY_BACKFILL).is_some());
    assert!(reg.get(job_pipeline::KIND_RETRY_ARTICLE_ASSETS).is_some());
    // 确认不是 FakeHandler 空跑：kind 匹配
    let h = reg.get(KIND_FETCH_ARTICLE_DETAIL).unwrap();
    assert_eq!(h.kind(), KIND_FETCH_ARTICLE_DETAIL);
}

#[tokio::test]
async fn article_fake_connector_discover_pool_customizable() {
    let (svc, _, account_id, fake) = setup_discovery_service().await;
    fake.set_discover_pool(vec![platform_connectors::ArticleCandidate {
        title: "仅一篇".into(),
        canonical_url: "https://mp.weixin.qq.com/s?__biz=Z&mid=1&idx=1".into(),
        published_at: None,
        identity: Some("z-1".into()),
        author: None,
        cover_url: None,
        evidence: None,
    }]);
    let result = svc
        .fetch_source(&FetchSourcePayload {
            source_account_id: Uuid::new_v4(),
            source_identity: "z".into(),
            heartbeat_event_id: "hb-1".into(),
            collector_account_id: account_id,
            connector: NAME_FAKE.into(),
            limit: 3,
            manual: false,
            recalculate_next: false,
            daily_plan_id: None,
        })
        .await
        .unwrap();
    assert_eq!(result["discovered"], 1);
}

#[tokio::test]
async fn history_backfill_checkpoints_each_page_and_enqueues_next_page() {
    let (svc, jobs, account_id, _) = setup_discovery_service().await;
    let payload = HistoryBackfillPayload {
        source_account_id: Uuid::new_v4(),
        source_identity: "fake-mp".into(),
        collector_account_id: account_id,
        connector: NAME_FAKE.into(),
        run_id: "run-1".into(),
        cursor: 0,
        page_size: 2,
        remaining_pages: 3,
    };

    let result = svc.run_history_backfill_page(&payload, None).await.unwrap();
    assert_eq!(result["checkpoint_cursor"], 0);
    assert_eq!(result["next_cursor"], 2);
    assert_eq!(result["candidate_count"], 2);
    assert_eq!(result["complete"], false);
    assert!(result["next_job_id"].is_string());

    let queued = jobs
        .list_jobs(Some(KIND_HISTORY_BACKFILL), None, 10)
        .await
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].payload_json["cursor"], 2);
    assert_eq!(queued[0].payload_json["remaining_pages"], 2);

    let a = svc.enqueue_history_backfill(payload.clone()).await.unwrap();
    let b = svc.enqueue_history_backfill(payload).await.unwrap();
    assert_eq!(a, b);
}
