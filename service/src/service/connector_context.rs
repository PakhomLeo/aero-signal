//! 真实 connector 调用上下文：账号选择、会话引用、授权范围与固定出口状态。

use std::sync::Arc;

use platform_connectors::{
    CapabilitySet, CollectorAccountView, ConnectorCapability, ConnectorRegistry, ConnectorSelector,
    SelectRequest, SelectionResult,
};
use server_core::{AppError, Clock};
use uuid::Uuid;

use crate::adapters::secret_store::SecretStore;
use crate::domain::collector::CollectorAccountStatus;
use crate::repository::collector::CollectorRepository;
use crate::repository::egress::EgressRepository;

#[derive(Clone)]
pub struct SelectedConnectorContext {
    pub account: CollectorAccountView,
    pub credentials_json: Option<String>,
}

impl std::fmt::Debug for SelectedConnectorContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SelectedConnectorContext")
            .field("account", &self.account)
            .field(
                "credentials_json",
                &self.credentials_json.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

pub struct CollectorConnectorResolver {
    collectors: Arc<dyn CollectorRepository>,
    secrets: Arc<dyn SecretStore>,
    connectors: Arc<ConnectorRegistry>,
    clock: Arc<dyn Clock>,
    egress: Option<Arc<dyn EgressRepository>>,
}

impl CollectorConnectorResolver {
    pub fn new(
        collectors: Arc<dyn CollectorRepository>,
        secrets: Arc<dyn SecretStore>,
        connectors: Arc<ConnectorRegistry>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            collectors,
            secrets,
            connectors,
            clock,
            egress: None,
        }
    }

    pub fn with_egress_repository(mut self, egress: Arc<dyn EgressRepository>) -> Self {
        self.egress = Some(egress);
        self
    }

    pub async fn resolve(
        &self,
        required_capability: ConnectorCapability,
        requested_account_id: Option<Uuid>,
        preferred_connector: Option<String>,
    ) -> Result<SelectedConnectorContext, AppError> {
        let mut accounts = self.collectors.list_accounts().await.map_err(|e| {
            AppError::new("collector.repository_failed", sanitize_repository_error(&e))
        })?;
        if let Some(requested) = requested_account_id {
            accounts.retain(|account| account.id == requested);
            if accounts.is_empty() {
                return Err(AppError::not_found(
                    "collector.not_found",
                    "指定采集登录账号不存在",
                ));
            }
        }

        let mut candidates = Vec::with_capacity(accounts.len());
        let now = chrono::DateTime::<chrono::Utc>::from(self.clock.now());
        let today = now.date_naive();
        for account in accounts {
            let _backend = self.connectors.get(&account.connector).ok_or_else(|| {
                AppError::new(
                    "connector.unsupported",
                    format!("采集账号引用了未注册 connector: {}", account.connector),
                )
            })?;
            let binding = self
                .collectors
                .get_egress_binding(account.id)
                .await
                .map_err(|e| {
                    AppError::new(
                        "collector.egress_read_failed",
                        sanitize_repository_error(&e),
                    )
                })?;
            let enabled_capabilities =
                serde_json::from_value::<CapabilitySet>(account.capability_json.clone())
                    .unwrap_or_default();
            let cooling = account.cooldown_until.is_some_and(|until| until > now);
            let budget_exhausted = account.daily_budget_remaining == Some(0);
            let fixed_egress_healthy =
                match binding.as_ref().and_then(|binding| binding.fixed_egress_id) {
                    Some(egress_id) => match &self.egress {
                        Some(repository) => repository
                            .get_egress_profile(egress_id)
                            .await
                            .map_err(|e| {
                                AppError::new(
                                    "collector.egress_read_failed",
                                    sanitize_repository_error(&e),
                                )
                            })?
                            .is_some_and(|profile| {
                                profile.status == crate::domain::egress::EgressStatus::Active
                                    && !matches!(
                                        profile.last_preflight_status.as_deref(),
                                        Some("unhealthy" | "failed")
                                    )
                            }),
                        None => false,
                    },
                    None => true,
                };
            candidates.push(CollectorAccountView {
                id: account.id,
                connector: account.connector,
                display_name: Some(account.display_name),
                active: account.status == CollectorAccountStatus::Active
                    && account.last_error_code.is_none()
                    && account
                        .last_probe_at
                        .is_some_and(|probed_at| probed_at.date_naive() == today),
                needs_relogin: account.status == CollectorAccountStatus::NeedsRelogin
                    || binding
                        .as_ref()
                        .map(|b| b.requires_relogin)
                        .unwrap_or(false),
                fixed_egress_healthy: fixed_egress_healthy
                    && !binding.as_ref().is_some_and(|b| b.requires_relogin),
                fixed_egress_id: binding.as_ref().and_then(|b| b.fixed_egress_id),
                cooling,
                budget_exhausted,
                enabled_capabilities,
                authorized_mp_ids: Vec::new(),
            });
        }

        let selected = match ConnectorSelector::select(&SelectRequest {
            required_capability,
            preferred_connector,
            candidates,
        }) {
            SelectionResult::Selected(selected) => selected.account,
            SelectionResult::Unavailable(failure) => {
                return Err(AppError::new(failure.reason_code, failure.message));
            }
        };

        let session = self
            .collectors
            .get_active_login_session(selected.id)
            .await
            .map_err(|e| {
                AppError::new(
                    "collector.session_read_failed",
                    sanitize_repository_error(&e),
                )
            })?
            .ok_or_else(|| AppError::new("collector.needs_relogin", "采集账号没有有效登录会话"))?;
        if session.expires_at.is_some_and(|expires_at| {
            expires_at <= chrono::DateTime::<chrono::Utc>::from(self.clock.now())
        }) {
            return Err(AppError::new(
                "collector.session_expired",
                "采集账号登录会话已过期，请重新扫码",
            ));
        }
        let credentials_json = self
            .secrets
            .get(&session.credential_secret_ref)
            .await
            .map_err(|_| AppError::new("secret_store.read_failed", "读取采集账号凭据失败"))?
            .ok_or_else(|| {
                AppError::new("collector.credential_missing", "采集账号凭据引用不存在")
            })?;
        let mut account = selected;
        account.authorized_mp_ids = session.authorized_mp_ids;
        Ok(SelectedConnectorContext {
            account,
            credentials_json: Some(credentials_json),
        })
    }
}

fn sanitize_repository_error(error: &str) -> String {
    let code = error.split(':').next().unwrap_or("repository.failed");
    format!("仓储操作失败（{code}）")
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::{Duration, Utc};
    use platform_connectors::{ConnectorBackend, FakeConnector, NAME_FAKE};
    use server_core::SystemClock;

    use crate::adapters::secret_store::MemorySecretStore;
    use crate::domain::collector::{
        CollectorAccount, CollectorEgressBinding, CollectorLoginSession,
    };
    use crate::repository::collector_memory::MemoryCollectorStore;

    struct FailingSecretStore;

    #[async_trait]
    impl SecretStore for FailingSecretStore {
        async fn put(&self, _value: &str) -> Result<String, String> {
            Err("sensitive backend detail".into())
        }

        async fn get(&self, _secret_ref: &str) -> Result<Option<String>, String> {
            Err("sensitive backend detail".into())
        }

        async fn delete(&self, _secret_ref: &str) -> Result<(), String> {
            Ok(())
        }
    }

    struct Setup<'a> {
        status: CollectorAccountStatus,
        credential: Option<&'a str>,
        requires_relogin: bool,
        fixed_egress_id: Option<Uuid>,
        cooldown_until: Option<chrono::DateTime<Utc>>,
        daily_budget_remaining: Option<i64>,
        missing_secret: bool,
        failing_secret_store: bool,
    }

    impl<'a> Setup<'a> {
        fn active(credential: Option<&'a str>) -> Self {
            Self {
                status: CollectorAccountStatus::Active,
                credential,
                requires_relogin: false,
                fixed_egress_id: None,
                cooldown_until: None,
                daily_budget_remaining: None,
                missing_secret: false,
                failing_secret_store: false,
            }
        }
    }

    async fn setup(options: Setup<'_>) -> (CollectorConnectorResolver, Uuid) {
        let collectors = Arc::new(MemoryCollectorStore::new());
        let memory_secrets = Arc::new(MemorySecretStore::new());
        let mut registry = ConnectorRegistry::new();
        registry.register(Arc::new(FakeConnector::new()) as Arc<dyn ConnectorBackend>);
        let id = Uuid::new_v4();
        let now = Utc::now();
        collectors
            .insert_account(&CollectorAccount {
                id,
                connector: NAME_FAKE.into(),
                display_name: "collector".into(),
                external_account_ref: None,
                status: options.status,
                capability_json: serde_json::to_value(CapabilitySet::new([
                    ConnectorCapability::DiscoverRecent,
                ]))
                .unwrap(),
                last_probe_at: Some(now),
                last_success_at: None,
                last_failure_at: None,
                last_error_code: None,
                cooldown_until: options.cooldown_until,
                daily_budget_remaining: options.daily_budget_remaining,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        collectors
            .upsert_egress_binding(&CollectorEgressBinding {
                collector_account_id: id,
                fixed_egress_id: options.fixed_egress_id,
                bound_at: now,
                changed_at: None,
                requires_relogin: options.requires_relogin,
            })
            .await
            .unwrap();
        if let Some(value) = options.credential {
            let secret_ref = if options.missing_secret || options.failing_secret_store {
                "sec_missing".to_string()
            } else {
                memory_secrets.put(value).await.unwrap()
            };
            collectors
                .insert_login_session(&CollectorLoginSession {
                    id: Uuid::new_v4(),
                    collector_account_id: id,
                    credential_secret_ref: secret_ref,
                    browser_profile_ref: None,
                    authorized_mp_ids: serde_json::from_str::<serde_json::Value>(value)
                        .ok()
                        .and_then(|json| json.get("authorized_mp_ids").cloned())
                        .and_then(|ids| serde_json::from_value(ids).ok())
                        .unwrap_or_default(),
                    expires_at: None,
                    confirmed_at: now,
                    revoked_at: None,
                })
                .await
                .unwrap();
        }
        let secrets: Arc<dyn SecretStore> = if options.failing_secret_store {
            Arc::new(FailingSecretStore)
        } else {
            memory_secrets
        };
        (
            CollectorConnectorResolver::new(
                collectors,
                secrets,
                Arc::new(registry),
                Arc::new(SystemClock),
            ),
            id,
        )
    }

    #[tokio::test]
    async fn resolves_session_credentials_and_authorized_ids() {
        let (resolver, id) = setup(Setup::active(Some(
            r#"{"authorized_mp_ids":["biz-a","biz-b"],"token":"hidden"}"#,
        )))
        .await;
        let selected = resolver
            .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
            .await
            .unwrap();
        assert_eq!(selected.account.authorized_mp_ids, vec!["biz-a", "biz-b"]);
        assert!(selected.credentials_json.is_some());
        assert!(!format!("{selected:?}").contains("hidden"));
    }

    #[tokio::test]
    async fn reports_missing_session_and_unhealthy_binding_separately() {
        let (missing, id) = setup(Setup::active(None)).await;
        assert_eq!(
            missing
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "collector.needs_relogin"
        );

        let mut options = Setup::active(Some(r#"{"authorized_mp_ids":[]}"#));
        options.requires_relogin = true;
        let (unhealthy, id) = setup(options).await;
        assert_eq!(
            unhealthy
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "collector.needs_relogin"
        );
    }

    #[tokio::test]
    async fn reports_missing_requested_account() {
        let (resolver, _) = setup(Setup::active(Some(r#"{"authorized_mp_ids":[]}"#))).await;
        assert_eq!(
            resolver
                .resolve(
                    ConnectorCapability::DiscoverRecent,
                    Some(Uuid::new_v4()),
                    None
                )
                .await
                .unwrap_err()
                .code,
            "collector.not_found"
        );
    }

    #[tokio::test]
    async fn reports_credential_and_secret_store_failures_without_leaking_details() {
        let mut missing = Setup::active(Some(r#"{"token":"hidden"}"#));
        missing.missing_secret = true;
        let (resolver, id) = setup(missing).await;
        assert_eq!(
            resolver
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "collector.credential_missing"
        );

        let mut failing = Setup::active(Some(r#"{"token":"hidden"}"#));
        failing.failing_secret_store = true;
        let (resolver, id) = setup(failing).await;
        let error = resolver
            .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, "secret_store.read_failed");
        assert!(!error.message.contains("sensitive backend detail"));
    }

    #[tokio::test]
    async fn rejects_cooling_exhausted_and_unhealthy_fixed_egress_accounts() {
        let mut cooling = Setup::active(Some(r#"{"authorized_mp_ids":[]}"#));
        cooling.cooldown_until = Some(Utc::now() + Duration::minutes(5));
        let (resolver, id) = setup(cooling).await;
        assert_eq!(
            resolver
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "collector.cooling"
        );

        let mut exhausted = Setup::active(Some(r#"{"authorized_mp_ids":[]}"#));
        exhausted.daily_budget_remaining = Some(0);
        let (resolver, id) = setup(exhausted).await;
        assert_eq!(
            resolver
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "collector.budget_exhausted"
        );

        let mut unhealthy = Setup::active(Some(r#"{"authorized_mp_ids":[]}"#));
        unhealthy.fixed_egress_id = Some(Uuid::new_v4());
        let (resolver, id) = setup(unhealthy).await;
        assert_eq!(
            resolver
                .resolve(ConnectorCapability::DiscoverRecent, Some(id), None)
                .await
                .unwrap_err()
                .code,
            "egress.unhealthy"
        );
    }
}
