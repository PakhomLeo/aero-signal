//! 内存出口仓储（含默认本机出口种子）。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::egress::{
    EgressKind, EgressProfile, EgressPurpose, EgressStatus, LOCAL_DIRECT_PROFILE_ID, ProxyProvider,
};
use crate::repository::egress::EgressRepository;

#[derive(Default)]
struct Inner {
    providers: Vec<ProxyProvider>,
    profiles: Vec<EgressProfile>,
}

/// 线程安全内存出口存储。
#[derive(Clone)]
pub struct MemoryEgressStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryEgressStore {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// 带默认本机出口种子（对齐 U04-07）。
    pub fn with_local_direct() -> Self {
        let now = Utc::now();
        Self {
            inner: Arc::new(Mutex::new(Inner {
                providers: Vec::new(),
                profiles: vec![EgressProfile {
                    id: LOCAL_DIRECT_PROFILE_ID,
                    name: "local_direct".into(),
                    kind: EgressKind::LocalDirect,
                    provider_id: None,
                    endpoint_secret_ref: None,
                    priority: 0,
                    allowed_purposes: vec![
                        EgressPurpose::Login,
                        EgressPurpose::Discover,
                        EgressPurpose::ArticleDetail,
                        EgressPurpose::AssetDownload,
                    ],
                    status: EgressStatus::Active,
                    last_preflight_status: None,
                    last_preflight_at: None,
                    created_at: now,
                    updated_at: now,
                }],
            })),
        }
    }
}

impl Default for MemoryEgressStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl EgressRepository for MemoryEgressStore {
    async fn list_proxy_providers(&self) -> Result<Vec<ProxyProvider>, String> {
        Ok(self.inner.lock().await.providers.clone())
    }

    async fn get_proxy_provider(&self, id: Uuid) -> Result<Option<ProxyProvider>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .providers
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    async fn insert_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.providers.iter().any(|x| x.id == p.id || x.name == p.name) {
            return Err("proxy_provider already exists".into());
        }
        g.providers.push(p.clone());
        Ok(())
    }

    async fn update_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.providers.iter().any(|x| x.id != p.id && x.name == p.name) {
            return Err("proxy_provider name conflict".into());
        }
        let slot = g
            .providers
            .iter_mut()
            .find(|x| x.id == p.id)
            .ok_or_else(|| "proxy_provider not found".to_string())?;
        *slot = p.clone();
        Ok(())
    }

    async fn list_egress_profiles(&self) -> Result<Vec<EgressProfile>, String> {
        Ok(self.inner.lock().await.profiles.clone())
    }

    async fn get_egress_profile(&self, id: Uuid) -> Result<Option<EgressProfile>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .profiles
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    async fn get_egress_profile_by_name(
        &self,
        name: &str,
    ) -> Result<Option<EgressProfile>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .profiles
            .iter()
            .find(|p| p.name == name)
            .cloned())
    }

    async fn insert_egress_profile(&self, p: &EgressProfile) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.profiles.iter().any(|x| x.id == p.id || x.name == p.name) {
            return Err("egress_profile already exists".into());
        }
        g.profiles.push(p.clone());
        Ok(())
    }

    async fn update_egress_profile(&self, p: &EgressProfile) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.profiles.iter().any(|x| x.id != p.id && x.name == p.name) {
            return Err("egress_profile name conflict".into());
        }
        let slot = g
            .profiles
            .iter_mut()
            .find(|x| x.id == p.id)
            .ok_or_else(|| "egress_profile not found".to_string())?;
        *slot = p.clone();
        Ok(())
    }
}
