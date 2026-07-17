//! SourceValidationService：链接解析、最多三篇首次验证、确认创建。

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use crawler_capture::{
    ArticleIdentityResolver, IdentityInput, PageFetchRequest, PageFetcher, ReqwestPageFetcher,
    canonicalize_url,
};
use job_pipeline::kinds::idempotency;
use job_pipeline::{
    CreateJob, JobRepository, JobStatus, KIND_FETCH_SOURCE, KIND_SOURCE_VALIDATION,
    KIND_SOURCE_VALIDATION_AGGREGATE,
};
use platform_connectors::{
    ArticleCandidate, ConnectorCapability, ConnectorRegistry, DiscoverRequest, SourceRef,
};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};
use serde_json::json;
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::audit::{ACTOR_SYSTEM, ACTOR_SYSTEM_ADMINISTRATOR, AuditWriter};
use crate::domain::settings::WorkingWindow;
use crate::domain::source::{
    AccountFetchRulesPublic, IdConfidence, SourceAccountIdPublic, SourceAccountPublic,
    SourceIdType, SourceLevel, SourceStatus, SourceValidationRun, ValidationCandidateResult,
    ValidationRunStatus,
};
use crate::repository::article::ArticleRepository;
use crate::repository::source::{
    CompleteValidationRun, CreateSourceAccount, CreateSourceAccountId, CreateValidationRun,
    FieldPatch, PatchSourceAggregate, SaveValidationProgress, SourceRepository, UpsertFetchRules,
};
use crate::service::article_capture::{ArticleCaptureService, FetchArticleDetailPayload};
use crate::service::article_capture_contract::stable_uuid;
use crate::service::connector_context::CollectorConnectorResolver;
use crate::service::request_limiter::{RequestLimitContext, RequestPurpose, RequestRateLimiter};
use crate::service::source_parser::{ParsedSource, SourceParser};
use crate::service::source_validation_contract::{
    map_repo, parse_wechat_from_url, patch_summary, validate_hhmm, validate_nonempty,
    validate_patch_connector,
};

const VALIDATION_TTL_HOURS: i64 = 24;
const MAX_CANDIDATES: u8 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateSourceCommand {
    pub url: String,
    /// 可选指定采集账号；R02 将完成服务端 capability 选择。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collector_account_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateSourceResponse {
    pub run: SourceValidationRun,
    pub existing_source: Option<SourceAccountPublic>,
    pub idempotency_key: String,
    pub already_exists: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceValidationJobPayload {
    pub validation_run_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceValidationAggregatePayload {
    pub validation_run_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmSourceCommand {
    pub validation_run_id: Uuid,
    pub display_name: Option<String>,
    pub base_level: String,
    pub working_windows: Vec<WorkingWindow>,
    pub first_heartbeat_at: DateTime<Utc>,
    pub preferred_connector: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub enum PatchField<T> {
    #[default]
    Missing,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PatchField<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(|value| match value {
            Some(value) => Self::Value(value),
            None => Self::Null,
        })
    }
}

impl<T> PatchField<T> {
    fn into_repo(self) -> FieldPatch<T> {
        match self {
            Self::Missing => FieldPatch::Keep,
            Self::Null => FieldPatch::Clear,
            Self::Value(value) => FieldPatch::Set(value),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSourceCommand {
    pub expected_version: i64,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub preferred_connector: PatchField<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub base_level: Option<String>,
    #[serde(default)]
    pub working_windows: Option<Vec<WorkingWindow>>,
    #[serde(default)]
    pub connector_override: PatchField<String>,
    #[serde(default)]
    pub interval_override_hours: PatchField<i32>,
    /// Optional because omission means keep the current scheduling switch.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Canonical manual schedule override. Missing keeps it, null clears it,
    /// and a timestamp sets both the manual override and next heartbeat.
    #[serde(default)]
    pub manual_next_heartbeat_at: PatchField<DateTime<Utc>>,
    /// Legacy alias accepted for older clients. If both aliases are present,
    /// they must carry the same value; the canonical field wins otherwise.
    #[serde(default)]
    pub next_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub auto_promotion_enabled: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchNowCommand {
    /// Client supplied command key. Retries must reuse this value; when it is
    /// omitted, the source version provides a stable fallback key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// 是否在手动抓取完成后重算 next_heartbeat_at（默认 false，保持原计划）。
    #[serde(default)]
    pub recalculate_next: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchNowResponse {
    pub job_id: Uuid,
    pub idempotency_key: String,
    /// 本次手动抓取是否要求重算 next（供完成回调使用）。
    pub recalculate_next: bool,
}

pub struct SourceValidationService {
    sources: Arc<dyn SourceRepository>,
    articles: Arc<dyn ArticleRepository>,
    jobs: Arc<dyn JobRepository>,
    capture: Arc<ArticleCaptureService>,
    connectors: Arc<ConnectorRegistry>,
    clock: Arc<dyn Clock>,
    parser: SourceParser,
    identity: ArticleIdentityResolver,
    audit: Option<AuditWriter>,
    connector_resolver: Option<Arc<CollectorConnectorResolver>>,
    source_link_fetcher: Arc<dyn PageFetcher>,
    request_limiter: Arc<RequestRateLimiter>,
}

impl SourceValidationService {
    pub fn new(
        sources: Arc<dyn SourceRepository>,
        articles: Arc<dyn ArticleRepository>,
        jobs: Arc<dyn JobRepository>,
        capture: Arc<ArticleCaptureService>,
        connectors: Arc<ConnectorRegistry>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            sources,
            articles,
            jobs,
            capture,
            connectors,
            clock,
            parser: SourceParser::new(),
            identity: ArticleIdentityResolver::new(),
            audit: None,
            connector_resolver: None,
            source_link_fetcher: Arc::new(ReqwestPageFetcher::default()),
            request_limiter: Arc::new(RequestRateLimiter::default()),
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_connector_resolver(mut self, resolver: Arc<CollectorConnectorResolver>) -> Self {
        self.connector_resolver = Some(resolver);
        self
    }

    pub fn with_source_link_fetcher(mut self, fetcher: Arc<dyn PageFetcher>) -> Self {
        self.source_link_fetcher = fetcher;
        self
    }

    pub fn with_request_limiter(mut self, limiter: Arc<RequestRateLimiter>) -> Self {
        self.request_limiter = limiter;
        self
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    /// POST /sources/validate
    pub async fn validate(
        &self,
        cmd: ValidateSourceCommand,
    ) -> Result<ValidateSourceResponse, AppError> {
        let parsed = self
            .resolve_source_input(&cmd.url, cmd.collector_account_id)
            .await?;
        let input_hash = SourceParser::input_hash(&parsed.canonical_url);
        let idem_key = idempotency::source_validate(&input_hash);

        // 强标识已存在 → 返回已有 source，不新建
        if let (Some(id_type), Some(id_value)) = (
            parsed.strong_id_type.as_deref(),
            parsed.strong_id_value.as_deref(),
        ) {
            let id_type = SourceIdType::parse(id_type)
                .map_err(|e| AppError::new("source.invalid_id_type", e))?;
            if let Some((account, _)) = self
                .sources
                .find_by_strong_id(id_type, id_value)
                .await
                .map_err(map_repo)?
            {
                let existing_id = account.id;
                let public = self.to_public(&account).await?;
                // 仍返回/复用 validation run（幂等）
                if let Some(run) = self
                    .sources
                    .get_validation_run_by_hash(&input_hash)
                    .await
                    .map_err(map_repo)?
                {
                    return Ok(ValidateSourceResponse {
                        run,
                        existing_source: Some(public),
                        idempotency_key: idem_key,
                        already_exists: true,
                    });
                }
                // 新 URL hash 但强标识已存在：写一条已完成 stub run，不重复验证/建号
                let mut run = self
                    .ensure_run(&parsed, &input_hash, cmd.collector_account_id, &idem_key)
                    .await?;
                if run.status == ValidationRunStatus::Running {
                    run = self
                        .sources
                        .complete_validation_run(CompleteValidationRun {
                            run_id: run.id,
                            status: ValidationRunStatus::Succeeded,
                            candidate_count: 0,
                            successful_count: 0,
                            available_count: 0,
                            missing_reason: Some("强标识已存在，跳过重复验证".into()),
                            candidates: vec![],
                            source_account_id: Some(existing_id),
                            completed_at: self.now(),
                        })
                        .await
                        .map_err(map_repo)?;
                }
                return Ok(ValidateSourceResponse {
                    run,
                    existing_source: Some(public),
                    idempotency_key: idem_key,
                    already_exists: true,
                });
            }
        }

        // 幂等：已有 run 直接返回
        if let Some(run) = self
            .sources
            .get_validation_run_by_hash(&input_hash)
            .await
            .map_err(map_repo)?
        {
            let existing_source = if let Some(sid) = run.source_account_id {
                if let Some(a) = self.sources.get_source(sid).await.map_err(map_repo)? {
                    Some(self.to_public(&a).await?)
                } else {
                    None
                }
            } else {
                None
            };
            return Ok(ValidateSourceResponse {
                run,
                existing_source,
                idempotency_key: idem_key,
                already_exists: false,
            });
        }

        let run = self
            .ensure_run(&parsed, &input_hash, cmd.collector_account_id, &idem_key)
            .await?;

        // 若 run 已完成（并发插入返回已有），直接返回
        if run.status != ValidationRunStatus::Running || run.completed_at.is_some() {
            return Ok(ValidateSourceResponse {
                run,
                existing_source: None,
                idempotency_key: idem_key,
                already_exists: false,
            });
        }

        Ok(ValidateSourceResponse {
            run,
            existing_source: None,
            idempotency_key: idem_key,
            already_exists: false,
        })
    }

    async fn resolve_source_input(
        &self,
        raw_url: &str,
        collector_account_id: Option<Uuid>,
    ) -> Result<ParsedSource, AppError> {
        if let Ok(parsed) = self.parser.parse(raw_url) {
            return Ok(parsed);
        }
        if !is_allowed_short_link(raw_url) {
            return Err(AppError::new(
                "source.invalid_url",
                "仅接受微信文章链接或受支持的腾讯短链接",
            ));
        }
        self.acquire_request_budget(
            collector_account_id,
            stable_uuid(raw_url),
            short_link_host(raw_url).as_deref().unwrap_or("unknown"),
            RequestPurpose::Search,
        )?;
        let page = self
            .source_link_fetcher
            .fetch_page(&PageFetchRequest {
                url: raw_url.to_string(),
                proxy_url: None,
            })
            .await
            .map_err(|error| AppError::new(error.code, error.message))?;
        self.parser
            .parse_resolved(&page.effective_url, &page.html)
            .map_err(|error| AppError::new("source.invalid_url", error))
    }

    fn acquire_request_budget(
        &self,
        collector_id: Option<Uuid>,
        source_id: Uuid,
        host: &str,
        purpose: RequestPurpose,
    ) -> Result<(), AppError> {
        self.request_limiter
            .acquire(
                &RequestLimitContext {
                    collector_id,
                    egress_id: None,
                    source_id,
                    host: host.to_string(),
                    purpose,
                },
                self.now(),
            )
            .map_err(|error| {
                AppError::new(
                    error.code,
                    format!(
                        "request budget exhausted; retry after {} ms",
                        error.retry_after_ms
                    ),
                )
            })
    }

    async fn ensure_run(
        &self,
        parsed: &ParsedSource,
        input_hash: &str,
        collector_account_id: Option<Uuid>,
        idem_key: &str,
    ) -> Result<SourceValidationRun, AppError> {
        let now = self.now();
        let run_id = Uuid::new_v4();
        let run = self
            .sources
            .insert_validation_run(CreateValidationRun {
                id: run_id,
                input_url: parsed.input_url.clone(),
                input_hash: input_hash.to_string(),
                parsed_source_json: serde_json::to_value(parsed)
                    .map_err(|e| AppError::internal(e.to_string()))?,
                connector_account_id: collector_account_id,
                expires_at: now + Duration::hours(VALIDATION_TTL_HOURS),
                now,
            })
            .await
            .map_err(map_repo)?;

        // 记录 source_validation job（幂等）
        let _ = self
            .jobs
            .create_job(CreateJob::new(
                KIND_SOURCE_VALIDATION,
                idem_key.to_string(),
                serde_json::to_value(SourceValidationJobPayload {
                    validation_run_id: run.id,
                })
                .map_err(|e| AppError::internal(e.to_string()))?,
            ))
            .await
            .map_err(|e| AppError::new("source.validation_job_create_failed", e.to_string()))?;

        Ok(run)
    }

    /// source_validation job 第一阶段：发现候选、创建详情任务并持久化进度。
    pub async fn start_validation_job(&self, run_id: Uuid) -> Result<serde_json::Value, AppError> {
        let run = self
            .sources
            .get_validation_run(run_id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.validation_not_found", "验证 run 不存在"))?;
        if run.status != ValidationRunStatus::Running {
            return Ok(json!({ "validation_run_id": run.id, "status": run.status.as_str() }));
        }
        if run.source_account_id.is_some() {
            return Ok(json!({
                "validation_run_id": run.id,
                "status": "details_already_enqueued",
                "candidate_count": run.candidate_count,
            }));
        }
        let parsed: ParsedSource = serde_json::from_value(run.parsed_source_json.clone())
            .map_err(|e| AppError::new("source.validation_payload_invalid", e.to_string()))?;
        let now = self.now();
        let (connector_name, candidates) = self
            .discover_candidates(&parsed, run.connector_account_id)
            .await?;

        // 按文章 identity 去重，最多 3 篇
        let mut seen = HashSet::new();
        let mut unique: Vec<ArticleCandidate> = Vec::new();
        for c in candidates {
            let canon = canonicalize_url(&c.canonical_url);
            let (biz, mid, idx, _) = parse_wechat_from_url(&canon);
            let key = if let (Some(b), Some(m), Some(i)) = (&biz, &mid, &idx) {
                format!("w:{b}:{m}:{i}")
            } else {
                format!("u:{canon}")
            };
            if !seen.insert(key) {
                continue;
            }
            unique.push(ArticleCandidate {
                title: c.title,
                canonical_url: canon,
                published_at: c.published_at,
                identity: c.identity,
                author: c.author,
                cover_url: c.cover_url,
                evidence: c.evidence,
            });
            if unique.len() >= MAX_CANDIDATES as usize {
                break;
            }
        }

        let available_count = unique.len() as u8;
        let missing_reason = if available_count < MAX_CANDIDATES {
            Some(format!(
                "候选不足 {MAX_CANDIDATES} 篇，实际可用 {available_count}"
            ))
        } else {
            None
        };

        // 创建 draft source（verifying）
        let source_id = Uuid::new_v4();
        let display = parsed
            .display_name_hint
            .clone()
            .unwrap_or_else(|| "未命名公众号".into());
        let source = self
            .sources
            .insert_source(CreateSourceAccount {
                id: source_id,
                display_name: display,
                status: SourceStatus::Verifying,
                preferred_connector: Some(connector_name.clone()),
                now,
            })
            .await
            .map_err(map_repo)?;

        // 强标识先占位（确认创建前也可防并发重复）
        if let (Some(id_type), Some(id_value)) = (
            parsed.strong_id_type.as_deref(),
            parsed.strong_id_value.as_deref(),
        ) {
            let id_type = SourceIdType::parse(id_type)
                .map_err(|e| AppError::new("source.invalid_id_type", e))?;
            if let Err(e) = self
                .sources
                .insert_source_id(CreateSourceAccountId {
                    id: Uuid::new_v4(),
                    source_account_id: source.id,
                    id_type,
                    id_value: id_value.to_string(),
                    confidence: IdConfidence::Strong,
                })
                .await
            {
                if e.contains("strong_id_conflict") {
                    // 并发冲突：清理 draft 语义上返回已有（简化：标记失败）
                    return Err(AppError::new(
                        "source.strong_id_conflict",
                        "强标识已被占用，请勿重复创建",
                    ));
                }
                return Err(map_repo(e));
            }
        }

        self.articles
            .ensure_source_stub(source.id, &source.display_name)
            .await
            .map_err(map_repo)?;

        let mut results = Vec::new();
        for c in &unique {
            let (biz, mid, idx, sn) = parse_wechat_from_url(&c.canonical_url);
            let job_id = self
                .capture
                .enqueue_detail(FetchArticleDetailPayload {
                    source_account_id: source.id,
                    collector_account_id: run.connector_account_id,
                    canonical_url: c.canonical_url.clone(),
                    wechat_biz: biz,
                    wechat_mid: mid,
                    wechat_idx: idx,
                    wechat_sn: sn,
                    title_hint: Some(c.title.clone()),
                    candidate_published_at: c.published_at,
                    candidate_author: c.author.clone(),
                    candidate_cover_url: c.cover_url.clone(),
                    candidate_evidence: c.evidence.clone(),
                    source_heartbeat_event_id: None,
                    heartbeat_manual: false,
                    heartbeat_recalculate_next: false,
                    discovered_via: "initial_validation".into(),
                })
                .await
                .map_err(|e| AppError::new("source.detail_job_create_failed", e))?;
            results.push(ValidationCandidateResult {
                canonical_url: c.canonical_url.clone(),
                title: Some(c.title.clone()),
                identity_key: None,
                status: "queued".into(),
                error_code: None,
                article_id: None,
                job_id: Some(job_id),
            });
        }
        self.sources
            .save_validation_progress(SaveValidationProgress {
                run_id: run.id,
                candidate_count: results.len() as u8,
                available_count,
                missing_reason,
                candidates: results,
                source_account_id: source.id,
            })
            .await
            .map_err(map_repo)?;
        let mut aggregate_job = CreateJob::new(
            KIND_SOURCE_VALIDATION_AGGREGATE,
            format!("source-validation-aggregate:{}", run.id),
            serde_json::to_value(SourceValidationAggregatePayload {
                validation_run_id: run.id,
            })
            .map_err(|e| AppError::internal(e.to_string()))?,
        );
        aggregate_job.max_attempts = Some(100);
        self.jobs.create_job(aggregate_job).await.map_err(|e| {
            AppError::new("source.validation_aggregate_create_failed", e.to_string())
        })?;

        Ok(json!({
            "validation_run_id": run.id,
            "status": "detail_jobs_enqueued",
            "candidate_count": unique.len(),
        }))
    }

    /// source_validation_aggregate：所有详情终态后一次性完成 run。
    pub async fn aggregate_validation_job(
        &self,
        run_id: Uuid,
    ) -> Result<SourceValidationRun, AppError> {
        let run = self
            .sources
            .get_validation_run(run_id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.validation_not_found", "验证 run 不存在"))?;
        if run.status != ValidationRunStatus::Running {
            return Ok(run);
        }
        let source_id = run.source_account_id.ok_or_else(|| {
            AppError::new(
                "source.validation_incomplete",
                "验证 run 尚未创建草稿公众号",
            )
        })?;
        let mut pending = false;
        let mut results = Vec::with_capacity(run.candidates.len());
        for candidate in &run.candidates {
            let job_id = candidate.job_id.ok_or_else(|| {
                AppError::new("source.validation_candidate_invalid", "候选缺少详情 job_id")
            })?;
            let job = self
                .jobs
                .get_job(job_pipeline::JobId::from_raw(job_id))
                .await
                .map_err(|e| AppError::new("source.validation_job_read_failed", e.to_string()))?
                .ok_or_else(|| {
                    AppError::not_found("source.detail_job_not_found", "详情任务不存在")
                })?;
            let mut result = candidate.clone();
            match job.status {
                JobStatus::Succeeded => {
                    result.status = "ready".into();
                    result.article_id = job
                        .result_json
                        .as_ref()
                        .and_then(|v| v.get("article_id"))
                        .and_then(|v| v.as_str())
                        .and_then(|v| Uuid::parse_str(v).ok());
                    result.identity_key = job
                        .result_json
                        .as_ref()
                        .and_then(|v| v.get("identity_key"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    if result.article_id.is_none() {
                        result.status = "failed".into();
                        result.error_code = Some("source.detail_result_invalid".into());
                    }
                }
                JobStatus::Failed
                | JobStatus::NeedsAction
                | JobStatus::Cancelled
                | JobStatus::DeadLetter => {
                    result.status = "failed".into();
                    result.error_code = job
                        .last_error_code
                        .clone()
                        .or_else(|| Some("source.detail_failed".into()));
                }
                _ => {
                    pending = true;
                    result.status = job.status.as_str().into();
                }
            }
            results.push(result);
        }
        if pending {
            return Err(AppError::new(
                "source.validation_details_pending",
                "详情任务尚未全部完成",
            ));
        }

        let successful_count = results.iter().filter(|r| r.status == "ready").count() as u8;
        let run_status = if successful_count > 0 {
            ValidationRunStatus::Succeeded
        } else {
            ValidationRunStatus::Failed
        };
        let source_status = if successful_count > 0 {
            SourceStatus::PendingConfiguration
        } else {
            SourceStatus::NeedsAction
        };
        self.sources
            .update_source_status(source_id, source_status, self.now())
            .await
            .map_err(map_repo)?;
        let completed = self
            .sources
            .complete_validation_run(CompleteValidationRun {
                run_id,
                status: run_status,
                candidate_count: results.len() as u8,
                successful_count,
                available_count: run.available_count,
                missing_reason: run.missing_reason.clone(),
                candidates: results,
                source_account_id: Some(source_id),
                completed_at: self.now(),
            })
            .await
            .map_err(map_repo)?;

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM,
                    None,
                    "source.initial_validation",
                    Some("source_account"),
                    Some(&source_id.to_string()),
                    json!({
                        "validation_run_id": run_id.to_string(),
                        "status": run_status.as_str(),
                        "candidate_count": completed.candidate_count,
                        "successful_count": successful_count,
                    }),
                    self.now(),
                )
                .await;
        }

        Ok(completed)
    }

    pub async fn fail_validation_job(
        &self,
        run_id: Uuid,
        reason_code: &str,
    ) -> Result<SourceValidationRun, AppError> {
        let run = self
            .sources
            .get_validation_run(run_id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.validation_not_found", "验证 run 不存在"))?;
        if run.status != ValidationRunStatus::Running {
            return Ok(run);
        }
        if let Some(source_id) = run.source_account_id {
            self.sources
                .update_source_status(source_id, SourceStatus::NeedsAction, self.now())
                .await
                .map_err(map_repo)?;
        }
        self.sources
            .complete_validation_run(CompleteValidationRun {
                run_id,
                status: ValidationRunStatus::Failed,
                candidate_count: run.candidate_count,
                successful_count: 0,
                available_count: run.available_count,
                missing_reason: Some(reason_code.to_string()),
                candidates: run.candidates,
                source_account_id: run.source_account_id,
                completed_at: self.now(),
            })
            .await
            .map_err(map_repo)
    }

    async fn discover_candidates(
        &self,
        parsed: &ParsedSource,
        collector_account_id: Option<Uuid>,
    ) -> Result<(String, Vec<ArticleCandidate>), AppError> {
        let resolver = self.connector_resolver.as_ref().ok_or_else(|| {
            AppError::new(
                "connector.resolver_not_configured",
                "采集账号解析器未装配，拒绝使用临时账号",
            )
        })?;
        let selected = resolver
            .resolve(
                ConnectorCapability::DiscoverRecent,
                collector_account_id,
                None,
            )
            .await?;
        let connector_name = selected.account.connector.clone();
        self.acquire_request_budget(
            Some(selected.account.id),
            stable_uuid(&parsed.canonical_url),
            "mp.weixin.qq.com",
            RequestPurpose::List,
        )?;
        let backend = self
            .connectors
            .get(&connector_name)
            .ok_or_else(|| AppError::new("connector.unsupported", "选中的 connector 未注册"))?;
        let identity = parsed
            .strong_id_value
            .clone()
            .unwrap_or_else(|| parsed.canonical_url.clone());
        let candidates = backend
            .discover_recent(DiscoverRequest {
                source: SourceRef {
                    id: None,
                    identity,
                    level_hint: None,
                },
                account: selected.account,
                // connector 返回中可能含重复项，多取一小段后再按强 identity 截取前三篇。
                limit: MAX_CANDIDATES * 4,
                cursor: 0,
                page_budget: 1,
                credentials_json: selected.credentials_json,
            })
            .await
            .map_err(|e| AppError::new(e.reason_code, e.message))?;
        Ok((connector_name, candidates))
    }

    /// POST /sources 确认创建
    pub async fn confirm(
        &self,
        cmd: ConfirmSourceCommand,
    ) -> Result<SourceAccountPublic, AppError> {
        let run = self
            .sources
            .get_validation_run(cmd.validation_run_id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.validation_not_found", "验证 run 不存在"))?;

        if run.status != ValidationRunStatus::Succeeded || run.successful_count < 1 {
            return Err(AppError::new(
                "source.initial_validation_failed",
                "至少一篇验证成功才能确认创建",
            ));
        }
        if self.now() > run.expires_at {
            return Err(AppError::new(
                "source.validation_expired",
                "验证 run 已过期",
            ));
        }
        let source_id = run.source_account_id.ok_or_else(|| {
            AppError::new("source.validation_incomplete", "验证 run 未关联监测公众号")
        })?;
        let mut source = self
            .sources
            .get_source(source_id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.not_found", "监测公众号不存在"))?;

        if source.status != SourceStatus::PendingConfiguration
            && source.status != SourceStatus::Active
        {
            return Err(AppError::new(
                "source.invalid_status",
                format!("当前状态 {} 不可确认配置", source.status.as_str()),
            ));
        }

        let level = SourceLevel::parse(&cmd.base_level)
            .map_err(|e| AppError::new("source.invalid_level", e))?;
        if cmd.working_windows.is_empty() {
            return Err(AppError::new(
                "source.invalid_windows",
                "working_windows 至少需要一个窗口",
            ));
        }
        for (i, w) in cmd.working_windows.iter().enumerate() {
            validate_hhmm(&format!("working_windows[{i}].start"), &w.start)?;
            validate_hhmm(&format!("working_windows[{i}].end"), &w.end)?;
        }

        // 锚点必须用管理员提交的 first_heartbeat_at，不得用 added_at
        let first = cmd.first_heartbeat_at;
        let now = self.now();
        let window_count = cmd.working_windows.len();
        self.sources
            .upsert_fetch_rules(UpsertFetchRules {
                source_account_id: source_id,
                base_level: level,
                working_windows: cmd.working_windows,
                first_heartbeat_at: first,
                next_heartbeat_at: first,
                connector_override: cmd.preferred_connector.clone(),
                now,
            })
            .await
            .map_err(map_repo)?;

        if let Some(name) = cmd.display_name {
            source = self
                .sources
                .patch_source(
                    source_id,
                    Some(name),
                    Some(cmd.preferred_connector.clone()),
                    None,
                    now,
                )
                .await
                .map_err(map_repo)?;
        } else if cmd.preferred_connector.is_some() {
            source = self
                .sources
                .patch_source(
                    source_id,
                    None,
                    Some(cmd.preferred_connector.clone()),
                    None,
                    now,
                )
                .await
                .map_err(map_repo)?;
        }

        if source.status == SourceStatus::PendingConfiguration {
            source = self
                .sources
                .update_source_status(source_id, SourceStatus::Active, now)
                .await
                .map_err(map_repo)?;
        }

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "source.created",
                    Some("source_account"),
                    Some(&source_id.to_string()),
                    json!({
                        "validation_run_id": cmd.validation_run_id.to_string(),
                        "base_level": level.as_str(),
                        "status": source.status.as_str(),
                    }),
                    now,
                )
                .await;
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "source.rules_updated",
                    Some("source_account"),
                    Some(&source_id.to_string()),
                    json!({
                        "base_level": level.as_str(),
                        "window_count": window_count,
                    }),
                    now,
                )
                .await;
        }

        self.to_public(&source).await
    }

    pub async fn get(&self, id: Uuid) -> Result<SourceAccountPublic, AppError> {
        let source = self
            .sources
            .get_source(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.not_found", "监测公众号不存在"))?;
        self.to_public(&source).await
    }

    pub async fn get_validation_run(&self, id: Uuid) -> Result<SourceValidationRun, AppError> {
        self.sources
            .get_validation_run(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.validation_not_found", "验证 run 不存在"))
    }

    /// GET /sources — 返回全部监测公众号。
    pub async fn list(&self) -> Result<Vec<SourceAccountPublic>, AppError> {
        let accounts = self.sources.list_sources().await.map_err(map_repo)?;
        let mut out = Vec::with_capacity(accounts.len());
        for account in accounts {
            out.push(self.to_public(&account).await?);
        }
        Ok(out)
    }

    pub async fn patch(
        &self,
        id: Uuid,
        cmd: PatchSourceCommand,
    ) -> Result<SourceAccountPublic, AppError> {
        if cmd.expected_version < 1 {
            return Err(AppError::new(
                "source.invalid_version",
                "expected_version must be positive",
            ));
        }
        let status = match cmd.status.as_deref() {
            Some(s) => Some(
                SourceStatus::parse(s).map_err(|e| AppError::new("source.invalid_status", e))?,
            ),
            None => None,
        };
        let base_level = cmd
            .base_level
            .as_deref()
            .map(SourceLevel::parse)
            .transpose()
            .map_err(|e| AppError::new("source.invalid_level", e))?;
        if let Some(name) = cmd.display_name.as_deref() {
            validate_nonempty("display_name", name)?;
        }
        validate_patch_connector("preferred_connector", &cmd.preferred_connector)?;
        validate_patch_connector("connector_override", &cmd.connector_override)?;
        if let PatchField::Value(value) = &cmd.interval_override_hours
            && !(1..=8760).contains(value)
        {
            return Err(AppError::new(
                "source.invalid_interval",
                "interval_override_hours must be 1..=8760",
            ));
        }
        if let Some(windows) = &cmd.working_windows {
            if windows.is_empty() {
                return Err(AppError::new(
                    "source.invalid_windows",
                    "working_windows must not be empty",
                ));
            }
            for (i, window) in windows.iter().enumerate() {
                validate_hhmm(&format!("working_windows[{i}].start"), &window.start)?;
                validate_hhmm(&format!("working_windows[{i}].end"), &window.end)?;
                if window.start >= window.end {
                    return Err(AppError::new(
                        "source.invalid_windows",
                        format!("working_windows[{i}] start must precede end"),
                    ));
                }
            }
        }
        let now = self.now();
        let legacy_next = cmd.next_heartbeat_at;
        let manual_next = cmd.manual_next_heartbeat_at.clone();
        if let Some(next) = legacy_next
            && (next < now || next > now + Duration::days(3660))
        {
            return Err(AppError::new(
                "source.invalid_next_heartbeat",
                "next_heartbeat_at must be between now and ten years from now",
            ));
        }
        if let PatchField::Value(next) = manual_next
            && (next < now || next > now + Duration::days(3660))
        {
            return Err(AppError::new(
                "source.invalid_next_heartbeat",
                "manual_next_heartbeat_at must be between now and ten years from now",
            ));
        }
        if let (Some(legacy), PatchField::Value(manual)) =
            (legacy_next, &cmd.manual_next_heartbeat_at)
            && legacy != *manual
        {
            return Err(AppError::new(
                "source.conflicting_next_heartbeat",
                "next_heartbeat_at and manual_next_heartbeat_at must match when both are supplied",
            ));
        }
        if legacy_next.is_some() && matches!(cmd.manual_next_heartbeat_at, PatchField::Null) {
            return Err(AppError::new(
                "source.conflicting_next_heartbeat",
                "next_heartbeat_at cannot be combined with manual_next_heartbeat_at: null",
            ));
        }
        let preferred_summary = patch_summary(&cmd.preferred_connector);
        let connector_summary = patch_summary(&cmd.connector_override);
        let result = self
            .sources
            .patch_source_aggregate(PatchSourceAggregate {
                source_account_id: id,
                expected_version: cmd.expected_version,
                display_name: cmd.display_name,
                preferred_connector: cmd.preferred_connector.into_repo(),
                status,
                base_level,
                working_windows: cmd.working_windows,
                connector_override: cmd.connector_override.into_repo(),
                interval_override_hours: cmd.interval_override_hours.into_repo(),
                enabled: cmd.enabled,
                manual_next_heartbeat_at: cmd.manual_next_heartbeat_at.into_repo(),
                next_heartbeat_at: legacy_next,
                auto_promotion_enabled: cmd.auto_promotion_enabled,
                now,
            })
            .await
            .map_err(map_repo)?;
        if let Some(ref audit) = self.audit {
            audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "source.rules_updated",
                    Some("source_account"),
                    Some(&id.to_string()),
                    json!({
                        "version_before": cmd.expected_version,
                        "version_after": result.account.version,
                        "display_name_changed": result.previous_account.display_name != result.account.display_name,
                        "status": {"before": result.previous_account.status.as_str(), "after": result.account.status.as_str()},
                        "preferred_connector": {"operation": preferred_summary,
                            "before_configured": result.previous_account.preferred_connector.is_some(),
                            "after_configured": result.account.preferred_connector.is_some()},
                        "connector_override": {"operation": connector_summary,
                            "before_configured": result.previous_rules.connector_override.is_some(),
                            "after_configured": result.rules.connector_override.is_some()},
                        "base_level": {"before": result.previous_rules.base_level.as_str(), "after": result.rules.base_level.as_str()},
                        "working_window_count": {"before": result.previous_rules.working_windows.len(), "after": result.rules.working_windows.len()},
                        "interval_override_hours": {"before": result.previous_rules.interval_override_hours, "after": result.rules.interval_override_hours},
                        "enabled": {"before": result.previous_rules.enabled, "after": result.rules.enabled},
                        "next_heartbeat_changed": result.previous_rules.next_heartbeat_at != result.rules.next_heartbeat_at,
                        "manual_next_heartbeat": {"before": result.previous_rules.manual_next_heartbeat_at, "after": result.rules.manual_next_heartbeat_at},
                        "auto_promotion_enabled": {"before": result.previous_rules.auto_promotion_enabled, "after": result.rules.auto_promotion_enabled},
                    }),
                    now,
                )
                .await
                .map_err(AppError::internal)?;
        }
        self.to_public(&result.account).await
    }

    /// 再次验证：对新 URL 或同一公众号重新跑 validate（使用已有强标识 URL）。
    pub async fn verify_again(
        &self,
        id: Uuid,
        url: Option<String>,
    ) -> Result<ValidateSourceResponse, AppError> {
        let source = self
            .sources
            .get_source(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.not_found", "监测公众号不存在"))?;
        let url = if let Some(u) = url {
            u
        } else {
            // 从强标识构造占位 URL
            let ids = self.sources.list_source_ids(id).await.map_err(map_repo)?;
            let biz = ids
                .iter()
                .find(|i| i.id_type == SourceIdType::Biz)
                .map(|i| i.id_value.clone())
                .ok_or_else(|| {
                    AppError::new("source.missing_url", "缺少验证 URL，请在请求体提供 url")
                })?;
            format!("https://mp.weixin.qq.com/s?__biz={biz}&mid=0&idx=1&sn=verify")
        };
        // 已有强标识时 validate 会返回 already_exists；此处将状态置回 verifying 再跑
        let _ = source;
        self.validate(ValidateSourceCommand {
            url,
            collector_account_id: None,
        })
        .await
    }

    /// fetch-now：创建 fetch_source job。不改变 first_heartbeat_at。
    pub async fn fetch_now(
        &self,
        id: Uuid,
        cmd: FetchNowCommand,
    ) -> Result<FetchNowResponse, AppError> {
        let source = self
            .sources
            .get_source(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::not_found("source.not_found", "监测公众号不存在"))?;
        if source.status != SourceStatus::Active && source.status != SourceStatus::Paused {
            return Err(AppError::new(
                "source.not_active",
                "仅 active/paused 可立即抓取",
            ));
        }
        // 确认规则存在且不触碰 first_heartbeat_at
        let rules = self
            .sources
            .get_fetch_rules(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::new("source.fetch_rules_not_found", "抓取规则不存在"))?;
        let _first_anchor = rules.first_heartbeat_at;

        let ids = self.sources.list_source_ids(id).await.map_err(map_repo)?;
        let identity = ids
            .first()
            .map(|i| i.id_value.clone())
            .unwrap_or_else(|| source.id.to_string());
        let command_key = cmd
            .request_id
            .clone()
            .unwrap_or_else(|| format!("source-version-{}", source.version));
        let heartbeat = format!("manual:{command_key}");
        let key = idempotency::source_fetch(&id.to_string(), &heartbeat);
        let resolver = self.connector_resolver.as_ref().ok_or_else(|| {
            AppError::new("connector.resolver_not_configured", "采集账号解析器未装配")
        })?;
        let selected = resolver
            .resolve(
                ConnectorCapability::DiscoverRecent,
                None,
                source.preferred_connector.clone(),
            )
            .await?;
        let payload = json!({
            "source_account_id": id,
            "source_identity": identity,
            "heartbeat_event_id": heartbeat,
            "collector_account_id": selected.account.id,
            "connector": selected.account.connector,
            "limit": 10,
            "manual": true,
            "recalculate_next": cmd.recalculate_next,
        });
        let job = self
            .jobs
            .create_job(CreateJob::new(KIND_FETCH_SOURCE, key.clone(), payload))
            .await
            .map_err(|e| AppError::internal(e.to_string()))?;

        // 再次确认锚点未变
        let rules_after = self
            .sources
            .get_fetch_rules(id)
            .await
            .map_err(map_repo)?
            .ok_or_else(|| AppError::new("source.fetch_rules_not_found", "抓取规则不存在"))?;
        if rules_after.first_heartbeat_at != _first_anchor {
            return Err(AppError::internal(
                "heartbeat.first_anchor_mutated: 手动抓取不得改写 first_heartbeat_at",
            ));
        }

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_SYSTEM_ADMINISTRATOR,
                    None,
                    "article.manual_fetch",
                    Some("source_account"),
                    Some(&id.to_string()),
                    json!({
                        "job_id": job.id.as_uuid().to_string(),
                        "recalculate_next": cmd.recalculate_next,
                    }),
                    self.now(),
                )
                .await;
        }

        Ok(FetchNowResponse {
            job_id: *job.id.as_uuid(),
            idempotency_key: key,
            recalculate_next: cmd.recalculate_next,
        })
    }

    async fn to_public(
        &self,
        account: &crate::domain::source::SourceAccount,
    ) -> Result<SourceAccountPublic, AppError> {
        let ids = self
            .sources
            .list_source_ids(account.id)
            .await
            .map_err(map_repo)?;
        let rules = self
            .sources
            .get_fetch_rules(account.id)
            .await
            .map_err(map_repo)?;
        Ok(SourceAccountPublic {
            id: account.id,
            display_name: account.display_name.clone(),
            status: account.status,
            preferred_connector: account.preferred_connector.clone(),
            version: account.version,
            strong_ids: ids
                .into_iter()
                .map(|i| SourceAccountIdPublic {
                    id_type: i.id_type,
                    id_value: i.id_value,
                    confidence: i.confidence,
                })
                .collect(),
            added_at: account.added_at,
            last_fetch_at: account.last_fetch_at,
            fetch_rules: rules.map(|r| AccountFetchRulesPublic {
                base_level: r.base_level,
                enabled: r.enabled,
                first_heartbeat_at: r.first_heartbeat_at,
                next_heartbeat_at: r.next_heartbeat_at,
                working_windows: r.working_windows,
                connector_override: r.connector_override,
                interval_override_hours: r.interval_override_hours,
                manual_next_heartbeat_at: r.manual_next_heartbeat_at,
                auto_promotion_enabled: r.auto_promotion_enabled,
                updated_at: r.updated_at,
            }),
        })
    }
}

// 抑制未使用警告（identity 预留给扩展）
impl SourceValidationService {
    #[allow(dead_code)]
    fn resolve_identity_key(
        &self,
        source_id: Uuid,
        url: &str,
        biz: Option<&str>,
        mid: Option<&str>,
        idx: Option<&str>,
    ) -> String {
        self.identity
            .resolve(IdentityInput {
                source_account_id: source_id,
                canonical_url: url,
                wechat_biz: biz,
                wechat_mid: mid,
                wechat_idx: idx,
            })
            .key()
    }
}

fn short_link_host(raw_url: &str) -> Option<String> {
    let parsed = url::Url::parse(raw_url.trim()).ok()?;
    parsed.host_str().map(str::to_ascii_lowercase)
}

fn is_allowed_short_link(raw_url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(raw_url.trim()) else {
        return false;
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    matches!(
        host.trim_end_matches('.').to_ascii_lowercase().as_str(),
        "url.cn" | "weixin.qq.com" | "weixin110.qq.com"
    )
}
