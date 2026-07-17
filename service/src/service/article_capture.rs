//! ArticleCaptureService：详情抓取管线（不含 M10 revision 发布）。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use crawler_capture::{
    ArticleFetchClient, ArticleIdentityResolver, AssetDownloader, BrowserRuntime, ByteFetcher,
    CaptureRequest, FetchedAsset, IdentityInput, ImageClassifier, ImageDecision, ImageDecisionKind,
    ImageThresholds, ObjectStore, PageFetcher, ReqwestByteFetcher, canonicalize_url,
};
use job_pipeline::kinds::idempotency;
use job_pipeline::{
    CreateJob, JobRepository, KIND_FETCH_ARTICLE_DETAIL, KIND_FETCH_SOURCE, KIND_HISTORY_BACKFILL,
    KIND_RETRY_ARTICLE_ASSETS,
};
use platform_connectors::{
    ArticleCandidateEvidence, ConnectorRegistry, DiscoverRequest, SourceRef,
};
use serde::{Deserialize, Serialize};
use server_core::Clock;
use uuid::Uuid;

use crate::adapters::secret_store::SecretStore;
use crate::domain::article::{Article, ArticleAsset, ArticleImageDecision, ArticleStatus};
use crate::domain::daily_plan::DailyPlanStatus;
use crate::domain::source::{HeartbeatOutcome, SourceLevel};
use crate::repository::article::{ArticleRepository, CapturedArticleCommit, UpsertArticleLink};
use crate::repository::collector::CollectorRepository;
use crate::repository::daily_plan::DailyPlanRepository;
use crate::repository::source::SourceRepository;
use crate::service::article_capture_contract::{
    candidate_to_detail_payload, ensure_not_cancelled, hex_sha256, stable_uuid,
};
use crate::service::capture_failure::CaptureFailure;
use crate::service::connector_context::CollectorConnectorResolver;
use crate::service::content_normalizer::ContentNormalizerService;
use crate::service::egress_resolver::{EgressResolver, ResolveRequest, ResolvedEgress};
use crate::service::heartbeat::{ApplyOutcomeCommand, HeartbeatService};
use crate::service::markdown_builder::MarkdownBuilder;
use crate::service::plan_eligibility::should_skip_at_runtime;
use crate::service::request_limiter::{RequestLimitContext, RequestPurpose, RequestRateLimiter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchArticleDetailPayload {
    pub source_account_id: Uuid,
    #[serde(default)]
    pub collector_account_id: Option<Uuid>,
    pub canonical_url: String,
    pub wechat_biz: Option<String>,
    pub wechat_mid: Option<String>,
    pub wechat_idx: Option<String>,
    pub wechat_sn: Option<String>,
    pub title_hint: Option<String>,
    /// 发现列表提供的元数据；详情页面的可信值可覆盖这些回退值。
    #[serde(default)]
    pub candidate_published_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub candidate_author: Option<String>,
    #[serde(default)]
    pub candidate_cover_url: Option<String>,
    #[serde(default)]
    pub candidate_evidence: Option<ArticleCandidateEvidence>,
    /// 仅由日常 source fetch 传入。详情真正落库后才可把心跳记为发现新文章。
    #[serde(default)]
    pub source_heartbeat_event_id: Option<String>,
    #[serde(default)]
    pub heartbeat_manual: bool,
    #[serde(default)]
    pub heartbeat_recalculate_next: bool,
    pub discovered_via: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchSourcePayload {
    pub source_account_id: Uuid,
    pub source_identity: String,
    pub heartbeat_event_id: String,
    pub collector_account_id: Uuid,
    pub connector: String,
    pub limit: u8,
    /// 手动立即抓取标记。
    #[serde(default)]
    pub manual: bool,
    /// 手动抓取是否重算 next_heartbeat_at。
    #[serde(default)]
    pub recalculate_next: bool,
    /// 关联的日计划行（自动计划创建的 job 携带）。
    #[serde(default)]
    pub daily_plan_id: Option<Uuid>,
}

/// 历史回填按页执行；每页完成后创建带新 cursor 的下一页 Job。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryBackfillPayload {
    pub source_account_id: Uuid,
    pub source_identity: String,
    pub collector_account_id: Uuid,
    pub connector: String,
    pub run_id: String,
    #[serde(default)]
    pub cursor: u32,
    pub page_size: u8,
    pub remaining_pages: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureOutcome {
    pub article_id: Uuid,
    pub identity_key: String,
    pub created: bool,
    pub kept_assets: usize,
    pub decisions: usize,
    pub asset_warnings: usize,
    pub asset_retry_job_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryArticleAssetsPayload {
    pub detail: FetchArticleDetailPayload,
    pub failure_set_hash: String,
}

pub struct ArticleCaptureService {
    articles: Arc<dyn ArticleRepository>,
    jobs: Arc<dyn JobRepository>,
    fetch: ArticleFetchClient,
    classifier: ImageClassifier,
    downloader: AssetDownloader,
    object_store: Arc<dyn ObjectStore>,
    connectors: Arc<ConnectorRegistry>,
    clock: Arc<dyn Clock>,
    identity: ArticleIdentityResolver,
    heartbeat: Option<Arc<HeartbeatService>>,
    sources: Option<Arc<dyn SourceRepository>>,
    plans: Option<Arc<dyn DailyPlanRepository>>,
    normalizer: Option<Arc<ContentNormalizerService>>,
    connector_resolver: Option<Arc<CollectorConnectorResolver>>,
    egress_resolver: Option<Arc<EgressResolver>>,
    request_limiter: Arc<RequestRateLimiter>,
}

impl ArticleCaptureService {
    pub fn new(
        articles: Arc<dyn ArticleRepository>,
        jobs: Arc<dyn JobRepository>,
        connectors: Arc<ConnectorRegistry>,
        clock: Arc<dyn Clock>,
        object_store: Arc<dyn ObjectStore>,
    ) -> Self {
        Self {
            articles,
            jobs,
            fetch: ArticleFetchClient::default(),
            classifier: ImageClassifier::new(ImageThresholds::default()),
            downloader: AssetDownloader::new(
                object_store.clone(),
                Arc::new(ReqwestByteFetcher::default()),
            ),
            object_store,
            connectors,
            clock,
            identity: ArticleIdentityResolver::new(),
            heartbeat: None,
            sources: None,
            plans: None,
            normalizer: None,
            connector_resolver: None,
            egress_resolver: None,
            request_limiter: Arc::new(RequestRateLimiter::default()),
        }
    }

    pub fn with_collector_credentials(
        mut self,
        collectors: Arc<dyn CollectorRepository>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        self.connector_resolver = Some(Arc::new(CollectorConnectorResolver::new(
            collectors,
            secrets,
            self.connectors.clone(),
            self.clock.clone(),
        )));
        self
    }

    pub fn with_connector_resolver(mut self, resolver: Arc<CollectorConnectorResolver>) -> Self {
        self.connector_resolver = Some(resolver);
        self
    }

    pub fn with_heartbeat(mut self, heartbeat: Arc<HeartbeatService>) -> Self {
        self.heartbeat = Some(heartbeat);
        self
    }

    pub fn with_normalizer(mut self, normalizer: Arc<ContentNormalizerService>) -> Self {
        self.normalizer = Some(normalizer);
        self
    }

    pub fn with_sources(mut self, sources: Arc<dyn SourceRepository>) -> Self {
        self.sources = Some(sources);
        self
    }

    pub fn with_plans(mut self, plans: Arc<dyn DailyPlanRepository>) -> Self {
        self.plans = Some(plans);
        self
    }

    pub fn with_browser(mut self, browser: Arc<dyn BrowserRuntime>) -> Self {
        self.fetch = self.fetch.with_browser(browser);
        self
    }

    pub fn with_page_fetcher(mut self, fetcher: Arc<dyn PageFetcher>) -> Self {
        self.fetch = self.fetch.with_page_fetcher(fetcher);
        self
    }

    pub fn with_asset_fetcher(mut self, fetcher: Arc<dyn ByteFetcher>) -> Self {
        self.downloader = AssetDownloader::new(self.object_store.clone(), fetcher);
        self
    }

    pub fn with_egress_resolver(mut self, resolver: Arc<EgressResolver>) -> Self {
        self.egress_resolver = Some(resolver);
        self
    }

    pub fn with_request_limiter(mut self, limiter: Arc<RequestRateLimiter>) -> Self {
        self.request_limiter = limiter;
        self
    }

    fn enforce_request_limit(
        &self,
        source_id: Uuid,
        collector_id: Option<Uuid>,
        egress_id: Option<Uuid>,
        url: &str,
        purpose: RequestPurpose,
    ) -> Result<(), String> {
        let host = url::Url::parse(url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        self.request_limiter
            .acquire(
                &RequestLimitContext {
                    collector_id,
                    egress_id,
                    source_id,
                    host,
                    purpose,
                },
                DateTime::<Utc>::from(self.clock.now()),
            )
            .map_err(|error| format!("{}: retry_after_ms={}", error.code, error.retry_after_ms))
    }

    async fn resolve_dynamic_egress(
        &self,
        purpose: crate::domain::egress::EgressPurpose,
        now: DateTime<Utc>,
    ) -> Result<(Option<String>, Option<Uuid>), String> {
        let resolver = self
            .egress_resolver
            .as_ref()
            .ok_or_else(|| "egress.resolver_not_configured: 详情抓取缺少出口解析器".to_string())?;
        let resolved = resolver
            .resolve(
                ResolveRequest {
                    purpose,
                    preferred_profile_id: None,
                    bound_fixed_egress_id: None,
                },
                now,
            )
            .await
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        match resolved {
            ResolvedEgress::LocalDirect { .. } => Ok((None, None)),
            ResolvedEgress::DynamicLease { lease } => Ok((Some(lease.proxy_url), Some(lease.id))),
            ResolvedEgress::FixedBinding { .. } => {
                Err("egress.purpose_mismatch: 详情和资产下载禁止使用固定出口".to_string())
            }
        }
    }

    async fn release_egress_lease(&self, lease_id: Option<Uuid>) {
        if let (Some(resolver), Some(lease_id)) = (&self.egress_resolver, lease_id) {
            let _ = resolver.leases().release(lease_id).await;
        }
    }

    /// Job 开始前二次读 source：暂停 / L4 → 应跳过。
    async fn runtime_skip_reason(
        &self,
        source_account_id: Uuid,
    ) -> Result<Option<&'static str>, String> {
        let Some(sources) = &self.sources else {
            return Ok(None);
        };
        let account = sources
            .get_source(source_account_id)
            .await?
            .ok_or_else(|| "source.not_found: 监测公众号不存在".to_string())?;
        let rules = sources
            .get_fetch_rules(source_account_id)
            .await?
            .ok_or_else(|| "source.fetch_rules_not_found: 抓取规则不存在".to_string())?;
        let now = DateTime::<Utc>::from(self.clock.now());
        let level = if let Some(until) = rules.promotion_until {
            if until > now && matches!(rules.base_level, SourceLevel::L2 | SourceLevel::L3) {
                SourceLevel::L1
            } else {
                rules.base_level
            }
        } else {
            rules.base_level
        };
        Ok(should_skip_at_runtime(account.status, level))
    }

    /// 解析 identity 并构造幂等键 `detail:<article-identity>`。
    pub fn detail_idempotency_key(&self, payload: &FetchArticleDetailPayload) -> String {
        let id = self.identity.resolve(IdentityInput {
            source_account_id: payload.source_account_id,
            canonical_url: &payload.canonical_url,
            wechat_biz: payload.wechat_biz.as_deref(),
            wechat_mid: payload.wechat_mid.as_deref(),
            wechat_idx: payload.wechat_idx.as_deref(),
        });
        idempotency::detail(&id.key())
    }

    pub async fn enqueue_detail(
        &self,
        payload: FetchArticleDetailPayload,
    ) -> Result<uuid::Uuid, String> {
        let key = self.detail_idempotency_key(&payload);
        let job = self
            .jobs
            .create_job(CreateJob::new(
                KIND_FETCH_ARTICLE_DETAIL,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(*job.id.as_uuid())
    }

    /// 执行详情抓取：fetch → extract → classify → download → 落库。
    pub async fn capture_detail(
        &self,
        payload: &FetchArticleDetailPayload,
    ) -> Result<CaptureOutcome, CaptureFailure> {
        self.capture_detail_with_cancel(payload, None).await
    }

    pub async fn capture_detail_with_cancel(
        &self,
        payload: &FetchArticleDetailPayload,
        cancel_requested: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<CaptureOutcome, CaptureFailure> {
        self.capture_detail_internal(payload, cancel_requested, true)
            .await
            .map_err(CaptureFailure::from)
    }

    pub async fn retry_article_assets(
        &self,
        payload: &RetryArticleAssetsPayload,
        cancel_requested: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<CaptureOutcome, CaptureFailure> {
        self.capture_detail_internal(&payload.detail, cancel_requested, false)
            .await
            .map_err(CaptureFailure::from)
    }

    async fn capture_detail_internal(
        &self,
        payload: &FetchArticleDetailPayload,
        cancel_requested: Option<&std::sync::atomic::AtomicBool>,
        schedule_asset_retry: bool,
    ) -> Result<CaptureOutcome, String> {
        ensure_not_cancelled(cancel_requested)?;
        let now = DateTime::<Utc>::from(self.clock.now());
        self.articles
            .ensure_source_stub(payload.source_account_id, "stub-source")
            .await?;

        let identity = self.identity.resolve(IdentityInput {
            source_account_id: payload.source_account_id,
            canonical_url: &payload.canonical_url,
            wechat_biz: payload.wechat_biz.as_deref(),
            wechat_mid: payload.wechat_mid.as_deref(),
            wechat_idx: payload.wechat_idx.as_deref(),
        });
        let identity_key = identity.key();
        let canonical = canonicalize_url(&payload.canonical_url);

        let (biz, mid, idx) = match identity.wechat_parts() {
            Some((b, m, i)) => (
                Some(b.to_string()),
                Some(m.to_string()),
                Some(i.to_string()),
            ),
            None => (
                payload.wechat_biz.clone(),
                payload.wechat_mid.clone(),
                payload.wechat_idx.clone(),
            ),
        };

        let link_result = self
            .articles
            .upsert_article_link(UpsertArticleLink {
                source_account_id: payload.source_account_id,
                canonical_url: canonical.clone(),
                wechat_biz: biz.clone(),
                wechat_mid: mid.clone(),
                wechat_idx: idx.clone(),
                wechat_sn: payload.wechat_sn.clone(),
                title_hint: payload.title_hint.clone(),
                discovered_via: payload.discovered_via.clone(),
                discovered_at: now,
            })
            .await?;

        let (detail_proxy, detail_lease) = self
            .resolve_dynamic_egress(crate::domain::egress::EgressPurpose::ArticleDetail, now)
            .await?;
        if let Err(error) = self.enforce_request_limit(
            payload.source_account_id,
            payload.collector_account_id,
            detail_lease,
            &canonical,
            RequestPurpose::Detail,
        ) {
            self.release_egress_lease(detail_lease).await;
            return Err(error);
        }
        let captured_result = self
            .fetch
            .fetch_and_extract(&CaptureRequest {
                url: canonical.clone(),
                need_assets: true,
                platform_hint: None,
                proxy_url: detail_proxy,
            })
            .await;
        self.release_egress_lease(detail_lease).await;
        let captured = captured_result.map_err(|e| format!("{}: {}", e.code, e.message))?;
        ensure_not_cancelled(cancel_requested)?;

        let (asset_proxy, asset_lease) = self
            .resolve_dynamic_egress(crate::domain::egress::EgressPurpose::AssetDownload, now)
            .await?;
        let mut fetched_by_url: HashMap<String, FetchedAsset> = HashMap::new();
        let mut measured = Vec::new();
        let mut rejected_downloads: HashMap<String, ImageDecision> = HashMap::new();
        for candidate in &captured.candidates {
            ensure_not_cancelled(cancel_requested)?;
            if let Err(error) = self.enforce_request_limit(
                payload.source_account_id,
                payload.collector_account_id,
                asset_lease,
                &candidate.url,
                RequestPurpose::Media,
            ) {
                rejected_downloads.insert(
                    candidate.url.clone(),
                    ImageDecision {
                        candidate_url: candidate.url.clone(),
                        decision: ImageDecisionKind::DownloadFailed,
                        measured_width: None,
                        measured_height: None,
                        reason_json: serde_json::json!({
                            "reason": "local_rate_limited",
                            "error_code": error.split_once(':').map_or(error.as_str(), |(code, _)| code),
                            "retryable": true,
                        }),
                    },
                );
                continue;
            }
            match self
                .downloader
                .fetch(candidate.url.clone(), asset_proxy.clone())
                .await
            {
                Ok(fetched) => {
                    measured.push((candidate.url.clone(), fetched.measured()));
                    fetched_by_url.insert(candidate.url.clone(), fetched);
                }
                Err(error) if error.code == "image.filtered" => {
                    rejected_downloads.insert(
                        candidate.url.clone(),
                        ImageDecision {
                            candidate_url: candidate.url.clone(),
                            decision: ImageDecisionKind::Unsupported,
                            measured_width: None,
                            measured_height: None,
                            reason_json: serde_json::json!({
                                "reason": "unsupported_download",
                                "error_code": error.code,
                            }),
                        },
                    );
                }
                Err(error) => {
                    // 单个资源失败不应回滚已成功抓取的正文。保留可审计的
                    // 决策，后续可由重抓/资源恢复任务根据原 URL 补齐。
                    rejected_downloads.insert(
                        candidate.url.clone(),
                        ImageDecision {
                            candidate_url: candidate.url.clone(),
                            decision: ImageDecisionKind::DownloadFailed,
                            measured_width: None,
                            measured_height: None,
                            reason_json: serde_json::json!({
                                "reason": "download_failed",
                                "error_code": error.code,
                                "retryable": error.retryable,
                            }),
                        },
                    );
                }
            }
        }
        let cover_url = payload.candidate_cover_url.as_deref().map(canonicalize_url);
        let mut cover_fetched = None;
        let mut cover_failure = None;
        if let Some(url) = cover_url.as_deref() {
            if let Some(existing) = fetched_by_url.get(url) {
                cover_fetched = Some(existing.clone());
            } else {
                let limited = self.enforce_request_limit(
                    payload.source_account_id,
                    payload.collector_account_id,
                    asset_lease,
                    url,
                    RequestPurpose::Media,
                );
                match limited {
                    Err(error) => {
                        cover_failure = Some((
                            error
                                .split_once(':')
                                .map_or(error.as_str(), |(code, _)| code)
                                .to_string(),
                            true,
                        ));
                    }
                    Ok(()) => match self
                        .downloader
                        .fetch(url.to_string(), asset_proxy.clone())
                        .await
                    {
                        Ok(fetched) => cover_fetched = Some(fetched),
                        Err(error) => {
                            cover_failure = Some((error.code, error.retryable));
                        }
                    },
                }
            }
        }
        self.release_egress_lease(asset_lease).await;
        let mut decisions = self.classifier.classify(&captured.candidates, &measured);
        for decision in &mut decisions {
            if let Some(rejected) = rejected_downloads.remove(&decision.candidate_url) {
                *decision = rejected;
            }
        }

        let article_id = Uuid::new_v4();
        let mut staged_assets = Vec::new();
        let mut url_to_asset: HashMap<String, Uuid> = HashMap::new();
        let mut cover_url_to_asset: HashMap<String, Uuid> = HashMap::new();
        let mut store_failures: HashMap<String, (String, bool)> = HashMap::new();
        for decision in decisions
            .iter()
            .filter(|decision| decision.decision == ImageDecisionKind::Kept)
        {
            ensure_not_cancelled(cancel_requested)?;
            let fetched = fetched_by_url
                .get(&decision.candidate_url)
                .ok_or_else(|| "crawler.asset_missing: 保留图片缺少已下载对象".to_string())?;
            let stored = match self
                .downloader
                .store_fetched(&format!("articles/{article_id}/body"), fetched)
                .await
            {
                Ok(stored) => stored,
                Err(error) => {
                    store_failures.insert(
                        decision.candidate_url.clone(),
                        (error.code, error.retryable),
                    );
                    continue;
                }
            };
            let asset_id = stable_uuid(&format!("{identity_key}:body:{}", decision.candidate_url));
            staged_assets.push(ArticleAsset {
                id: asset_id,
                article_id,
                original_url: Some(decision.candidate_url.clone()),
                storage_key: stored.storage_key,
                asset_hash: Some(stored.content_hash),
                width: stored.width.map(|value| value as i32),
                height: stored.height.map(|value| value as i32),
                byte_size: Some(stored.byte_size as i64),
                mime_type: stored.mime_type,
                asset_kind: "body_image".into(),
                created_at: now,
            });
            url_to_asset.insert(decision.candidate_url.clone(), asset_id);
        }
        for decision in &mut decisions {
            if let Some((error_code, retryable)) = store_failures.remove(&decision.candidate_url) {
                decision.decision = ImageDecisionKind::DownloadFailed;
                decision.reason_json = serde_json::json!({
                    "reason": "store_failed",
                    "error_code": error_code,
                    "retryable": retryable,
                });
            }
        }
        if let (Some(url), Some(fetched)) = (cover_url.as_deref(), cover_fetched.as_ref()) {
            match self
                .downloader
                .store_fetched(&format!("articles/{article_id}/cover"), fetched)
                .await
            {
                Ok(stored) => {
                    let asset_id = stable_uuid(&format!("{identity_key}:cover:{url}"));
                    staged_assets.push(ArticleAsset {
                        id: asset_id,
                        article_id,
                        original_url: Some(url.to_string()),
                        storage_key: stored.storage_key,
                        asset_hash: Some(stored.content_hash),
                        width: stored.width.map(|value| value as i32),
                        height: stored.height.map(|value| value as i32),
                        byte_size: Some(stored.byte_size as i64),
                        mime_type: stored.mime_type,
                        asset_kind: "cover_image".into(),
                        created_at: now,
                    });
                    cover_url_to_asset.insert(url.to_string(), asset_id);
                    if !decisions
                        .iter()
                        .any(|decision| decision.candidate_url == url)
                    {
                        decisions.push(ImageDecision {
                            candidate_url: url.to_string(),
                            decision: ImageDecisionKind::Kept,
                            measured_width: Some(fetched.width),
                            measured_height: Some(fetched.height),
                            reason_json: serde_json::json!({ "reason": "candidate_cover" }),
                        });
                    }
                }
                Err(error) => cover_failure = Some((error.code, error.retryable)),
            }
        }
        if let (Some(url), Some((error_code, retryable))) = (cover_url, cover_failure)
            && !decisions
                .iter()
                .any(|decision| decision.candidate_url == url)
        {
            decisions.push(ImageDecision {
                candidate_url: url,
                decision: ImageDecisionKind::DownloadFailed,
                measured_width: None,
                measured_height: None,
                reason_json: serde_json::json!({
                    "reason": "cover_download_failed",
                    "error_code": error_code,
                    "retryable": retryable,
                }),
            });
        }
        if let Some(raw) = captured.raw_html.as_deref() {
            ensure_not_cancelled(cancel_requested)?;
            let stored = self
                .downloader
                .store_raw_html(&format!("articles/{article_id}"), raw)
                .await
                .map_err(|e| format!("{}: {}", e.code, e.message))?;
            staged_assets.push(ArticleAsset {
                id: stable_uuid(&format!("{identity_key}:raw_html")),
                article_id,
                original_url: Some(canonical.clone()),
                storage_key: stored.storage_key,
                asset_hash: Some(stored.content_hash),
                width: None,
                height: None,
                byte_size: Some(stored.byte_size as i64),
                mime_type: stored.mime_type,
                asset_kind: "raw_html".into(),
                created_at: now,
            });
        }
        if !captured.content_html.trim().is_empty()
            && let Ok(stored) = self
                .downloader
                .store_bytes(
                    &format!("articles/{article_id}/sanitized"),
                    captured.content_html.as_bytes(),
                    Some("text/html; charset=utf-8"),
                )
                .await
        {
            staged_assets.push(ArticleAsset {
                id: stable_uuid(&format!("{identity_key}:sanitized_html")),
                article_id,
                original_url: Some(canonical.clone()),
                storage_key: stored.storage_key,
                asset_hash: Some(stored.content_hash),
                width: None,
                height: None,
                byte_size: Some(stored.byte_size as i64),
                mime_type: stored.mime_type,
                asset_kind: "sanitized_html".into(),
                created_at: now,
            });
        }
        if payload.candidate_evidence.is_some()
            || payload.candidate_cover_url.is_some()
            || payload.candidate_author.is_some()
            || payload.candidate_published_at.is_some()
        {
            let evidence = serde_json::json!({
                "connector_evidence": payload.candidate_evidence,
                "author": payload.candidate_author,
                "published_at": payload.candidate_published_at,
                "cover_url": payload.candidate_cover_url.as_deref().map(canonicalize_url),
            });
            let bytes = serde_json::to_vec(&evidence).map_err(|e| e.to_string())?;
            if let Ok(stored) = self
                .downloader
                .store_bytes(
                    &format!("articles/{article_id}/candidate-evidence"),
                    &bytes,
                    Some("application/json"),
                )
                .await
            {
                staged_assets.push(ArticleAsset {
                    id: stable_uuid(&format!("{identity_key}:candidate_evidence")),
                    article_id,
                    original_url: None,
                    storage_key: stored.storage_key,
                    asset_hash: Some(stored.content_hash),
                    width: None,
                    height: None,
                    byte_size: Some(stored.byte_size as i64),
                    mime_type: stored.mime_type,
                    asset_kind: "candidate_evidence".into(),
                    created_at: now,
                });
            }
        }

        let body_asset_tuples: Vec<_> = staged_assets
            .iter()
            .filter(|asset| asset.asset_kind == "body_image")
            .map(|asset| {
                (
                    asset.id,
                    asset.storage_key.clone(),
                    asset.original_url.clone(),
                )
            })
            .collect();
        let markdown = MarkdownBuilder::new();
        let normalized_content = markdown
            .render(&markdown.from_captured_markdown(&captured.content_md, &body_asset_tuples));
        let mut body_hashes: Vec<String> = staged_assets
            .iter()
            .filter(|asset| asset.asset_kind == "body_image")
            .filter_map(|asset| asset.asset_hash.clone())
            .collect();
        body_hashes.sort();
        let asset_manifest_hash = Some(hex_sha256(body_hashes.join("\n").as_bytes()));
        let content_hash = hex_sha256(normalized_content.as_bytes());
        let article = Article {
            id: article_id,
            source_account_id: payload.source_account_id,
            article_link_id: Some(link_result.link.id),
            original_url: payload.canonical_url.clone(),
            canonical_url: canonical.clone(),
            original_title: if captured.title.is_empty() {
                payload
                    .title_hint
                    .clone()
                    .unwrap_or_else(|| "untitled".into())
            } else {
                captured.title.clone()
            },
            author: captured.author.clone().or(payload.candidate_author.clone()),
            published_at: captured
                .published_at
                .as_deref()
                .and_then(parse_published_at)
                .or(payload.candidate_published_at),
            fetched_at: now,
            current_revision: 0,
            content_md: normalized_content,
            content_hash,
            asset_manifest_hash,
            status: if body_asset_tuples.is_empty() {
                ArticleStatus::Ready
            } else {
                ArticleStatus::Captured
            },
            withdrawn_at: None,
        };
        let mut staged_decisions = Vec::with_capacity(decisions.len());
        for d in &decisions {
            let asset_id = if d.decision == ImageDecisionKind::Kept {
                url_to_asset
                    .get(&d.candidate_url)
                    .or_else(|| cover_url_to_asset.get(&d.candidate_url))
                    .copied()
            } else {
                None
            };
            staged_decisions.push(ArticleImageDecision {
                id: stable_uuid(&format!("{identity_key}:decision:{}", d.candidate_url)),
                article_id,
                candidate_url: d.candidate_url.clone(),
                decision: d.decision.as_str().to_string(),
                measured_width: d.measured_width.map(|value| value as i32),
                measured_height: d.measured_height.map(|value| value as i32),
                reason_json: d.reason_json.clone(),
                asset_id,
                created_at: now,
            });
        }

        ensure_not_cancelled(cancel_requested)?;
        let commit = self
            .articles
            .commit_captured_article(CapturedArticleCommit {
                article,
                assets: staged_assets,
                image_decisions: staged_decisions,
                change_reason: "crawler_capture".into(),
                now,
            })
            .await?;

        // 原子提交后再规划 OCR；内容和资产未变化时不重复创建 OCR 任务。
        if commit.changed
            && let Some(ref normalizer) = self.normalizer
        {
            let _ = normalizer.normalize_captured(commit.article.id).await?;
        }

        let mut failed_urls = decisions
            .iter()
            .filter(|decision| decision.decision == ImageDecisionKind::DownloadFailed)
            .map(|decision| decision.candidate_url.clone())
            .collect::<Vec<_>>();
        failed_urls.sort();
        failed_urls.dedup();
        let asset_warnings = failed_urls.len();
        let asset_retry_job_id = if schedule_asset_retry && !failed_urls.is_empty() {
            let failure_set_hash = hex_sha256(failed_urls.join("\n").as_bytes());
            let retry_payload = RetryArticleAssetsPayload {
                detail: payload.clone(),
                failure_set_hash: failure_set_hash.clone(),
            };
            let mut job = CreateJob::new(
                KIND_RETRY_ARTICLE_ASSETS,
                idempotency::asset_retry(&identity_key, &failure_set_hash),
                serde_json::to_value(&retry_payload).map_err(|e| e.to_string())?,
            );
            job.max_attempts = Some(3);
            Some(
                *self
                    .jobs
                    .create_job(job)
                    .await
                    .map_err(|e| e.to_string())?
                    .id
                    .as_uuid(),
            )
        } else {
            None
        };

        Ok(CaptureOutcome {
            article_id: commit.article.id,
            identity_key,
            created: commit.created,
            kept_assets: body_asset_tuples.len(),
            decisions: decisions.len(),
            asset_warnings,
            asset_retry_job_id,
        })
    }

    /// fetch_source：开始前二次读 source 状态；暂停/L4 → 安全 skipped。
    pub async fn fetch_source(
        &self,
        payload: &FetchSourcePayload,
    ) -> Result<serde_json::Value, CaptureFailure> {
        if let Some(skip_reason) = self.runtime_skip_reason(payload.source_account_id).await? {
            if let (Some(plans), Some(plan_id)) = (&self.plans, payload.daily_plan_id) {
                let now = DateTime::<Utc>::from(self.clock.now());
                let _ = plans
                    .update_status(
                        plan_id,
                        DailyPlanStatus::Skipped,
                        Some(skip_reason.to_string()),
                        None,
                        now,
                    )
                    .await;
            }
            return Ok(serde_json::json!({
                "ok": true,
                "skipped": true,
                "reason_code": skip_reason,
                "daily_plan_id": payload.daily_plan_id,
            }));
        }

        let connector_name = if payload.connector.trim().is_empty() {
            return Err("connector.required: fetch_source 必须携带已选择的 connector".into());
        } else {
            payload.connector.as_str()
        };
        let backend = self
            .connectors
            .get(connector_name)
            .ok_or_else(|| format!("connector.unsupported: {connector_name}"))?;

        let resolver = self
            .connector_resolver
            .as_ref()
            .ok_or_else(|| "connector.resolver_not_configured: 拒绝使用临时采集账号".to_string())?;
        let selected = resolver
            .resolve(
                platform_connectors::ConnectorCapability::DiscoverRecent,
                Some(payload.collector_account_id),
                Some(connector_name.to_string()),
            )
            .await
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        let account = selected.account;
        let credentials_json = selected.credentials_json;

        self.enforce_request_limit(
            payload.source_account_id,
            Some(payload.collector_account_id),
            None,
            "https://mp.weixin.qq.com/cgi-bin/appmsgpublish",
            RequestPurpose::List,
        )?;

        let candidates = backend
            .discover_recent(DiscoverRequest {
                source: SourceRef {
                    id: Some(payload.source_account_id),
                    identity: payload.source_identity.clone(),
                    level_hint: None,
                },
                account,
                limit: payload.limit.max(1),
                cursor: 0,
                page_budget: 3,
                credentials_json,
            })
            .await
            .map_err(|e| format!("{}: {}", e.reason_code, e.message))?;

        self.articles
            .ensure_source_stub(payload.source_account_id, &payload.source_identity)
            .await?;

        let mut detail_job_ids = Vec::new();
        let mut seen = HashSet::new();
        let mut skipped_existing = 0usize;
        for c in &candidates {
            let detail_payload = candidate_to_detail_payload(payload, c);
            let identity_key = self.detail_idempotency_key(&detail_payload);
            if !seen.insert(identity_key) {
                continue;
            }
            if self
                .articles
                .find_article_by_identity(
                    detail_payload.source_account_id,
                    detail_payload.wechat_biz.as_deref(),
                    detail_payload.wechat_mid.as_deref(),
                    detail_payload.wechat_idx.as_deref(),
                    &detail_payload.canonical_url,
                )
                .await?
                .is_some()
            {
                skipped_existing += 1;
                continue;
            }
            let job_id = self.enqueue_detail(detail_payload).await?;
            detail_job_ids.push(job_id.to_string());
        }

        // 有待抓取候选时，不能把“发现链接”误记成新文章；详情任务成功
        // 落库后会以同一个 event id 写 NewArticle。全部都已存在才是空抓。
        let heartbeat_result = if detail_job_ids.is_empty() {
            self.apply_fetch_heartbeat(payload, false, None).await?
        } else {
            None
        };

        let source_key = idempotency::source_fetch(
            &payload.source_account_id.to_string(),
            &payload.heartbeat_event_id,
        );
        Ok(serde_json::json!({
            "ok": true,
            "source_idempotency_key": source_key,
            "discovered": candidates.len(),
            "skipped_existing": skipped_existing,
            "detail_job_ids": detail_job_ids,
            "heartbeat_deferred": !detail_job_ids.is_empty(),
            "heartbeat": heartbeat_result,
        }))
    }

    /// 详情成功提交后才允许日常心跳记为发现新文章。相同 event id 的多个
    /// 详情任务由 HeartbeatService 的幂等约束收敛为一次事实。
    pub async fn apply_detail_success_heartbeat(
        &self,
        payload: &FetchArticleDetailPayload,
    ) -> Result<Option<serde_json::Value>, CaptureFailure> {
        let Some(event_key) = payload.source_heartbeat_event_id.as_deref() else {
            return Ok(None);
        };
        let Some(hb) = &self.heartbeat else {
            return Ok(None);
        };
        let result = hb
            .apply_outcome(ApplyOutcomeCommand {
                source_account_id: payload.source_account_id,
                event_key: event_key.to_string(),
                outcome: if payload.heartbeat_manual {
                    HeartbeatOutcome::Manual
                } else {
                    HeartbeatOutcome::NewArticle
                },
                recalculate_next: if payload.heartbeat_manual {
                    payload.heartbeat_recalculate_next
                } else {
                    true
                },
                reset_empty_on_manual_new: true,
                manual_found_new: payload.heartbeat_manual,
            })
            .await?;
        Ok(Some(serde_json::json!({
            "event_id": result.event.id,
            "reason_code": result.event.reason_code,
        })))
    }

    /// 抓取完成/失败后把 outcome 交给 HeartbeatService（唯一写 next/等级入口）。
    pub async fn apply_fetch_heartbeat(
        &self,
        payload: &FetchSourcePayload,
        found_new: bool,
        failure: Option<HeartbeatOutcome>,
    ) -> Result<Option<serde_json::Value>, String> {
        let Some(hb) = &self.heartbeat else {
            return Ok(None);
        };

        let (outcome, manual_found_new) = match failure {
            Some(HeartbeatOutcome::TechnicalFailure) => (HeartbeatOutcome::TechnicalFailure, false),
            Some(HeartbeatOutcome::AccessBlocked) => (HeartbeatOutcome::AccessBlocked, false),
            Some(other) => (other, found_new),
            None if payload.manual => (HeartbeatOutcome::Manual, found_new),
            None if found_new => (HeartbeatOutcome::NewArticle, false),
            None => (HeartbeatOutcome::EmptySuccess, false),
        };

        let result = hb
            .apply_outcome(ApplyOutcomeCommand {
                source_account_id: payload.source_account_id,
                event_key: payload.heartbeat_event_id.clone(),
                outcome,
                recalculate_next: if payload.manual {
                    payload.recalculate_next
                } else {
                    true
                },
                reset_empty_on_manual_new: true,
                manual_found_new,
            })
            .await?;

        Ok(Some(serde_json::json!({
            "event_id": result.event.id,
            "reason_code": result.event.reason_code,
            "level_before": result.event.level_before.as_str(),
            "level_after": result.event.level_after.as_str(),
            "empty_count_after": result.event.empty_count_after,
            "next_heartbeat_at": result.event.next_heartbeat_at,
            "source_status": result.source_status.map(|s| s.as_str()),
        })))
    }

    pub async fn enqueue_fetch_source(
        &self,
        payload: FetchSourcePayload,
    ) -> Result<uuid::Uuid, String> {
        let key = idempotency::source_fetch(
            &payload.source_account_id.to_string(),
            &payload.heartbeat_event_id,
        );
        let job = self
            .jobs
            .create_job(CreateJob::new(
                KIND_FETCH_SOURCE,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(*job.id.as_uuid())
    }

    pub async fn enqueue_history_backfill(
        &self,
        mut payload: HistoryBackfillPayload,
    ) -> Result<uuid::Uuid, String> {
        payload.page_size = payload.page_size.clamp(1, 10);
        payload.remaining_pages = payload.remaining_pages.clamp(1, 100);
        let key = idempotency::history_backfill(&payload.run_id, payload.cursor);
        let job = self
            .jobs
            .create_job(CreateJob::new(
                KIND_HISTORY_BACKFILL,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())?;
        Ok(*job.id.as_uuid())
    }

    /// 每个回填 Job 只读取一页；成功后才创建下一页，当前 cursor 即持久 checkpoint。
    pub async fn run_history_backfill_page(
        &self,
        payload: &HistoryBackfillPayload,
        cancel_requested: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<serde_json::Value, CaptureFailure> {
        ensure_not_cancelled(cancel_requested)?;
        let backend = self
            .connectors
            .get(&payload.connector)
            .ok_or_else(|| format!("connector.unsupported: {}", payload.connector))?;
        let resolver = self
            .connector_resolver
            .as_ref()
            .ok_or_else(|| "connector.resolver_not_configured: 拒绝使用临时采集账号".to_string())?;
        let selected = resolver
            .resolve(
                platform_connectors::ConnectorCapability::DiscoverRecent,
                Some(payload.collector_account_id),
                Some(payload.connector.clone()),
            )
            .await
            .map_err(|e| format!("{}: {}", e.code, e.message))?;
        let page_size = payload.page_size.clamp(1, 10);
        self.enforce_request_limit(
            payload.source_account_id,
            Some(payload.collector_account_id),
            None,
            "https://mp.weixin.qq.com/cgi-bin/appmsgpublish",
            RequestPurpose::List,
        )?;
        let candidates = backend
            .discover_recent(DiscoverRequest {
                source: SourceRef {
                    id: Some(payload.source_account_id),
                    identity: payload.source_identity.clone(),
                    level_hint: None,
                },
                account: selected.account,
                limit: page_size,
                cursor: payload.cursor,
                page_budget: 1,
                credentials_json: selected.credentials_json,
            })
            .await
            .map_err(|e| format!("{}: {}", e.reason_code, e.message))?;

        self.articles
            .ensure_source_stub(payload.source_account_id, &payload.source_identity)
            .await?;
        let mut seen = HashSet::new();
        let mut detail_job_ids = Vec::new();
        let mut reached_known_boundary = false;
        for candidate in &candidates {
            ensure_not_cancelled(cancel_requested)?;
            let (biz, mid, idx, sn) =
                crate::service::article_capture_contract::parse_wechat_from_url(
                    &candidate.canonical_url,
                );
            let detail = FetchArticleDetailPayload {
                source_account_id: payload.source_account_id,
                collector_account_id: Some(payload.collector_account_id),
                canonical_url: candidate.canonical_url.clone(),
                wechat_biz: biz,
                wechat_mid: mid,
                wechat_idx: idx,
                wechat_sn: sn,
                title_hint: Some(candidate.title.clone()),
                candidate_published_at: candidate.published_at,
                candidate_author: candidate.author.clone(),
                candidate_cover_url: candidate.cover_url.clone(),
                candidate_evidence: candidate.evidence.clone(),
                source_heartbeat_event_id: None,
                heartbeat_manual: false,
                heartbeat_recalculate_next: false,
                discovered_via: format!("history_backfill:{}:{}", payload.run_id, payload.cursor),
            };
            if !seen.insert(self.detail_idempotency_key(&detail)) {
                continue;
            }
            if self
                .articles
                .find_article_by_identity(
                    detail.source_account_id,
                    detail.wechat_biz.as_deref(),
                    detail.wechat_mid.as_deref(),
                    detail.wechat_idx.as_deref(),
                    &detail.canonical_url,
                )
                .await?
                .is_some()
            {
                reached_known_boundary = true;
                break;
            }
            detail_job_ids.push(self.enqueue_detail(detail).await?.to_string());
        }

        ensure_not_cancelled(cancel_requested)?;
        let next_cursor = payload.cursor.saturating_add(u32::from(page_size));
        let should_continue = !reached_known_boundary
            && candidates.len() >= usize::from(page_size)
            && payload.remaining_pages > 1;
        let next_job_id = if should_continue {
            Some(
                self.enqueue_history_backfill(HistoryBackfillPayload {
                    cursor: next_cursor,
                    remaining_pages: payload.remaining_pages - 1,
                    ..payload.clone()
                })
                .await?
                .to_string(),
            )
        } else {
            None
        };

        Ok(serde_json::json!({
            "ok": true,
            "run_id": payload.run_id,
            "checkpoint_cursor": payload.cursor,
            "next_cursor": next_cursor,
            "candidate_count": candidates.len(),
            "detail_job_ids": detail_job_ids,
            "reached_known_boundary": reached_known_boundary,
            "next_job_id": next_job_id,
            "complete": !should_continue,
        }))
    }
}

fn parse_published_at(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
}
