//! EgressResolver：按用途解析 LocalDirect / FixedBinding / DynamicLease。

use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::adapters::proxy_provider::{ProxyProviderAdapter, ProxyProviderError};
use crate::domain::egress::{
    EgressKind, EgressProfile, EgressPurpose, EgressStatus, LOCAL_DIRECT_PROFILE_ID, ProxyProvider,
};
use crate::repository::EgressRepository;
use crate::service::dynamic_lease::{DynamicLease, DynamicLeaseManager};
use server_core::AppError;

/// 解析结果。
#[derive(Debug, Clone)]
pub enum ResolvedEgress {
    LocalDirect {
        profile_id: Uuid,
    },
    FixedBinding {
        profile_id: Uuid,
        endpoint_secret_ref: Option<String>,
        provider_id: Option<Uuid>,
    },
    DynamicLease {
        lease: DynamicLease,
    },
}

impl ResolvedEgress {
    pub fn profile_id(&self) -> Uuid {
        match self {
            Self::LocalDirect { profile_id } => *profile_id,
            Self::FixedBinding { profile_id, .. } => *profile_id,
            Self::DynamicLease { lease } => lease.profile_id,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::LocalDirect { .. } => "local_direct",
            Self::FixedBinding { .. } => "fixed",
            Self::DynamicLease { .. } => "dynamic",
        }
    }
}

/// 解析请求。
#[derive(Debug, Clone)]
pub struct ResolveRequest {
    pub purpose: EgressPurpose,
    /// 可选：指定出口 id（须通过用途校验）。
    pub preferred_profile_id: Option<Uuid>,
    /// 固定绑定场景：已绑定的固定出口。
    pub bound_fixed_egress_id: Option<Uuid>,
}

/// 出口解析器。
pub struct EgressResolver {
    repo: Arc<dyn EgressRepository>,
    leases: Arc<DynamicLeaseManager>,
    providers: Arc<dyn ProxyProviderAdapter>,
}

impl EgressResolver {
    pub fn new(
        repo: Arc<dyn EgressRepository>,
        leases: Arc<DynamicLeaseManager>,
        providers: Arc<dyn ProxyProviderAdapter>,
    ) -> Self {
        Self {
            repo,
            leases,
            providers,
        }
    }

    pub fn leases(&self) -> &Arc<DynamicLeaseManager> {
        &self.leases
    }

    /// 按用途解析出口；混用拒绝。
    pub async fn resolve(
        &self,
        req: ResolveRequest,
        now: DateTime<Utc>,
    ) -> Result<ResolvedEgress, AppError> {
        if let Some(id) = req.preferred_profile_id {
            let profile = self.require_profile(id).await?;
            self.ensure_purpose_allowed(&profile, req.purpose)?;
            return self.resolve_profile(&profile, req.purpose, now).await;
        }

        if req.purpose.is_fixed_family() {
            if let Some(bound_id) = req.bound_fixed_egress_id {
                let profile = self.require_profile(bound_id).await?;
                self.ensure_purpose_allowed(&profile, req.purpose)?;
                if profile.kind != EgressKind::Fixed && profile.kind != EgressKind::LocalDirect {
                    return Err(AppError::new(
                        "egress.purpose_mismatch",
                        "绑定出口必须是固定或本机出口",
                    ));
                }
                return self.resolve_profile(&profile, req.purpose, now).await;
            }
            // 按优先级选固定，否则回退本机
            if let Some(p) = self.pick_active(EgressKind::Fixed, req.purpose).await? {
                return self.resolve_profile(&p, req.purpose, now).await;
            }
            return self.resolve_local(now).await;
        }

        // 动态族：按优先级选动态，否则本机
        if let Some(p) = self.pick_active(EgressKind::Dynamic, req.purpose).await? {
            return self.resolve_profile(&p, req.purpose, now).await;
        }
        self.resolve_local(now).await
    }

    /// 在动态出口网络失败时，尝试下一健康出口。
    ///
    /// 调用方必须先经 [`crate::service::EgressPolicy`] 判定：仅
    /// `network_unreachable` 允许轮换；`captcha/waf/platform_denied/authentication_expired`
    /// 不得调用本方法规避平台限制。
    pub async fn try_next_dynamic(
        &self,
        purpose: EgressPurpose,
        exclude_profile_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Option<ResolvedEgress>, AppError> {
        if !purpose.is_dynamic_family() {
            return Err(AppError::new(
                "egress.purpose_mismatch",
                "仅详情/资产下载可轮换动态出口",
            ));
        }
        let candidates = self
            .repo
            .list_egress_profiles()
            .await
            .map_err(AppError::internal)?;
        let mut ranked: Vec<_> = candidates
            .into_iter()
            .filter(|p| {
                p.kind == EgressKind::Dynamic
                    && p.status == EgressStatus::Active
                    && p.allowed_purposes.contains(&purpose)
                    && !exclude_profile_ids.contains(&p.id)
            })
            .collect();
        ranked.sort_by_key(|p| (p.priority, p.created_at));
        if let Some(p) = ranked.into_iter().next() {
            return Ok(Some(self.resolve_profile(&p, purpose, now).await?));
        }
        Ok(None)
    }

    async fn pick_active(
        &self,
        kind: EgressKind,
        purpose: EgressPurpose,
    ) -> Result<Option<EgressProfile>, AppError> {
        let mut list = self
            .repo
            .list_egress_profiles()
            .await
            .map_err(AppError::internal)?;
        list.retain(|p| {
            p.kind == kind
                && p.status == EgressStatus::Active
                && p.allowed_purposes.contains(&purpose)
        });
        list.sort_by_key(|p| (p.priority, p.created_at));
        Ok(list.into_iter().next())
    }

    async fn require_profile(&self, id: Uuid) -> Result<EgressProfile, AppError> {
        self.repo
            .get_egress_profile(id)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("egress.not_found", format!("出口不存在: {id}")))
    }

    fn ensure_purpose_allowed(
        &self,
        profile: &EgressProfile,
        purpose: EgressPurpose,
    ) -> Result<(), AppError> {
        if !profile.kind.allows_purpose(purpose) {
            return Err(AppError::new(
                "egress.purpose_mismatch",
                format!(
                    "出口种类 {} 不允许用途 {}（固定/动态用途不得混用）",
                    profile.kind, purpose
                ),
            )
            .with_detail("kind", profile.kind.as_str())
            .with_detail("purpose", purpose.as_str()));
        }
        if !profile.allowed_purposes.contains(&purpose) {
            return Err(AppError::new(
                "egress.purpose_not_allowed",
                format!("出口 {} 的 allowed_purposes 不包含 {purpose}", profile.name),
            )
            .with_detail("purpose", purpose.as_str()));
        }
        Ok(())
    }

    async fn resolve_local(&self, _now: DateTime<Utc>) -> Result<ResolvedEgress, AppError> {
        // 优先用种子/库中的 local_direct；没有则返回稳定本地 id
        if let Ok(Some(p)) = self.repo.get_egress_profile(LOCAL_DIRECT_PROFILE_ID).await {
            return Ok(ResolvedEgress::LocalDirect { profile_id: p.id });
        }
        let list = self
            .repo
            .list_egress_profiles()
            .await
            .map_err(AppError::internal)?;
        if let Some(p) = list.into_iter().find(|p| p.kind == EgressKind::LocalDirect) {
            return Ok(ResolvedEgress::LocalDirect { profile_id: p.id });
        }
        Ok(ResolvedEgress::LocalDirect {
            profile_id: LOCAL_DIRECT_PROFILE_ID,
        })
    }

    async fn resolve_profile(
        &self,
        profile: &EgressProfile,
        purpose: EgressPurpose,
        now: DateTime<Utc>,
    ) -> Result<ResolvedEgress, AppError> {
        match profile.kind {
            EgressKind::LocalDirect => Ok(ResolvedEgress::LocalDirect {
                profile_id: profile.id,
            }),
            EgressKind::Fixed => {
                if purpose.is_dynamic_family() {
                    return Err(AppError::new(
                        "egress.purpose_mismatch",
                        "固定出口不可用于详情/资产下载",
                    ));
                }
                Ok(ResolvedEgress::FixedBinding {
                    profile_id: profile.id,
                    endpoint_secret_ref: profile.endpoint_secret_ref.clone(),
                    provider_id: profile.provider_id,
                })
            }
            EgressKind::Dynamic => {
                if purpose.is_fixed_family() {
                    return Err(AppError::new(
                        "egress.purpose_mismatch",
                        "动态出口不可用于登录/发现",
                    ));
                }
                let provider = self.require_provider_for(profile).await?;
                let endpoint = self
                    .providers
                    .allocate(
                        &provider,
                        profile.endpoint_secret_ref.as_deref(),
                        purpose,
                        now,
                    )
                    .await
                    .map_err(map_provider_err)?;
                let lease = self.leases.acquire(profile.id, endpoint, now).await;
                Ok(ResolvedEgress::DynamicLease { lease })
            }
        }
    }

    async fn require_provider_for(
        &self,
        profile: &EgressProfile,
    ) -> Result<ProxyProvider, AppError> {
        let pid = profile.provider_id.ok_or_else(|| {
            AppError::new(
                "egress.provider_required",
                "动态/固定出口需要关联 proxy_provider",
            )
        })?;
        self.repo
            .get_proxy_provider(pid)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::not_found("egress.provider_not_found", format!("供应商不存在: {pid}"))
            })
    }
}

fn map_provider_err(e: ProxyProviderError) -> AppError {
    match e {
        ProxyProviderError::Disabled => AppError::new("egress.provider_disabled", "供应商未启用"),
        ProxyProviderError::MissingEndpointSecret => {
            AppError::new("egress.missing_secret", "缺少端点密钥引用")
        }
        ProxyProviderError::SecretNotFound(r) => {
            AppError::new("egress.secret_not_found", "密钥引用不存在").with_detail("ref", r)
        }
        ProxyProviderError::VendorFailed(_) => {
            AppError::new("egress.provider_failed", "出口供应商调用失败")
        }
        ProxyProviderError::UnsupportedMode => {
            AppError::new("egress.unsupported_mode", "不支持的供应商模式")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::proxy_provider::DefaultProxyProviderAdapter;
    use crate::adapters::secret_store::{MemorySecretStore, SecretStore};
    use crate::domain::egress::{
        EgressKind, EgressPurpose, EgressStatus, ProxyProviderMode, validate_purposes_for_kind,
    };
    use crate::repository::MemoryEgressStore;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    async fn setup() -> (
        EgressResolver,
        Arc<MemoryEgressStore>,
        Arc<MemorySecretStore>,
    ) {
        let store = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let adapter = Arc::new(DefaultProxyProviderAdapter::new(
            secrets.clone() as Arc<dyn crate::adapters::secret_store::SecretStore>
        ));
        let leases = Arc::new(DynamicLeaseManager::new());
        let resolver =
            EgressResolver::new(store.clone() as Arc<dyn EgressRepository>, leases, adapter);
        (resolver, store, secrets)
    }

    async fn insert_fixed(
        store: &MemoryEgressStore,
        secrets: &MemorySecretStore,
        name: &str,
    ) -> Uuid {
        let now = now();
        let secret_ref = secrets.put("http://fixed-proxy").await.unwrap();
        let provider = ProxyProvider {
            id: Uuid::new_v4(),
            name: format!("{name}-prov"),
            mode: ProxyProviderMode::StaticEndpoint,
            api_base_url: None,
            auth_secret_ref: None,
            request_template_json: json!({}),
            response_mapping_json: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store.insert_proxy_provider(&provider).await.unwrap();
        let profile = EgressProfile {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: EgressKind::Fixed,
            provider_id: Some(provider.id),
            endpoint_secret_ref: Some(secret_ref),
            priority: 10,
            allowed_purposes: vec![EgressPurpose::Login, EgressPurpose::Discover],
            status: EgressStatus::Active,
            last_preflight_status: None,
            last_preflight_at: None,
            created_at: now,
            updated_at: now,
        };
        validate_purposes_for_kind(profile.kind, &profile.allowed_purposes).unwrap();
        store.insert_egress_profile(&profile).await.unwrap();
        profile.id
    }

    async fn insert_dynamic(
        store: &MemoryEgressStore,
        secrets: &MemorySecretStore,
        name: &str,
    ) -> Uuid {
        let now = now();
        let secret_ref = secrets.put("http://dyn-proxy").await.unwrap();
        let provider = ProxyProvider {
            id: Uuid::new_v4(),
            name: format!("{name}-prov"),
            mode: ProxyProviderMode::StaticEndpoint,
            api_base_url: None,
            auth_secret_ref: None,
            request_template_json: json!({}),
            response_mapping_json: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store.insert_proxy_provider(&provider).await.unwrap();
        let profile = EgressProfile {
            id: Uuid::new_v4(),
            name: name.into(),
            kind: EgressKind::Dynamic,
            provider_id: Some(provider.id),
            endpoint_secret_ref: Some(secret_ref),
            priority: 10,
            allowed_purposes: vec![EgressPurpose::ArticleDetail, EgressPurpose::AssetDownload],
            status: EgressStatus::Active,
            last_preflight_status: None,
            last_preflight_at: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_egress_profile(&profile).await.unwrap();
        profile.id
    }

    #[tokio::test]
    async fn login_rejects_dynamic_profile() {
        let (resolver, store, secrets) = setup().await;
        let dyn_id = insert_dynamic(&store, &secrets, "dyn1").await;
        let err = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::Login,
                    preferred_profile_id: Some(dyn_id),
                    bound_fixed_egress_id: None,
                },
                now(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "egress.purpose_mismatch");
    }

    #[tokio::test]
    async fn detail_rejects_fixed_profile() {
        let (resolver, store, secrets) = setup().await;
        let fixed_id = insert_fixed(&store, &secrets, "fix1").await;
        let err = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::ArticleDetail,
                    preferred_profile_id: Some(fixed_id),
                    bound_fixed_egress_id: None,
                },
                now(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "egress.purpose_mismatch");
    }

    #[tokio::test]
    async fn login_uses_fixed_or_local() {
        let (resolver, store, secrets) = setup().await;
        let fixed_id = insert_fixed(&store, &secrets, "fix2").await;
        let r = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::Login,
                    preferred_profile_id: None,
                    bound_fixed_egress_id: None,
                },
                now(),
            )
            .await
            .unwrap();
        assert!(
            matches!(r, ResolvedEgress::FixedBinding { profile_id, .. } if profile_id == fixed_id)
        );
    }

    #[tokio::test]
    async fn detail_acquires_dynamic_lease() {
        let (resolver, store, secrets) = setup().await;
        let _ = insert_dynamic(&store, &secrets, "dyn2").await;
        let r = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::ArticleDetail,
                    preferred_profile_id: None,
                    bound_fixed_egress_id: None,
                },
                now(),
            )
            .await
            .unwrap();
        assert!(matches!(r, ResolvedEgress::DynamicLease { .. }));
    }

    #[tokio::test]
    async fn discover_falls_back_to_local() {
        let (resolver, _, _) = setup().await;
        let r = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::Discover,
                    preferred_profile_id: None,
                    bound_fixed_egress_id: None,
                },
                now(),
            )
            .await
            .unwrap();
        assert!(matches!(r, ResolvedEgress::LocalDirect { .. }));
    }

    #[tokio::test]
    async fn try_next_dynamic_skips_excluded_and_picks_next() {
        use crate::service::egress_policy::{EgressErrorClass, EgressPolicy};

        let (resolver, store, secrets) = setup().await;
        let first = insert_dynamic(&store, &secrets, "dyn-a").await;
        // 提高第二出口优先级数值（更低优先），但仍应在排除 first 后被选中
        let second = {
            let now = now();
            let secret_ref = secrets.put("http://dyn-b").await.unwrap();
            let provider = ProxyProvider {
                id: Uuid::new_v4(),
                name: "dyn-b-prov".into(),
                mode: ProxyProviderMode::StaticEndpoint,
                api_base_url: None,
                auth_secret_ref: None,
                request_template_json: json!({}),
                response_mapping_json: json!({}),
                enabled: true,
                created_at: now,
                updated_at: now,
            };
            store.insert_proxy_provider(&provider).await.unwrap();
            let profile = EgressProfile {
                id: Uuid::new_v4(),
                name: "dyn-b".into(),
                kind: EgressKind::Dynamic,
                provider_id: Some(provider.id),
                endpoint_secret_ref: Some(secret_ref),
                priority: 20,
                allowed_purposes: vec![EgressPurpose::ArticleDetail, EgressPurpose::AssetDownload],
                status: EgressStatus::Active,
                last_preflight_status: None,
                last_preflight_at: None,
                created_at: now,
                updated_at: now,
            };
            store.insert_egress_profile(&profile).await.unwrap();
            profile.id
        };

        // 平台限制：策略禁止轮换，调用方不得调用 try_next_dynamic
        assert!(!EgressPolicy::allows_rotation(
            EgressErrorClass::PlatformDenied
        ));
        assert!(!EgressPolicy::allows_rotation(EgressErrorClass::Captcha));
        assert!(!EgressPolicy::allows_rotation(EgressErrorClass::Waf));
        assert!(!EgressPolicy::allows_rotation(
            EgressErrorClass::AuthenticationExpired
        ));

        // 仅 network_unreachable 允许轮换
        assert!(EgressPolicy::allows_rotation(
            EgressErrorClass::NetworkUnreachable
        ));
        let next = resolver
            .try_next_dynamic(EgressPurpose::ArticleDetail, &[first], now())
            .await
            .unwrap()
            .expect("应选中下一动态出口");
        assert_eq!(next.profile_id(), second);
    }

    #[tokio::test]
    async fn try_next_dynamic_rejects_login_purpose() {
        let (resolver, store, secrets) = setup().await;
        let _ = insert_dynamic(&store, &secrets, "dyn-login-guard").await;
        let err = resolver
            .try_next_dynamic(EgressPurpose::Login, &[], now())
            .await
            .unwrap_err();
        assert_eq!(err.code, "egress.purpose_mismatch");
    }

    #[tokio::test]
    async fn dynamic_provider_failure_is_structured_and_sanitized() {
        let store = Arc::new(MemoryEgressStore::with_local_direct());
        let secrets = Arc::new(MemorySecretStore::new());
        let now = now();
        let provider = ProxyProvider {
            id: Uuid::new_v4(),
            name: "failing-vendor".into(),
            mode: ProxyProviderMode::VendorApi,
            api_base_url: Some("http://secret-vendor.internal".into()),
            auth_secret_ref: None,
            request_template_json: json!({}),
            response_mapping_json: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        };
        store.insert_proxy_provider(&provider).await.unwrap();
        store
            .insert_egress_profile(&EgressProfile {
                id: Uuid::new_v4(),
                name: "dynamic-failure".into(),
                kind: EgressKind::Dynamic,
                provider_id: Some(provider.id),
                endpoint_secret_ref: None,
                priority: 1,
                allowed_purposes: vec![EgressPurpose::ArticleDetail],
                status: EgressStatus::Active,
                last_preflight_status: None,
                last_preflight_at: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        let mut provider_adapter = DefaultProxyProviderAdapter::new(secrets);
        provider_adapter.fail_vendor = true;
        let resolver = EgressResolver::new(
            store,
            Arc::new(DynamicLeaseManager::new()),
            Arc::new(provider_adapter),
        );
        let error = resolver
            .resolve(
                ResolveRequest {
                    purpose: EgressPurpose::ArticleDetail,
                    preferred_profile_id: None,
                    bound_fixed_egress_id: None,
                },
                now,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "egress.provider_failed");
        assert!(!error.message.contains("secret-vendor"));
    }
}
