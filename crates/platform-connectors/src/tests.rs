//! platform-connectors 单元测试：Fake / Selector / unsupported / 未授权枚举。

use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;

use crate::error::codes;
use crate::fake::{FakeConnector, FakeProbeBehavior, assert_probe_sanitized};
use crate::mp_admin::MpAdminConnector;
use crate::probe_recorder::{MemoryProbeRecorder, ProbeRecorder};
use crate::registry::ConnectorRegistry;
use crate::selector::ConnectorSelector;
use crate::types::{
    CapabilitySet, CollectorAccountView, ConnectorCapability, DiscoverRequest, LoginContext,
    NAME_FAKE, NAME_MP_ADMIN, NAME_WEREAD, ProbeStatus, QrPollStatus, SelectRequest,
    SelectionResult, SourceRef,
};
use crate::weread::WereadConnector;
use crate::{ConnectorBackend, default_registry};

fn account(connector: &str) -> CollectorAccountView {
    CollectorAccountView {
        id: Uuid::new_v4(),
        connector: connector.to_string(),
        display_name: Some("测试账号".into()),
        active: true,
        needs_relogin: false,
        fixed_egress_healthy: true,
        fixed_egress_id: None,
        cooling: false,
        budget_exhausted: false,
        enabled_capabilities: CapabilitySet::new([
            ConnectorCapability::QrLogin,
            ConnectorCapability::DiscoverRecent,
            ConnectorCapability::FetchFullArticle,
        ]),
        authorized_mp_ids: vec!["biz_authorized".into()],
    }
}

#[tokio::test]
async fn fake_probe_and_discover_respects_limit() {
    let fake = FakeConnector::new();
    let now = Utc::now();
    let acc = account(NAME_FAKE);

    let probe = fake.probe(&acc, None, now).await.unwrap();
    assert_eq!(probe.status, ProbeStatus::Healthy);
    assert!(assert_probe_sanitized(&probe).is_ok());

    let found = fake
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "biz_authorized".into(),
                level_hint: None,
            },
            account: acc,
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 3);
    for c in &found {
        assert!(!c.canonical_url.contains("token="));
        assert!(!c.canonical_url.contains("cookie="));
    }
}

#[tokio::test]
async fn fake_qr_state_machine() {
    let fake = FakeConnector::new();
    let session = fake
        .create_qr_session(LoginContext {
            ticket: "t-1".into(),
            fixed_egress_id: None,
            administrator_session_hash: "hash".into(),
            proxy_url: None,
        })
        .await
        .unwrap();
    assert_eq!(session.ticket, "t-1");

    let poll = fake.poll_qr_session("t-1", None).await.unwrap();
    assert_eq!(poll.status, QrPollStatus::Waiting);

    fake.advance_qr("t-1", QrPollStatus::Scanned).unwrap();
    assert_eq!(
        fake.poll_qr_session("t-1", None).await.unwrap().status,
        QrPollStatus::Scanned
    );

    fake.advance_qr("t-1", QrPollStatus::Confirmed).unwrap();
    let cred = fake.finalize_login("t-1", None).await.unwrap();
    assert!(cred.secret_ref.starts_with("secret:"));
}

#[tokio::test]
async fn fake_degraded_probe_is_sanitized() {
    let fake = FakeConnector::new();
    fake.set_probe_behavior(FakeProbeBehavior::Degraded {
        reason_code: "connector.degraded".into(),
        message: "平台限流，稍后重试".into(),
    });
    let probe = fake
        .probe(&account(NAME_FAKE), None, Utc::now())
        .await
        .unwrap();
    assert_eq!(probe.status, ProbeStatus::Degraded);
    assert_eq!(probe.reason_code.as_deref(), Some("connector.degraded"));
    assert!(assert_probe_sanitized(&probe).is_ok());
}

#[test]
fn registry_exposes_versioned_capabilities() {
    let registry = default_registry();
    assert!(registry.len() >= 3);

    let fake = registry.require(NAME_FAKE).unwrap();
    assert_eq!(fake.version(), "0.1.0-fake");
    assert!(
        fake.capabilities()
            .contains(ConnectorCapability::DiscoverRecent)
    );

    let desc = registry.list();
    assert!(desc.iter().any(|d| d.name == NAME_WEREAD));
    assert!(desc.iter().any(|d| d.name == NAME_MP_ADMIN));
}

#[test]
fn selector_returns_structured_reason_when_unavailable() {
    let mut acc = account(NAME_FAKE);
    acc.active = false;
    let result = ConnectorSelector::select(&SelectRequest {
        required_capability: ConnectorCapability::DiscoverRecent,
        preferred_connector: None,
        candidates: vec![acc],
    });
    match result {
        SelectionResult::Unavailable(f) => {
            assert_eq!(f.reason_code, codes::INACTIVE);
            assert!(!f.rejected.is_empty());
        }
        SelectionResult::Selected(_) => panic!("应不可用"),
    }
}

#[test]
fn selector_needs_relogin_reason() {
    let mut acc = account(NAME_FAKE);
    acc.needs_relogin = true;
    let result = ConnectorSelector::select(&SelectRequest {
        required_capability: ConnectorCapability::DiscoverRecent,
        preferred_connector: None,
        candidates: vec![acc],
    });
    match result {
        SelectionResult::Unavailable(f) => {
            assert_eq!(f.reason_code, codes::NEEDS_RELOGIN);
        }
        SelectionResult::Selected(_) => panic!("应不可用"),
    }
}

#[test]
fn selector_picks_healthy_account() {
    let bad = {
        let mut a = account(NAME_FAKE);
        a.budget_exhausted = true;
        a
    };
    let good = account(NAME_FAKE);
    let good_id = good.id;
    let result = ConnectorSelector::select(&SelectRequest {
        required_capability: ConnectorCapability::DiscoverRecent,
        preferred_connector: None,
        candidates: vec![bad, good],
    });
    match result {
        SelectionResult::Selected(s) => assert_eq!(s.account.id, good_id),
        SelectionResult::Unavailable(f) => panic!("应选中账号: {}", f.reason_code),
    }
}

#[test]
fn selector_unsupported_when_capability_missing() {
    let mut acc = account(NAME_WEREAD);
    acc.enabled_capabilities = CapabilitySet::default();
    let result = ConnectorSelector::select(&SelectRequest {
        required_capability: ConnectorCapability::DiscoverRecent,
        preferred_connector: Some(NAME_WEREAD.into()),
        candidates: vec![acc],
    });
    match result {
        SelectionResult::Unavailable(f) => {
            assert_eq!(f.reason_code, codes::UNSUPPORTED);
        }
        SelectionResult::Selected(_) => panic!("weread 无能力时应 unsupported"),
    }
}

#[tokio::test]
async fn weread_default_off_returns_unsupported() {
    let weread = WereadConnector::new();
    assert!(!weread.is_enabled());
    // 骨架阶段仅开放扫码展示；发现能力仍关闭。
    assert!(weread.capabilities().contains(ConnectorCapability::QrLogin));
    assert!(
        !weread
            .capabilities()
            .contains(ConnectorCapability::DiscoverRecent)
    );

    // LO0 等级提示不得改变结论
    let mut acc = account(NAME_WEREAD);
    acc.enabled_capabilities = CapabilitySet::default();

    let probe = weread.probe(&acc, None, Utc::now()).await.unwrap();
    assert_eq!(probe.status, ProbeStatus::Unsupported);
    assert_eq!(probe.reason_code.as_deref(), Some(codes::UNSUPPORTED));

    let err = weread
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "weread_mp_1".into(),
                level_hint: Some("LO0".into()),
            },
            account: acc,
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.reason_code, codes::UNSUPPORTED);
}

#[tokio::test]
async fn disabled_real_qr_connectors_do_not_return_placeholders() {
    let ctx = LoginContext {
        ticket: "display-check".into(),
        fixed_egress_id: None,
        administrator_session_hash: "hash".into(),
        proxy_url: None,
    };

    for backend in [
        Box::new(MpAdminConnector::new()) as Box<dyn ConnectorBackend>,
        Box::new(WereadConnector::new()) as Box<dyn ConnectorBackend>,
    ] {
        let error = backend.create_qr_session(ctx.clone()).await.unwrap_err();
        assert_eq!(error.reason_code, codes::UNSUPPORTED);
    }
}

#[test]
fn qr_provider_state_and_proxy_are_redacted_in_debug() {
    let context = LoginContext {
        ticket: "ticket".into(),
        fixed_egress_id: None,
        administrator_session_hash: "admin-secret".into(),
        proxy_url: Some("http://user:password@proxy.local".into()),
    };
    let session = crate::types::QrSession {
        ticket: "ticket".into(),
        qr_data_url: "data:image/png;base64,x".into(),
        expires_at: Utc::now(),
        poll_after_ms: 1000,
        provider_state_json: Some("cookie=provider-secret".into()),
    };
    let poll = crate::types::QrPollResult {
        status: QrPollStatus::Waiting,
        message: None,
        provider_state_json: Some("cookie=updated-secret".into()),
    };
    let debug = format!("{context:?} {session:?} {poll:?}");
    assert!(!debug.contains("admin-secret"));
    assert!(!debug.contains("password"));
    assert!(!debug.contains("provider-secret"));
    assert!(!debug.contains("updated-secret"));
}

#[tokio::test]
async fn mp_admin_rejects_unauthorized_enumeration() {
    let mp = MpAdminConnector::new();
    let mut acc = account(NAME_MP_ADMIN);
    acc.authorized_mp_ids = vec!["biz_authorized".into()];

    let err = mp
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "biz_third_party".into(),
                level_hint: None,
            },
            account: acc.clone(),
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.reason_code, codes::UNAUTHORIZED_ENUMERATION);

    // 无授权范围
    acc.authorized_mp_ids.clear();
    let err2 = mp
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "biz_authorized".into(),
                level_hint: None,
            },
            account: acc,
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err2.reason_code, codes::UNAUTHORIZED_ENUMERATION);
}

#[tokio::test]
async fn mp_admin_protocol_not_ready_is_explicitly_unsupported() {
    let mp = MpAdminConnector::new();
    let acc = account(NAME_MP_ADMIN);
    let error = mp
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "biz_authorized".into(),
                level_hint: None,
            },
            account: acc.clone(),
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap_err();
    assert_eq!(error.reason_code, codes::UNSUPPORTED);

    let probe = mp.probe(&acc, None, Utc::now()).await.unwrap();
    assert_eq!(probe.status, ProbeStatus::Unsupported);
    assert!(assert_probe_sanitized(&probe).is_ok());
}

#[tokio::test]
async fn mp_admin_protocol_ready_requires_credentials_json() {
    let mp = MpAdminConnector::new().with_protocol_ready(true);
    let mut acc = account(NAME_MP_ADMIN);
    acc.authorized_mp_ids = vec!["biz_authorized".into()];
    let err = mp
        .discover_recent(DiscoverRequest {
            source: SourceRef {
                id: None,
                identity: "biz_authorized".into(),
                level_hint: None,
            },
            account: acc,
            limit: 3,
            cursor: 0,
            page_budget: 3,
            credentials_json: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.reason_code, codes::NEEDS_RELOGIN);
}

#[tokio::test]
async fn probe_recorder_stores_sanitized_result() {
    let recorder = MemoryProbeRecorder::new();
    let fake = FakeConnector::new();
    let acc = account(NAME_FAKE);
    let now = Utc::now();
    let result = fake.probe(&acc, None, now).await.unwrap();
    recorder.record(acc.id, NAME_FAKE, result.clone(), now);

    let latest = recorder.latest(acc.id).unwrap();
    assert_eq!(latest.connector, NAME_FAKE);
    assert_eq!(latest.result.status, ProbeStatus::Healthy);
    assert!(assert_probe_sanitized(&latest.result).is_ok());
}

#[tokio::test]
async fn registry_get_and_probe_via_trait_object() {
    let mut registry = ConnectorRegistry::new();
    registry.register(Arc::new(FakeConnector::new()));
    let backend = registry.require(NAME_FAKE).unwrap();
    let probe = backend
        .probe(&account(NAME_FAKE), None, Utc::now())
        .await
        .unwrap();
    assert_eq!(probe.status, ProbeStatus::Healthy);
}
