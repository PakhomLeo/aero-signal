//! M05 collector-login 集成测试（内存 repo + FakeConnector）。

use std::sync::Arc;
use std::time::Duration;

use platform_connectors::{
    ConnectorBackend, FakeConnector, NAME_FAKE, QrPollStatus, default_registry,
};
use server_core::{Clock, SystemClock};
use uuid::Uuid;

use signal_service::adapters::QrTicketStore;
use signal_service::adapters::qr_ticket_store::MemoryQrTicketStore;
use signal_service::adapters::secret_store::{MemorySecretStore, SecretStore};
use signal_service::audit::AuditWriter;
use signal_service::domain::BootstrapCredentials;
use signal_service::domain::collector::CollectorAccountStatus;
use signal_service::domain::egress::{
    EgressKind, EgressProfile, EgressPurpose, EgressStatus, LOCAL_DIRECT_PROFILE_ID,
};
use signal_service::repository::{
    AdministratorRepository, AuditRepository, EgressRepository, MemoryAdministratorStore,
    MemoryCollectorStore, MemoryEgressStore, SettingsRepository,
};
use signal_service::service::{
    AdministratorService, AdvanceQrSessionCommand, AuthSession, CreateQrSessionCommand,
    EgressService, LoginCommand, PatchCollectorAccountCommand, PatchMeCommand, QrLoginService,
    ReauthorizeCommand,
};

struct TestCtx {
    qr: Arc<QrLoginService>,
    fake: Arc<FakeConnector>,
    secrets: Arc<MemorySecretStore>,
    tickets: Arc<MemoryQrTicketStore>,
    auth_a: AuthSession,
    auth_b: AuthSession,
    #[allow(dead_code)]
    admin: Arc<AdministratorService>,
}

async fn setup() -> TestCtx {
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let store = Arc::new(MemoryAdministratorStore::new());
    let admin = Arc::new(AdministratorService::new(
        store.clone() as Arc<dyn AdministratorRepository>,
        store.clone() as Arc<dyn SettingsRepository>,
        AuditWriter::new(store.clone() as Arc<dyn AuditRepository>),
        clock.clone(),
        Duration::from_secs(12 * 60 * 60),
    ));
    admin
        .bootstrap(BootstrapCredentials {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
        })
        .await
        .unwrap();
    // 改密以通过 ensure_password_changed（服务层测试不走 HTTP）
    let login = admin
        .login(LoginCommand {
            username: "admin".into(),
            password: "Bootstrap9!".into(),
            ip: None,
            user_agent: None,
        })
        .await
        .unwrap();
    let auth_tmp = admin
        .sessions()
        .authenticate_token(login.token.as_str())
        .await
        .unwrap();
    admin
        .patch_me(
            &auth_tmp,
            PatchMeCommand {
                username: None,
                current_password: Some("Bootstrap9!".into()),
                new_password: Some("NewSecure1!".into()),
            },
        )
        .await
        .unwrap();

    let login_a = admin
        .login(LoginCommand {
            username: "admin".into(),
            password: "NewSecure1!".into(),
            ip: None,
            user_agent: None,
        })
        .await
        .unwrap();
    let auth_a = admin
        .sessions()
        .authenticate_token(login_a.token.as_str())
        .await
        .unwrap();

    let login_b = admin
        .login(LoginCommand {
            username: "admin".into(),
            password: "NewSecure1!".into(),
            ip: None,
            user_agent: None,
        })
        .await
        .unwrap();
    let auth_b = admin
        .sessions()
        .authenticate_token(login_b.token.as_str())
        .await
        .unwrap();

    let egress_store = Arc::new(MemoryEgressStore::with_local_direct());
    // 额外固定出口，供变更测试
    let now = chrono::Utc::now();
    let egress_repo: Arc<dyn EgressRepository> = egress_store.clone();
    egress_repo
        .insert_egress_profile(&EgressProfile {
            id: Uuid::parse_str("b0000000-0000-4000-8000-000000000002").unwrap(),
            name: "fixed-login-2".into(),
            kind: EgressKind::Fixed,
            provider_id: None,
            endpoint_secret_ref: Some("sec_placeholder".into()),
            priority: 10,
            allowed_purposes: vec![EgressPurpose::Login, EgressPurpose::Discover],
            status: EgressStatus::Active,
            last_preflight_status: None,
            last_preflight_at: None,
            created_at: now,
            updated_at: now,
        })
        .await
        .unwrap();

    let secrets = Arc::new(MemorySecretStore::new());
    let egress = Arc::new(EgressService::new(
        egress_repo,
        secrets.clone() as Arc<dyn signal_service::adapters::SecretStore>,
        AuditWriter::new(store as Arc<dyn AuditRepository>),
        clock.clone(),
    ));

    let fake = Arc::new(FakeConnector::new());
    let mut registry = platform_connectors::ConnectorRegistry::new();
    registry.register(fake.clone());
    // 也注册 default 中的其他（可选）
    let _ = default_registry();

    let tickets = Arc::new(MemoryQrTicketStore::new());
    let collectors = Arc::new(MemoryCollectorStore::new());
    let qr = Arc::new(QrLoginService::new(
        tickets.clone() as Arc<dyn QrTicketStore>,
        collectors as Arc<dyn signal_service::repository::CollectorRepository>,
        secrets.clone() as Arc<dyn signal_service::adapters::SecretStore>,
        Arc::new(registry),
        egress,
        clock,
    ));

    TestCtx {
        qr,
        fake,
        secrets,
        tickets,
        auth_a,
        auth_b,
        admin,
    }
}

#[tokio::test]
async fn collector_qr_happy_path_to_active() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    assert_eq!(created.status, "qr_waiting");
    assert!(created.qr_data_url.starts_with("data:"));
    assert!(!created.qr_data_url.contains("cookie"));

    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Scanned)
        .unwrap();
    let scanned = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(scanned.status, "qr_scanned");

    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Confirmed)
        .unwrap();
    let confirmed = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(confirmed.status, "active");
    let account = confirmed.account.expect("应有账号摘要");
    assert_eq!(account.status, CollectorAccountStatus::Active);
    assert_eq!(account.connector, NAME_FAKE);
    assert!(!format!("{account:?}").to_lowercase().contains("cookie"));

    // 幂等：再 poll 同一摘要，不重复存
    let again = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(again.status, "active");
    assert_eq!(again.account.as_ref().unwrap().id, account.id);

    let list = ctx.qr.list_accounts().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, account.id);
}

#[tokio::test]
async fn collector_provider_state_survives_connector_recreation() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Scanned)
        .unwrap();
    let scanned = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(scanned.status, "qr_scanned");

    let ticket = ctx.tickets.get(&created.ticket).await.unwrap().unwrap();
    let state_ref = ticket.provider_state_secret_ref.expect("state ref");
    let state = ctx.secrets.get(&state_ref).await.unwrap().expect("state");
    let recreated = FakeConnector::new();
    let restored = recreated
        .poll_qr_session(&created.ticket, Some(&state))
        .await
        .unwrap();
    assert_eq!(restored.status, QrPollStatus::Scanned);
    assert!(!format!("{restored:?}").contains(&state));
}

#[tokio::test]
async fn collector_cross_session_ticket_theft_fails() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();

    let err = ctx
        .qr
        .poll_qr_session(&ctx.auth_b, &created.ticket)
        .await
        .unwrap_err();
    assert_eq!(err.code, "auth.forbidden");
}

#[tokio::test]
async fn collector_cancel_cleans_temp_secret() {
    let ctx = setup().await;
    let before = ctx.secrets.len().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    assert!(ctx.secrets.len().await > before);

    let cancelled = ctx
        .qr
        .cancel_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(cancelled.status, "cancelled");
    assert!(ctx.tickets.get(&created.ticket).await.unwrap().is_none());
}

#[tokio::test]
async fn collector_egress_change_needs_relogin() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Confirmed)
        .unwrap();
    let confirmed = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    let id = confirmed.account.unwrap().id;

    let other = Uuid::parse_str("b0000000-0000-4000-8000-000000000002").unwrap();
    let patched = ctx
        .qr
        .patch_account(
            id,
            PatchCollectorAccountCommand {
                display_name: None,
                status: None,
                fixed_egress_id: Some(other),
            },
        )
        .await
        .unwrap();
    assert_eq!(patched.status, CollectorAccountStatus::NeedsRelogin);
    assert!(patched.requires_relogin);
    assert_eq!(patched.fixed_egress_id, Some(other));
}

#[tokio::test]
async fn collector_idempotent_finalize_single_account() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Confirmed)
        .unwrap();

    let a = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    let b = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(
        a.account.as_ref().unwrap().id,
        b.account.as_ref().unwrap().id
    );
    assert_eq!(ctx.qr.list_accounts().await.unwrap().len(), 1);
}

#[tokio::test]
async fn collector_reauthorize_marks_needs_relogin() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Confirmed)
        .unwrap();
    let id = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap()
        .account
        .unwrap()
        .id;

    let r = ctx
        .qr
        .reauthorize(
            id,
            ReauthorizeCommand {
                fixed_egress_id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(r.status, CollectorAccountStatus::NeedsRelogin);
    assert!(r.requires_relogin);
}

#[tokio::test]
async fn collector_failed_probe_cleans_secrets() {
    let ctx = setup().await;
    ctx.fake
        .set_probe_behavior(platform_connectors::FakeProbeBehavior::Failed {
            message: "boom".into(),
        });
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();
    ctx.fake
        .advance_qr(&created.ticket, QrPollStatus::Confirmed)
        .unwrap();
    let resp = ctx
        .qr
        .poll_qr_session(&ctx.auth_a, &created.ticket)
        .await
        .unwrap();
    assert_eq!(resp.status, "failed");
    assert!(ctx.qr.list_accounts().await.unwrap().is_empty());
}

#[tokio::test]
async fn collector_advance_qr_session_happy_path() {
    let ctx = setup().await;
    let created = ctx
        .qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: NAME_FAKE.into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap();

    let scanned = ctx
        .qr
        .advance_qr_session(
            &ctx.auth_a,
            &created.ticket,
            AdvanceQrSessionCommand {
                status: "scanned".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(scanned.status, "qr_scanned");

    let confirmed = ctx
        .qr
        .advance_qr_session(
            &ctx.auth_a,
            &created.ticket,
            AdvanceQrSessionCommand {
                status: "confirmed".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status, "active");
    assert!(confirmed.account.is_some());
}

#[tokio::test]
async fn collector_advance_rejects_non_fake_connector() {
    let ctx = setup().await;
    // 未启用真实协议时，不得创建可误解为可用的占位二维码。
    let mut registry = platform_connectors::ConnectorRegistry::new();
    let fake = Arc::new(FakeConnector::new());
    registry.register(fake.clone());
    registry.register(Arc::new(platform_connectors::MpAdminConnector::new()));

    let tickets = Arc::new(MemoryQrTicketStore::new());
    let collectors = Arc::new(MemoryCollectorStore::new());
    let secrets = Arc::new(MemorySecretStore::new());
    let clock = Arc::new(SystemClock) as Arc<dyn Clock>;
    let egress_store = Arc::new(MemoryEgressStore::with_local_direct());
    let admin_store = Arc::new(MemoryAdministratorStore::new());
    let egress = Arc::new(EgressService::new(
        egress_store as Arc<dyn EgressRepository>,
        secrets.clone() as Arc<dyn signal_service::adapters::SecretStore>,
        AuditWriter::new(admin_store as Arc<dyn AuditRepository>),
        clock.clone(),
    ));
    let qr = Arc::new(QrLoginService::new(
        tickets as Arc<dyn QrTicketStore>,
        collectors as Arc<dyn signal_service::repository::CollectorRepository>,
        secrets as Arc<dyn signal_service::adapters::SecretStore>,
        Arc::new(registry),
        egress,
        clock,
    ));

    let err = qr
        .create_qr_session(
            &ctx.auth_a,
            CreateQrSessionCommand {
                connector: "mp_admin".into(),
                fixed_egress_id: LOCAL_DIRECT_PROFILE_ID,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "connector.unsupported");
}
