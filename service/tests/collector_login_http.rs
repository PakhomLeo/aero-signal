//! Fake QR advance HTTP 闭环：create → advance(scanned/confirmed) → active。

use std::sync::Arc;
use std::time::Duration;

use http_body_util::BodyExt;
use job_pipeline::MemoryRepo;
use serde_json::json;
use server_core::{Clock, ServiceConfig, SignalEnvironment, SystemClock};
use signal_service::adapters::QrTicketStore;
use signal_service::adapters::qr_ticket_store::MemoryQrTicketStore;
use signal_service::adapters::secret_store::MemorySecretStore;
use signal_service::audit::AuditWriter;
use signal_service::build_router_with_collector;
use signal_service::domain::BootstrapCredentials;
use signal_service::domain::egress::LOCAL_DIRECT_PROFILE_ID;
use signal_service::repository::{
    AdministratorRepository, AuditRepository, MemoryAdministratorStore, MemoryCollectorStore,
    MemoryEgressStore, SettingsRepository,
};
use signal_service::service::{
    AdministratorService, EgressService, LoginCommand, PatchMeCommand, QrLoginService,
};
use tower::ServiceExt;

async fn bootstrap_admin_token(admin: &AdministratorService) -> String {
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
                username: None,
                current_password: Some("Bootstrap9!".into()),
                new_password: Some("NewSecure1!".into()),
            },
        )
        .await
        .unwrap();
    // 改密后重新登录，拿到 must_change_password=false 的会话
    let login2 = admin
        .login(LoginCommand {
            username: "admin".into(),
            password: "NewSecure1!".into(),
            ip: None,
            user_agent: None,
        })
        .await
        .unwrap();
    login2.token.into_inner()
}

async fn json_body(resp: axum::response::Response) -> serde_json::Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}))
}

struct HttpHarness {
    app: axum::Router,
    token: String,
}

async fn harness() -> HttpHarness {
    harness_for_environment(SignalEnvironment::Test).await
}

async fn harness_for_environment(environment: SignalEnvironment) -> HttpHarness {
    let store = Arc::new(MemoryAdministratorStore::new());
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let admin = Arc::new(AdministratorService::new(
        store.clone() as Arc<dyn AdministratorRepository>,
        store.clone() as Arc<dyn SettingsRepository>,
        AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
        Duration::from_secs(3600),
    ));
    let token = bootstrap_admin_token(&admin).await;

    let secrets = Arc::new(MemorySecretStore::new());
    let egress = Arc::new(EgressService::new(
        Arc::new(MemoryEgressStore::with_local_direct())
            as Arc<dyn signal_service::repository::EgressRepository>,
        secrets.clone() as Arc<dyn signal_service::adapters::SecretStore>,
        AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
    ));

    let registry = Arc::new(platform_connectors::default_registry());
    let collector = Arc::new(QrLoginService::new(
        Arc::new(MemoryQrTicketStore::new()) as Arc<dyn QrTicketStore>,
        Arc::new(MemoryCollectorStore::new())
            as Arc<dyn signal_service::repository::CollectorRepository>,
        secrets as Arc<dyn signal_service::adapters::SecretStore>,
        registry,
        egress.clone(),
        clock,
    ));

    let mut config = ServiceConfig::for_test();
    config.environment = environment;
    let app = build_router_with_collector(
        &config,
        admin,
        egress,
        Arc::new(MemoryRepo::new()),
        Some(collector),
    );
    HttpHarness { app, token }
}

#[tokio::test]
async fn production_router_does_not_mount_fake_advance_endpoint() {
    let h = harness_for_environment(SignalEnvironment::Production).await;
    let (status, _) = admin_json(
        &h.app,
        &h.token,
        "POST",
        "/api/admin/collector-accounts/qr-sessions/not-a-ticket/advance",
        Some(json!({ "status": "scanned" })),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
}

async fn admin_json(
    app: &axum::Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (axum::http::StatusCode, serde_json::Value) {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    let req = if let Some(b) = body {
        builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        builder.body(axum::body::Body::from(b.to_string())).unwrap()
    } else {
        builder.body(axum::body::Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let json = json_body(resp).await;
    (status, json)
}

#[tokio::test]
async fn http_fake_qr_advance_to_active() {
    let h = harness().await;

    let (st, created) = admin_json(
        &h.app,
        &h.token,
        "POST",
        "/api/admin/collector-accounts/qr-sessions",
        Some(json!({
            "connector": "fake",
            "fixed_egress_id": LOCAL_DIRECT_PROFILE_ID,
        })),
    )
    .await;
    assert_eq!(st, axum::http::StatusCode::OK, "{created}");
    assert_eq!(created["status"], "qr_waiting");
    let ticket = created["ticket"].as_str().expect("ticket");

    let (st, scanned) = admin_json(
        &h.app,
        &h.token,
        "POST",
        &format!("/api/admin/collector-accounts/qr-sessions/{ticket}/advance"),
        Some(json!({ "status": "scanned" })),
    )
    .await;
    assert_eq!(st, axum::http::StatusCode::OK, "{scanned}");
    assert_eq!(scanned["status"], "qr_scanned");

    let (st, confirmed) = admin_json(
        &h.app,
        &h.token,
        "POST",
        &format!("/api/admin/collector-accounts/qr-sessions/{ticket}/advance"),
        Some(json!({ "status": "confirmed" })),
    )
    .await;
    assert_eq!(st, axum::http::StatusCode::OK, "{confirmed}");
    assert_eq!(confirmed["status"], "active");
    assert!(confirmed["account"].is_object());
    assert_eq!(confirmed["account"]["connector"], "fake");
}

#[tokio::test]
async fn http_mp_admin_disabled_does_not_return_placeholder_qr() {
    let h = harness().await;

    let (st, created) = admin_json(
        &h.app,
        &h.token,
        "POST",
        "/api/admin/collector-accounts/qr-sessions",
        Some(json!({
            "connector": "mp_admin",
            "fixed_egress_id": LOCAL_DIRECT_PROFILE_ID,
        })),
    )
    .await;
    assert_eq!(st, axum::http::StatusCode::BAD_REQUEST, "{created}");
    assert_eq!(created["error"]["code"], "connector.unsupported");
    assert!(!created.to_string().contains("data:image"));
}
