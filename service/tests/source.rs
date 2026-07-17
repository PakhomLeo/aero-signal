//! R01 source validation：HTTP 命令只建 run/job，详情任务独立聚合并可重放。

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use crawler_capture::{FakeObjectStore, MemoryPageFetcher};
use job_pipeline::{JobId, JobRepository, JobStatus, KIND_FETCH_ARTICLE_DETAIL, MemoryRepo};
use platform_connectors::{
    ArticleCandidate, CapabilitySet, ConnectorBackend, ConnectorCapability, FakeConnector,
    NAME_FAKE,
};
use server_core::{Clock, SystemClock};
use uuid::Uuid;

use signal_service::adapters::secret_store::{MemorySecretStore, SecretStore};
use signal_service::domain::collector::{
    CollectorAccount, CollectorAccountStatus, CollectorEgressBinding, CollectorLoginSession,
};
use signal_service::domain::settings::WorkingWindow;
use signal_service::domain::source::{SourceLevel, SourceStatus, ValidationRunStatus};
use signal_service::jobs::signal_job_registry_with;
use signal_service::repository::source::{CreateSourceAccount, UpsertFetchRules};
use signal_service::repository::{
    ArticleRepository, CollectorRepository, MemoryArticleStore, MemoryCollectorStore,
    MemorySourceStore, SourceRepository,
};
use signal_service::service::source_validation::{PatchField, PatchSourceCommand};
use signal_service::service::{
    ArticleCaptureService, CollectorConnectorResolver, ConfirmSourceCommand,
    SourceValidationService, ValidateSourceCommand,
};

struct Harness {
    service: Arc<SourceValidationService>,
    sources: Arc<MemorySourceStore>,
    jobs: Arc<MemoryRepo>,
    connector: Arc<FakeConnector>,
    pages: Arc<MemoryPageFetcher>,
}

async fn setup() -> Harness {
    let articles = Arc::new(MemoryArticleStore::new());
    let sources = Arc::new(MemorySourceStore::new());
    let jobs = Arc::new(MemoryRepo::new());
    let connector = Arc::new(FakeConnector::new());
    let pages = Arc::new(MemoryPageFetcher::default());
    let mut registry = platform_connectors::ConnectorRegistry::new();
    registry.register(connector.clone() as Arc<dyn ConnectorBackend>);
    let connectors = Arc::new(registry);
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let collectors = Arc::new(MemoryCollectorStore::new());
    let secrets = Arc::new(MemorySecretStore::new());
    let account_id = Uuid::new_v4();
    let now = Utc::now();
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
    let resolver = Arc::new(CollectorConnectorResolver::new(
        collectors,
        secrets,
        connectors.clone(),
        clock.clone(),
    ));
    let capture = Arc::new(ArticleCaptureService::new(
        articles.clone() as Arc<dyn ArticleRepository>,
        jobs.clone() as Arc<dyn JobRepository>,
        connectors.clone(),
        clock.clone(),
        Arc::new(FakeObjectStore::new()),
    ));
    let service = Arc::new(
        SourceValidationService::new(
            sources.clone() as Arc<dyn SourceRepository>,
            articles as Arc<dyn ArticleRepository>,
            jobs.clone() as Arc<dyn JobRepository>,
            capture,
            connectors,
            clock,
        )
        .with_connector_resolver(resolver)
        .with_source_link_fetcher(pages.clone()),
    );
    Harness {
        service,
        sources,
        jobs,
        connector,
        pages,
    }
}

#[tokio::test]
async fn supported_short_link_uses_bounded_resolver_and_trusted_html_identity() {
    let h = setup().await;
    h.pages.insert_page(
        "https://url.cn/short-article",
        "https://mp.weixin.qq.com/s/article?mid=100&idx=1&sn=abc",
        r#"<a href="/s?__biz=MzShortBiz==&amp;mid=100">article</a>"#,
    );

    let response = h
        .service
        .validate(ValidateSourceCommand {
            url: "https://url.cn/short-article".into(),
            collector_account_id: None,
        })
        .await
        .unwrap();

    assert_eq!(
        response.run.parsed_source_json["wechat_biz"],
        "MzShortBiz=="
    );
    assert_eq!(h.pages.call_count(), 1);
    assert!(!response.run.input_url.contains("url.cn"));
}

fn sample_url(biz: &str) -> String {
    format!("https://mp.weixin.qq.com/s?__biz={biz}&mid=100&idx=1&sn=abc")
}

fn candidate(biz: &str, mid: &str) -> ArticleCandidate {
    ArticleCandidate {
        title: format!("article-{mid}"),
        canonical_url: format!("https://mp.weixin.qq.com/s?__biz={biz}&mid={mid}&idx=1&sn=x{mid}"),
        published_at: None,
        identity: Some(format!("{biz}:{mid}")),
        author: None,
        cover_url: None,
        evidence: None,
    }
}

async fn begin(h: &Harness, biz: &str, candidates: Vec<ArticleCandidate>) -> Uuid {
    h.connector.set_discover_pool(candidates);
    let response = h
        .service
        .validate(ValidateSourceCommand {
            url: sample_url(biz),
            collector_account_id: None,
        })
        .await
        .unwrap();
    assert_eq!(response.run.status, ValidationRunStatus::Running);
    assert!(response.run.candidates.is_empty());
    h.service
        .start_validation_job(response.run.id)
        .await
        .unwrap();
    response.run.id
}

async fn detail_jobs(h: &Harness) -> Vec<job_pipeline::Job> {
    h.jobs
        .list_jobs(Some(KIND_FETCH_ARTICLE_DETAIL), None, 10)
        .await
        .unwrap()
}

async fn succeed(h: &Harness, job_id: JobId) {
    h.jobs
        .transition(job_id, "", JobStatus::Queued, JobStatus::Running)
        .await
        .unwrap();
    h.jobs
        .update_result(
            job_id,
            "",
            serde_json::json!({
                "article_id": Uuid::new_v4(),
                "identity_key": format!("identity:{job_id}"),
            }),
        )
        .await
        .unwrap();
    h.jobs
        .transition(job_id, "", JobStatus::Running, JobStatus::Succeeded)
        .await
        .unwrap();
}

async fn fail(h: &Harness, job_id: JobId) {
    h.jobs
        .transition(job_id, "", JobStatus::Queued, JobStatus::Running)
        .await
        .unwrap();
    h.jobs
        .fail(job_id, "", "crawler.fetch_failed", "network", false)
        .await
        .unwrap();
}

#[tokio::test]
async fn validate_only_creates_running_run_until_worker_starts() {
    let h = setup().await;
    let response = h
        .service
        .validate(ValidateSourceCommand {
            url: sample_url("AsyncOnly"),
            collector_account_id: None,
        })
        .await
        .unwrap();
    assert_eq!(response.run.status, ValidationRunStatus::Running);
    assert!(response.run.source_account_id.is_none());
    assert!(detail_jobs(&h).await.is_empty());
}

#[tokio::test]
async fn one_success_allows_configuration_after_all_details_finish() {
    let h = setup().await;
    let run_id = begin(
        &h,
        "OneReady",
        vec![candidate("OneReady", "201"), candidate("OneReady", "202")],
    )
    .await;
    let jobs = detail_jobs(&h).await;
    assert_eq!(jobs.len(), 2, "只验证 connector 发现的最新候选");
    succeed(&h, jobs[0].id).await;
    fail(&h, jobs[1].id).await;

    let run = h.service.aggregate_validation_job(run_id).await.unwrap();
    assert_eq!(run.status, ValidationRunStatus::Succeeded);
    assert_eq!(run.successful_count, 1);
    let source = h
        .sources
        .get_source(run.source_account_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.status, SourceStatus::PendingConfiguration);

    let confirmed = h
        .service
        .confirm(ConfirmSourceCommand {
            validation_run_id: run_id,
            display_name: None,
            base_level: "L1".into(),
            working_windows: vec![WorkingWindow {
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            first_heartbeat_at: Utc::now() + Duration::hours(1),
            preferred_connector: None,
        })
        .await
        .unwrap();
    assert_eq!(confirmed.status, SourceStatus::Active);
}

#[tokio::test]
async fn zero_success_becomes_needs_action() {
    let h = setup().await;
    let run_id = begin(&h, "ZeroReady", vec![]).await;
    for job in detail_jobs(&h).await {
        fail(&h, job.id).await;
    }
    let run = h.service.aggregate_validation_job(run_id).await.unwrap();
    assert_eq!(run.status, ValidationRunStatus::Failed);
    let source = h
        .sources
        .get_source(run.source_account_id.unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.status, SourceStatus::NeedsAction);
}

#[tokio::test]
async fn aggregate_waits_without_completing_partial_results() {
    let h = setup().await;
    let run_id = begin(&h, "Pending", vec![candidate("Pending", "301")]).await;
    let error = h
        .service
        .aggregate_validation_job(run_id)
        .await
        .unwrap_err();
    assert_eq!(error.code, "source.validation_details_pending");
    let run = h.sources.get_validation_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.status, ValidationRunStatus::Running);
}

#[tokio::test]
async fn discovery_deduplicates_and_caps_at_three() {
    let h = setup().await;
    let duplicate = candidate("CapThree", "501");
    let run_id = begin(
        &h,
        "CapThree",
        vec![
            duplicate.clone(),
            duplicate,
            candidate("CapThree", "502"),
            candidate("CapThree", "503"),
            candidate("CapThree", "504"),
        ],
    )
    .await;
    let run = h.sources.get_validation_run(run_id).await.unwrap().unwrap();
    assert_eq!(run.candidate_count, 3);
    assert_eq!(detail_jobs(&h).await.len(), 3);
}

#[tokio::test]
async fn validation_and_enqueue_are_idempotent() {
    let h = setup().await;
    h.connector
        .set_discover_pool(vec![candidate("Idempotent", "401")]);
    let command = ValidateSourceCommand {
        url: sample_url("Idempotent"),
        collector_account_id: None,
    };
    let first = h.service.validate(command.clone()).await.unwrap();
    let second = h.service.validate(command).await.unwrap();
    assert_eq!(first.run.id, second.run.id);
    h.service.start_validation_job(first.run.id).await.unwrap();
    let count = detail_jobs(&h).await.len();
    h.service.start_validation_job(first.run.id).await.unwrap();
    assert_eq!(detail_jobs(&h).await.len(), count);
}

#[tokio::test]
async fn validation_handlers_are_real_not_placeholders() {
    let h = setup().await;
    let registry = signal_job_registry_with(None, None, None, None, None, Some(h.service));
    assert!(registry.contains(job_pipeline::KIND_SOURCE_VALIDATION));
    assert!(registry.contains(job_pipeline::KIND_SOURCE_VALIDATION_AGGREGATE));
}

#[test]
fn public_validation_dto_rejects_test_injection_fields() {
    let parsed = serde_json::from_value::<ValidateSourceCommand>(serde_json::json!({
        "url": sample_url("NoInjection"),
        "sync_capture": true
    }));
    assert!(parsed.is_err());
}

#[tokio::test]
async fn validation_uses_persisted_authorization_and_secret_credentials() {
    use platform_connectors::{
        CapabilitySet, ConnectorCapability, ConnectorRegistry, MpAdminConnector, NAME_MP_ADMIN,
    };

    let articles = Arc::new(MemoryArticleStore::new());
    let sources = Arc::new(MemorySourceStore::new());
    let jobs = Arc::new(MemoryRepo::new());
    let collectors = Arc::new(MemoryCollectorStore::new());
    let secrets = Arc::new(MemorySecretStore::new());
    let mut registry = ConnectorRegistry::new();
    registry.register(Arc::new(MpAdminConnector::new()) as Arc<dyn ConnectorBackend>);
    let connectors = Arc::new(registry);
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let capture = Arc::new(ArticleCaptureService::new(
        articles.clone(),
        jobs.clone(),
        connectors.clone(),
        clock.clone(),
        Arc::new(FakeObjectStore::new()),
    ));
    let account_id = Uuid::new_v4();
    let now = Utc::now();
    collectors
        .insert_account(&CollectorAccount {
            id: account_id,
            connector: NAME_MP_ADMIN.into(),
            display_name: "mp".into(),
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
    let secret_ref = secrets
        .put(r#"{"authorized_mp_ids":["DifferentBiz"]}"#)
        .await
        .unwrap();
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
    let resolver = Arc::new(CollectorConnectorResolver::new(
        collectors,
        secrets,
        connectors.clone(),
        clock.clone(),
    ));
    let service = SourceValidationService::new(sources, articles, jobs, capture, connectors, clock)
        .with_connector_resolver(resolver);
    let response = service
        .validate(ValidateSourceCommand {
            url: sample_url("ExpectedBiz"),
            collector_account_id: Some(account_id),
        })
        .await
        .unwrap();
    let error = service
        .start_validation_job(response.run.id)
        .await
        .unwrap_err();
    assert_eq!(error.code, "mp_admin.unauthorized_enumeration");
}

async fn configured_source(h: &Harness) -> (Uuid, chrono::DateTime<Utc>) {
    let id = Uuid::new_v4();
    let now = Utc::now();
    h.sources
        .insert_source(CreateSourceAccount {
            id,
            display_name: "source".into(),
            status: SourceStatus::Active,
            preferred_connector: Some("old-account".into()),
            now,
        })
        .await
        .unwrap();
    let first = now + Duration::hours(1);
    h.sources
        .upsert_fetch_rules(UpsertFetchRules {
            source_account_id: id,
            base_level: SourceLevel::L1,
            working_windows: vec![WorkingWindow {
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            first_heartbeat_at: first,
            next_heartbeat_at: first,
            connector_override: Some("old-rule".into()),
            now,
        })
        .await
        .unwrap();
    (id, first)
}

#[test]
fn patch_nullable_fields_distinguish_missing_null_and_value() {
    let missing: PatchSourceCommand =
        serde_json::from_value(serde_json::json!({"expected_version": 1})).unwrap();
    assert!(matches!(missing.connector_override, PatchField::Missing));
    assert!(matches!(missing.preferred_connector, PatchField::Missing));
    let clear: PatchSourceCommand = serde_json::from_value(serde_json::json!({
        "expected_version": 1, "connector_override": null, "interval_override_hours": null
    }))
    .unwrap();
    assert!(matches!(clear.connector_override, PatchField::Null));
    assert!(matches!(clear.interval_override_hours, PatchField::Null));
    let value: PatchSourceCommand = serde_json::from_value(serde_json::json!({
        "expected_version": 1, "connector_override": "mp_admin", "interval_override_hours": 12
    }))
    .unwrap();
    assert!(matches!(value.connector_override, PatchField::Value(ref v) if v == "mp_admin"));
    assert!(matches!(
        value.interval_override_hours,
        PatchField::Value(12)
    ));
    let canonical: PatchSourceCommand = serde_json::from_value(serde_json::json!({
        "expected_version": 1,
        "enabled": false,
        "manual_next_heartbeat_at": "2030-01-02T03:04:05Z"
    }))
    .unwrap();
    let expected_manual = DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(canonical.enabled, Some(false));
    assert!(matches!(
        canonical.manual_next_heartbeat_at,
        PatchField::Value(value) if value == expected_manual
    ));
    let clear_manual: PatchSourceCommand = serde_json::from_value(serde_json::json!({
        "expected_version": 1,
        "manual_next_heartbeat_at": null
    }))
    .unwrap();
    assert!(matches!(
        clear_manual.manual_next_heartbeat_at,
        PatchField::Null
    ));
    assert!(
        serde_json::from_value::<PatchSourceCommand>(serde_json::json!({
            "expected_version": 1, "first_heartbeat_at": Utc::now()
        }))
        .is_err()
    );
}

#[tokio::test]
async fn aggregate_patch_updates_all_fields_without_mutating_first_anchor() {
    let h = setup().await;
    let (id, first) = configured_source(&h).await;
    let next = Utc::now() + Duration::hours(8);
    let updated = h
        .service
        .patch(
            id,
            PatchSourceCommand {
                expected_version: 1,
                display_name: Some("renamed".into()),
                preferred_connector: PatchField::Null,
                status: Some("paused".into()),
                base_level: Some("L3".into()),
                working_windows: Some(vec![WorkingWindow {
                    start: "10:00".into(),
                    end: "12:30".into(),
                }]),
                connector_override: PatchField::Value("mp_admin".into()),
                interval_override_hours: PatchField::Value(24),
                enabled: Some(false),
                manual_next_heartbeat_at: PatchField::Missing,
                next_heartbeat_at: Some(next),
                auto_promotion_enabled: Some(false),
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.version, 2);
    assert_eq!(updated.display_name, "renamed");
    assert_eq!(updated.preferred_connector, None);
    let rules = updated.fetch_rules.unwrap();
    assert_eq!(rules.base_level, SourceLevel::L3);
    assert_eq!(rules.connector_override.as_deref(), Some("mp_admin"));
    assert_eq!(rules.interval_override_hours, Some(24));
    assert_eq!(rules.manual_next_heartbeat_at, Some(next));
    assert!(!rules.enabled);
    assert!(!rules.auto_promotion_enabled);
    assert_eq!(rules.first_heartbeat_at, first);
}

#[tokio::test]
async fn canonical_manual_schedule_supports_clear_and_rejects_alias_conflicts() {
    let h = setup().await;
    let (id, first) = configured_source(&h).await;
    let next = Utc::now() + Duration::hours(8);
    let updated = h
        .service
        .patch(
            id,
            PatchSourceCommand {
                expected_version: 1,
                display_name: None,
                preferred_connector: PatchField::Missing,
                status: None,
                base_level: None,
                working_windows: None,
                connector_override: PatchField::Missing,
                interval_override_hours: PatchField::Missing,
                enabled: Some(false),
                manual_next_heartbeat_at: PatchField::Value(next),
                next_heartbeat_at: None,
                auto_promotion_enabled: None,
            },
        )
        .await
        .unwrap();
    let rules = updated.fetch_rules.unwrap();
    assert!(!rules.enabled);
    assert_eq!(rules.next_heartbeat_at, next);
    assert_eq!(rules.manual_next_heartbeat_at, Some(next));

    let cleared = h
        .service
        .patch(
            id,
            PatchSourceCommand {
                expected_version: 2,
                display_name: None,
                preferred_connector: PatchField::Missing,
                status: None,
                base_level: None,
                working_windows: None,
                connector_override: PatchField::Missing,
                interval_override_hours: PatchField::Missing,
                enabled: None,
                manual_next_heartbeat_at: PatchField::Null,
                next_heartbeat_at: None,
                auto_promotion_enabled: None,
            },
        )
        .await
        .unwrap();
    let rules = cleared.fetch_rules.unwrap();
    assert_eq!(rules.next_heartbeat_at, next);
    assert_eq!(rules.manual_next_heartbeat_at, None);
    assert_eq!(rules.first_heartbeat_at, first);

    let conflict = h
        .service
        .patch(
            id,
            PatchSourceCommand {
                expected_version: 3,
                display_name: None,
                preferred_connector: PatchField::Missing,
                status: None,
                base_level: None,
                working_windows: None,
                connector_override: PatchField::Missing,
                interval_override_hours: PatchField::Missing,
                enabled: None,
                manual_next_heartbeat_at: PatchField::Value(next + Duration::hours(1)),
                next_heartbeat_at: Some(next),
                auto_promotion_enabled: None,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "source.conflicting_next_heartbeat");
}

#[tokio::test]
async fn stale_and_concurrent_patches_have_exactly_one_winner() {
    let h = setup().await;
    let (id, _) = configured_source(&h).await;
    let command = |name: &str| PatchSourceCommand {
        expected_version: 1,
        display_name: Some(name.into()),
        preferred_connector: PatchField::Missing,
        status: None,
        base_level: None,
        working_windows: None,
        connector_override: PatchField::Missing,
        interval_override_hours: PatchField::Missing,
        enabled: None,
        manual_next_heartbeat_at: PatchField::Missing,
        next_heartbeat_at: None,
        auto_promotion_enabled: None,
    };
    let (a, b) = tokio::join!(
        h.service.patch(id, command("a")),
        h.service.patch(id, command("b"))
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = a.err().or_else(|| b.err()).unwrap();
    assert_eq!(error.code, "source.version_conflict");
    assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
    let stale = h.service.patch(id, command("stale")).await.unwrap_err();
    assert_eq!(stale.code, "source.version_conflict");
    let stored = h.service.get(id).await.unwrap();
    assert_eq!(stored.preferred_connector.as_deref(), Some("old-account"));
    let rules = stored.fetch_rules.unwrap();
    assert_eq!(rules.connector_override.as_deref(), Some("old-rule"));
    assert_eq!(rules.interval_override_hours, None);
}

#[tokio::test]
async fn patch_rejects_invalid_rule_inputs_without_incrementing_version() {
    let h = setup().await;
    let (id, _) = configured_source(&h).await;
    let invalid_inputs = [
        serde_json::json!({"expected_version": 1, "working_windows": [{"start":"18:00", "end":"09:00"}]}),
        serde_json::json!({"expected_version": 1, "working_windows": [{"start":"9:00", "end":"18:00"}]}),
        serde_json::json!({"expected_version": 1, "interval_override_hours": 0}),
        serde_json::json!({"expected_version": 1, "interval_override_hours": 8761}),
        serde_json::json!({"expected_version": 1, "connector_override": "   "}),
        serde_json::json!({"expected_version": 1, "base_level": "L9"}),
        serde_json::json!({"expected_version": 1, "next_heartbeat_at": Utc::now() - Duration::hours(1)}),
    ];
    for input in invalid_inputs {
        let invalid: PatchSourceCommand = serde_json::from_value(input).unwrap();
        assert!(h.service.patch(id, invalid).await.is_err());
    }
    assert_eq!(h.sources.get_source(id).await.unwrap().unwrap().version, 1);
}
