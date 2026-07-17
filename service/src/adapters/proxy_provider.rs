//! ProxyProvider 适配器：统一产出动态租约端点信息。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::adapters::secret_store::SecretStore;
use crate::domain::egress::{EgressPurpose, ProxyProvider, ProxyProviderMode};

/// 供应商分配出的端点（含敏感 URL，仅内部使用）。
#[derive(Clone)]
pub struct AllocatedEndpoint {
    pub proxy_url: String,
    pub ttl: Duration,
    pub purpose: EgressPurpose,
    pub provider_id: Uuid,
    pub allocated_at: DateTime<Utc>,
}

impl std::fmt::Debug for AllocatedEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AllocatedEndpoint")
            .field("proxy_url", &"[REDACTED]")
            .field("ttl", &self.ttl)
            .field("purpose", &self.purpose)
            .field("provider_id", &self.provider_id)
            .field("allocated_at", &self.allocated_at)
            .finish()
    }
}

/// 适配器错误。
#[derive(Debug, Clone, thiserror::Error)]
pub enum ProxyProviderError {
    #[error("供应商未启用")]
    Disabled,
    #[error("缺少端点密钥引用")]
    MissingEndpointSecret,
    #[error("密钥不存在: {0}")]
    SecretNotFound(String),
    #[error("供应商失败: {0}")]
    VendorFailed(String),
    #[error("不支持的模式")]
    UnsupportedMode,
}

/// 代理供应商适配器。
#[async_trait]
pub trait ProxyProviderAdapter: Send + Sync {
    async fn allocate(
        &self,
        provider: &ProxyProvider,
        endpoint_secret_ref: Option<&str>,
        purpose: EgressPurpose,
        now: DateTime<Utc>,
    ) -> Result<AllocatedEndpoint, ProxyProviderError>;
}

/// 默认实现：static_endpoint / gateway 从 SecretStore 取 URL；vendor_api 可 fake。
pub struct DefaultProxyProviderAdapter {
    secrets: Arc<dyn SecretStore>,
    /// vendor_api 失败开关（测试用）。
    pub fail_vendor: bool,
    /// 默认 TTL。
    pub default_ttl: Duration,
}

impl DefaultProxyProviderAdapter {
    pub fn new(secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            secrets,
            fail_vendor: false,
            default_ttl: Duration::from_secs(300),
        }
    }
}

#[async_trait]
impl ProxyProviderAdapter for DefaultProxyProviderAdapter {
    async fn allocate(
        &self,
        provider: &ProxyProvider,
        endpoint_secret_ref: Option<&str>,
        purpose: EgressPurpose,
        now: DateTime<Utc>,
    ) -> Result<AllocatedEndpoint, ProxyProviderError> {
        if !provider.enabled {
            return Err(ProxyProviderError::Disabled);
        }

        match provider.mode {
            ProxyProviderMode::StaticEndpoint | ProxyProviderMode::Gateway => {
                let secret_ref = endpoint_secret_ref
                    .or(provider.auth_secret_ref.as_deref())
                    .ok_or(ProxyProviderError::MissingEndpointSecret)?;
                let url = self
                    .secrets
                    .get(secret_ref)
                    .await
                    .map_err(ProxyProviderError::VendorFailed)?
                    .ok_or_else(|| ProxyProviderError::SecretNotFound(secret_ref.to_string()))?;
                Ok(AllocatedEndpoint {
                    proxy_url: url,
                    ttl: self.default_ttl,
                    purpose,
                    provider_id: provider.id,
                    allocated_at: now,
                })
            }
            ProxyProviderMode::VendorApi => {
                if self.fail_vendor {
                    return Err(ProxyProviderError::VendorFailed(
                        "vendor_api fake failure".into(),
                    ));
                }
                // Fake：基于 api_base_url 生成临时 URL，不把 auth 明文拼进响应。
                let base = provider
                    .api_base_url
                    .as_deref()
                    .unwrap_or("http://vendor.local");
                let lease_id = Uuid::new_v4().simple();
                Ok(AllocatedEndpoint {
                    proxy_url: format!("{base}/lease/{lease_id}"),
                    ttl: self.default_ttl,
                    purpose,
                    provider_id: provider.id,
                    allocated_at: now,
                })
            }
        }
    }
}

/// 出口连通性测试的结构化结果（不含密钥）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressTestResult {
    pub ok: bool,
    pub profile_id: Uuid,
    pub kind: String,
    pub purpose: Option<String>,
    pub latency_ms: u64,
    pub message: String,
    pub error_code: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::secret_store::MemorySecretStore;
    use crate::domain::egress::ProxyProviderMode;
    use serde_json::json;

    fn provider(mode: ProxyProviderMode, auth: Option<String>) -> ProxyProvider {
        let now = Utc::now();
        ProxyProvider {
            id: Uuid::new_v4(),
            name: "p".into(),
            mode,
            api_base_url: Some("http://gw.example".into()),
            auth_secret_ref: auth,
            request_template_json: json!({}),
            response_mapping_json: json!({}),
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn static_endpoint_reads_secret() {
        let secrets = Arc::new(MemorySecretStore::new());
        let r = secrets.put("http://proxy:1").await.unwrap();
        let adapter = DefaultProxyProviderAdapter::new(secrets);
        let p = provider(ProxyProviderMode::StaticEndpoint, None);
        let ep = adapter
            .allocate(&p, Some(&r), EgressPurpose::ArticleDetail, Utc::now())
            .await
            .unwrap();
        assert_eq!(ep.proxy_url, "http://proxy:1");
    }

    #[tokio::test]
    async fn vendor_api_can_fail() {
        let secrets = Arc::new(MemorySecretStore::new());
        let mut adapter = DefaultProxyProviderAdapter::new(secrets);
        adapter.fail_vendor = true;
        let p = provider(ProxyProviderMode::VendorApi, None);
        let err = adapter
            .allocate(&p, None, EgressPurpose::AssetDownload, Utc::now())
            .await
            .unwrap_err();
        assert!(matches!(err, ProxyProviderError::VendorFailed(_)));
    }
}
