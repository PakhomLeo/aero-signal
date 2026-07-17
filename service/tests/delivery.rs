//! M13 delivery 契约测试：Token、scope、组内隔离、since 重放、健康脱敏。

use std::sync::Arc;
use std::time::Duration;

use chrono::{NaiveDate, Utc};
use http_body_util::BodyExt;
use job_pipeline::MemoryRepo;
use serde_json::json;
use server_core::{Clock, ServiceConfig, SystemClock};
use signal_service::adapters::secret_store::MemorySecretStore;
use signal_service::audit::AuditWriter;
use signal_service::build_router_with_delivery;
use signal_service::domain::BootstrapCredentials;
use signal_service::domain::article::{Article, ArticleStatus, ChangeType, EditorKind};
use signal_service::domain::delivery::ApiScope;
use signal_service::domain::health::{
    HealthComponentStatus, HealthComponentType, OverallHealthStatus, ServiceHealthComponent,
    ServiceHealthSnapshot,
};
use signal_service::domain::source::SourceStatus;
use signal_service::repository::article::{ArticleRepository, CommitRevisionCommand};
use signal_service::repository::health::HealthSnapshotRepository;
use signal_service::repository::source::{CreateSourceAccount, SourceRepository};
use signal_service::repository::{
    AdministratorRepository, AuditRepository, DeliveryRepository, MemoryAdministratorStore,
    MemoryArticleStore, MemoryDeliveryStore, MemoryEgressStore, MemoryHealthStore,
    MemorySourceStore, SettingsRepository,
};
use signal_service::service::{
    AdministratorService, ApiTokenService, ArticleCursorService, CreateGroupCommand,
    DeliveryGroupService, EgressService, HealthDeliveryService, IssueTokenCommand, LoginCommand,
    PatchMeCommand,
};
use tower::ServiceExt;
use uuid::Uuid;

async fn bootstrap_admin(admin: &AdministratorService) -> String {
    admin
        .bootstrap(BootstrapCredentials {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
        })
        .await
        .unwrap();
    let login = admin
        .login(LoginCommand {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
            ip: None,
            user_agent: None,
        })
        .await
        .unwrap();
    let auth = admin
        .sessions()
        .authenticate_token(login.token.as_str())
        .await
        .unwrap();
    admin
        .patch_me(
            &auth,
            PatchMeCommand {
                current_password: Some("Bootstrap9!".into()),
                new_password: Some("NewSecure1!".into()),
                username: None,
            },
        )
        .await
        .unwrap();
    login.token.into_inner()
}

struct Harness {
    app: axum::Router,
    articles: Arc<MemoryArticleStore>,
    sources: Arc<MemorySourceStore>,
    delivery: Arc<MemoryDeliveryStore>,
    health: Arc<MemoryHealthStore>,
    groups: Arc<DeliveryGroupService>,
    tokens: Arc<ApiTokenService>,
    admin_token: String,
}

async fn harness() -> Harness {
    let store = Arc::new(MemoryAdministratorStore::new());
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let admin = Arc::new(AdministratorService::new(
        store.clone() as Arc<dyn AdministratorRepository>,
        store.clone() as Arc<dyn SettingsRepository>,
        AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
        Duration::from_secs(3600),
    ));
    let admin_token = bootstrap_admin(&admin).await;
    let egress = Arc::new(EgressService::new(
        Arc::new(MemoryEgressStore::with_local_direct())
            as Arc<dyn signal_service::repository::EgressRepository>,
        Arc::new(MemorySecretStore::new()) as Arc<dyn signal_service::adapters::SecretStore>,
        AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
    ));
    let articles = Arc::new(MemoryArticleStore::new());
    let sources = Arc::new(MemorySourceStore::new());
    let delivery = Arc::new(MemoryDeliveryStore::new());
    let health = Arc::new(MemoryHealthStore::new());
    let groups = Arc::new(DeliveryGroupService::new(
        delivery.clone() as Arc<dyn DeliveryRepository>,
        sources.clone() as Arc<dyn SourceRepository>,
        clock.clone(),
    ));
    let tokens = Arc::new(ApiTokenService::new(
        delivery.clone() as Arc<dyn DeliveryRepository>,
        clock.clone(),
    ));
    let cursor = Arc::new(ArticleCursorService::new(
        articles.clone() as Arc<dyn ArticleRepository>,
        delivery.clone() as Arc<dyn DeliveryRepository>,
    ));
    let health_delivery = Arc::new(HealthDeliveryService::new(
        health.clone() as Arc<dyn HealthSnapshotRepository>
    ));
    let config = ServiceConfig::for_test();
    let app = build_router_with_delivery(
        &config,
        admin,
        egress,
        Arc::new(MemoryRepo::new()),
        None,
        None,
        None,
        None,
        None,
        Some(groups.clone()),
        Some(tokens.clone()),
        Some(cursor),
        Some(health_delivery),
    );
    Harness {
        app,
        articles,
        sources,
        delivery,
        health,
        groups,
        tokens,
        admin_token,
    }
}

async fn seed_source(h: &Harness, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now();
    h.sources
        .insert_source(CreateSourceAccount {
            id,
            display_name: name.into(),
            status: SourceStatus::Active,
            preferred_connector: Some("fake".into()),
            now,
        })
        .await
        .unwrap();
    h.articles.ensure_source_stub(id, name).await.unwrap();
    id
}

async fn seed_article(h: &Harness, source_id: Uuid, title: &str) -> Uuid {
    let id = Uuid::new_v4();
    let now = Utc::now();
    let article = Article {
        id,
        source_account_id: source_id,
        article_link_id: None,
        original_url: format!("https://example.com/{id}"),
        canonical_url: format!("https://example.com/{id}"),
        original_title: title.into(),
        author: Some("author".into()),
        published_at: Some(now),
        fetched_at: now,
        current_revision: 0,
        content_md: format!("# {title}"),
        content_hash: format!("hash-{id}"),
        asset_manifest_hash: None,
        status: ArticleStatus::Ready,
        withdrawn_at: None,
    };
    h.articles.insert_article(article).await.unwrap();
    h.articles
        .commit_revision(CommitRevisionCommand {
            article_id: id,
            editor_kind: EditorKind::Crawler,
            content_md: format!("# {title}"),
            content_hash: format!("hash-{id}"),
            asset_manifest_hash: None,
            change_reason: "initial".into(),
            change_type: ChangeType::ArticleCreated,
            status: ArticleStatus::Ready,
            asset_refs: vec![],
            now,
            force_new_revision: true,
        })
        .await
        .unwrap();
    id
}

async fn issue_token(
    h: &Harness,
    group_id: Uuid,
    scopes: Vec<ApiScope>,
    ip_allowlist: Vec<String>,
) -> (String, Uuid) {
    let issued = h
        .tokens
        .issue(IssueTokenCommand {
            delivery_group_id: group_id,
            name: "test".into(),
            scopes,
            ip_allowlist,
            expires_at: None,
        })
        .await
        .unwrap();
    (issued.plaintext.into_inner(), issued.token.id)
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(json!({}))
}

#[tokio::test]
async fn plaintext_token_only_once_and_hash_stored() {
    let h = harness().await;
    let src = seed_source(&h, "公众号A").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "组1".into(),
            description: None,
            source_account_ids: vec![src],
        })
        .await
        .unwrap();
    let (plain, token_id) = issue_token(&h, group.id, vec![ApiScope::ArticlesRead], vec![]).await;
    assert!(plain.starts_with("sig_"));
    assert!(
        ApiTokenService::reissue_plaintext_impossible(
            &h.delivery.get_token(token_id).await.unwrap().unwrap()
        )
        .is_none()
    );
    let stored = h.delivery.get_token(token_id).await.unwrap().unwrap();
    assert_ne!(stored.token_hash, plain);
    assert!(!stored.token_hash.is_empty());
}

#[tokio::test]
async fn scope_denied_returns_token_scope_denied() {
    let h = harness().await;
    let src = seed_source(&h, "公众号A").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "组scope".into(),
            description: None,
            source_account_ids: vec![src],
        })
        .await
        .unwrap();
    let (plain, _) = issue_token(&h, group.id, vec![ApiScope::SourcesRead], vec![]).await;
    let resp = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/articles")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    let v = json_body(resp).await;
    assert_eq!(v["error"]["code"], "token.scope_denied");
}

#[tokio::test]
async fn ip_allowlist_denied() {
    let h = harness().await;
    let src = seed_source(&h, "公众号A").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "组ip".into(),
            description: None,
            source_account_ids: vec![src],
        })
        .await
        .unwrap();
    let (plain, _) = issue_token(
        &h,
        group.id,
        vec![ApiScope::ArticlesRead],
        vec!["10.0.0.1".into()],
    )
    .await;
    let resp = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/articles")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .header("x-forwarded-for", "10.0.0.99")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);
    let v = json_body(resp).await;
    assert_eq!(v["error"]["code"], "token.ip_denied");
}

#[tokio::test]
async fn group_isolation_on_article_detail() {
    let h = harness().await;
    let src_a = seed_source(&h, "A").await;
    let src_b = seed_source(&h, "B").await;
    let art_a = seed_article(&h, src_a, "文章A").await;
    let art_b = seed_article(&h, src_b, "文章B").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "仅A".into(),
            description: None,
            source_account_ids: vec![src_a],
        })
        .await
        .unwrap();
    let (plain, _) = issue_token(&h, group.id, vec![ApiScope::ArticlesRead], vec![]).await;

    let ok = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/articles/{art_a}"))
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), axum::http::StatusCode::OK);

    let denied = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/articles/{art_b}"))
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), axum::http::StatusCode::NOT_FOUND);
    let v = json_body(denied).await;
    assert_eq!(v["error"]["code"], "article.not_in_group");
}

#[tokio::test]
async fn since_replay_idempotent_and_withdrawal_tombstone() {
    let h = harness().await;
    let src = seed_source(&h, "A").await;
    let art = seed_article(&h, src, "可撤回").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "since组".into(),
            description: None,
            source_account_ids: vec![src],
        })
        .await
        .unwrap();
    let (plain, _) = issue_token(&h, group.id, vec![ApiScope::ArticlesRead], vec![]).await;

    // 编辑产生新事件
    let now = Utc::now();
    h.articles
        .commit_revision(CommitRevisionCommand {
            article_id: art,
            editor_kind: EditorKind::Administrator,
            content_md: "# 编辑后".into(),
            content_hash: "hash-edited".into(),
            asset_manifest_hash: None,
            change_reason: "edit".into(),
            change_type: ChangeType::ArticleEdited,
            status: ArticleStatus::Ready,
            asset_refs: vec![],
            now,
            force_new_revision: true,
        })
        .await
        .unwrap();
    // 撤回
    h.articles
        .commit_revision(CommitRevisionCommand {
            article_id: art,
            editor_kind: EditorKind::Administrator,
            content_md: "".into(),
            content_hash: "hash-withdrawn".into(),
            asset_manifest_hash: None,
            change_reason: "withdraw".into(),
            change_type: ChangeType::ArticleWithdrawn,
            status: ArticleStatus::Withdrawn,
            asset_refs: vec![],
            now,
            force_new_revision: true,
        })
        .await
        .unwrap();

    let resp1 = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/articles/since?after=0&limit=50")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp1.status(), axum::http::StatusCode::OK);
    let v1 = json_body(resp1).await;
    let events1 = v1["events"].as_array().unwrap();
    assert!(events1.len() >= 2);
    let event_ids: Vec<_> = events1
        .iter()
        .map(|e| e["event_id"].as_str().unwrap().to_string())
        .collect();
    let withdrawn = events1
        .iter()
        .find(|e| e["change_type"] == "article_withdrawn")
        .expect("应有撤回事件");
    assert_eq!(withdrawn["payload"]["tombstone"], true);

    // 重放同一 cursor：相同 event_id 集合（幂等）
    let resp2 = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/articles/since?after=0&limit=50")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let v2 = json_body(resp2).await;
    let event_ids2: Vec<_> = v2["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event_id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(event_ids, event_ids2);

    // 推进 cursor 后不再返回已消费事件
    let next = v1["next_sequence"].as_i64().unwrap();
    let resp3 = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/articles/since?after={next}&limit=50"))
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let v3 = json_body(resp3).await;
    assert!(v3["events"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn service_health_redacted_and_no_component_id() {
    let h = harness().await;
    let src = seed_source(&h, "A").await;
    let group = h
        .groups
        .create(CreateGroupCommand {
            name: "健康组".into(),
            description: None,
            source_account_ids: vec![src],
        })
        .await
        .unwrap();
    let (plain, _) = issue_token(&h, group.id, vec![ApiScope::ServiceHealthRead], vec![]).await;

    let snap_id = Uuid::new_v4();
    let date = NaiveDate::from_ymd_opt(2026, 7, 9).unwrap();
    let now = Utc::now();
    h.health
        .insert_snapshot(
            &ServiceHealthSnapshot {
                id: snap_id,
                snapshot_date: date,
                generated_at: now,
                overall_status: OverallHealthStatus::Healthy,
                yesterday_plan_success_count: 1,
                yesterday_plan_failure_count: 0,
                yesterday_article_failure_count: 0,
                today_eligible_source_count: 1,
                today_blocked_source_count: 0,
                pending_action_count: 0,
                summary_json: json!({
                    "cookie": "SECRET",
                    "proxy_url": "http://proxy",
                    "ok": true
                }),
            },
            &[ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: snap_id,
                component_type: HealthComponentType::Collector,
                component_id: Some(Uuid::new_v4()),
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: now,
                safe_details_json: json!({
                    "token": "abc",
                    "status": "ok",
                    "raw_error": "stacktrace"
                }),
            }],
        )
        .await
        .unwrap();

    let resp = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/service-health/latest")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let v = json_body(resp).await;
    assert_eq!(v["snapshot_date"], "2026-07-09");
    assert!(v["summary"].get("cookie").is_none());
    assert!(v["summary"].get("proxy_url").is_none());
    assert_eq!(v["summary"]["ok"], true);
    let comp = &v["components"][0];
    assert!(comp.get("component_id").is_none());
    assert!(comp["safe_details"].get("token").is_none());
    assert!(comp["safe_details"].get("raw_error").is_none());
    assert_eq!(comp["safe_details"]["status"], "ok");

    let by_date = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/service-health?date=2026-07-09")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {plain}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(by_date.status(), axum::http::StatusCode::OK);
}

#[tokio::test]
async fn admin_session_cannot_access_delivery_api() {
    let h = harness().await;
    let resp = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri("/api/articles")
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", h.admin_token),
                )
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn admin_can_create_group_and_issue_token() {
    let h = harness().await;
    let src = seed_source(&h, "A").await;
    let resp = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/admin/delivery-groups")
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", h.admin_token),
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({
                        "name": "管理创建组",
                        "source_account_ids": [src]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let group = json_body(resp).await;
    let group_id = group["id"].as_str().unwrap();

    let tok = h
        .app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(format!("/api/admin/delivery-groups/{group_id}/tokens"))
                .header(
                    axum::http::header::AUTHORIZATION,
                    format!("Bearer {}", h.admin_token),
                )
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    json!({
                        "name": "t1",
                        "scopes": ["articles:read", "service_health:read"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(tok.status(), axum::http::StatusCode::OK);
    let tv = json_body(tok).await;
    assert!(tv["plaintext"].as_str().unwrap().starts_with("sig_"));
    assert!(tv["token"]["token_prefix"].as_str().is_some());
}

#[tokio::test]
async fn delivery_group_create_rolls_back_metadata_and_bindings_on_commit_failure() {
    let h = harness().await;
    let source_id = seed_source(&h, "atomic-create").await;
    h.delivery.fail_next_atomic_group_write().await;

    let error = h
        .groups
        .create(CreateGroupCommand {
            name: "atomic-create-group".into(),
            description: None,
            source_account_ids: vec![source_id],
        })
        .await
        .expect_err("injected aggregate failure must be returned");
    assert_eq!(error.code, "server.internal");
    assert!(h.delivery.list_groups().await.unwrap().is_empty());
}

#[tokio::test]
async fn delivery_group_patch_rolls_back_metadata_and_bindings_on_commit_failure() {
    let h = harness().await;
    let source_id = seed_source(&h, "atomic-patch").await;
    let created = h
        .groups
        .create(CreateGroupCommand {
            name: "atomic-patch-group".into(),
            description: Some("before".into()),
            source_account_ids: vec![source_id],
        })
        .await
        .unwrap();
    h.delivery.fail_next_atomic_group_write().await;

    let error = h
        .groups
        .patch(
            created.id,
            signal_service::service::PatchGroupCommand {
                name: Some("after".into()),
                description: Some(Some("after".into())),
                status: None,
                source_account_ids: Some(Vec::new()),
            },
        )
        .await
        .expect_err("injected aggregate failure must be returned");
    assert_eq!(error.code, "server.internal");

    let restored = h.groups.get(created.id).await.unwrap();
    assert_eq!(restored.name, "atomic-patch-group");
    assert_eq!(restored.description.as_deref(), Some("before"));
    assert_eq!(restored.source_account_ids, vec![source_id]);
}
