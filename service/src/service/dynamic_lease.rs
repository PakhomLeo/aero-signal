//! 动态出口租约管理：申请、释放、TTL 过期回收。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::adapters::proxy_provider::AllocatedEndpoint;
use crate::domain::egress::EgressPurpose;

/// 动态租约。
#[derive(Clone)]
pub struct DynamicLease {
    pub id: Uuid,
    pub profile_id: Uuid,
    pub purpose: EgressPurpose,
    pub proxy_url: String,
    pub provider_id: Uuid,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub released: bool,
}

impl std::fmt::Debug for DynamicLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DynamicLease")
            .field("id", &self.id)
            .field("profile_id", &self.profile_id)
            .field("purpose", &self.purpose)
            .field("proxy_url", &"[REDACTED]")
            .field("provider_id", &self.provider_id)
            .field("acquired_at", &self.acquired_at)
            .field("expires_at", &self.expires_at)
            .field("released", &self.released)
            .finish()
    }
}

impl DynamicLease {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        !self.released && now >= self.expires_at
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        !self.released && now < self.expires_at
    }
}

/// 内存租约管理器。
#[derive(Clone, Default)]
pub struct DynamicLeaseManager {
    inner: Arc<Mutex<HashMap<Uuid, DynamicLease>>>,
}

impl DynamicLeaseManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// 申请租约。
    pub async fn acquire(
        &self,
        profile_id: Uuid,
        endpoint: AllocatedEndpoint,
        now: DateTime<Utc>,
    ) -> DynamicLease {
        let id = Uuid::new_v4();
        let expires_at = now
            + chrono::Duration::from_std(endpoint.ttl).unwrap_or(chrono::Duration::seconds(300));
        let lease = DynamicLease {
            id,
            profile_id,
            purpose: endpoint.purpose,
            proxy_url: endpoint.proxy_url,
            provider_id: endpoint.provider_id,
            acquired_at: now,
            expires_at,
            released: false,
        };
        self.inner.lock().await.insert(id, lease.clone());
        lease
    }

    /// 释放租约。
    pub async fn release(&self, lease_id: Uuid) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let lease = g
            .get_mut(&lease_id)
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        if lease.released {
            return Ok(());
        }
        lease.released = true;
        // 释放后清除敏感 URL
        lease.proxy_url.clear();
        Ok(())
    }

    /// 续约：在未释放且未过期时延长 `expires_at`。
    pub async fn renew(
        &self,
        lease_id: Uuid,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> Result<DynamicLease, String> {
        let mut g = self.inner.lock().await;
        let lease = g
            .get_mut(&lease_id)
            .ok_or_else(|| format!("lease not found: {lease_id}"))?;
        if lease.released {
            return Err("lease already released".into());
        }
        if now >= lease.expires_at {
            return Err("lease already expired".into());
        }
        lease.expires_at =
            now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::seconds(300));
        Ok(lease.clone())
    }

    pub async fn get(&self, lease_id: Uuid) -> Option<DynamicLease> {
        self.inner.lock().await.get(&lease_id).cloned()
    }

    /// 回收所有已过期且未释放的租约，返回回收数量。
    pub async fn reclaim_expired(&self, now: DateTime<Utc>) -> u64 {
        let mut g = self.inner.lock().await;
        let mut n = 0u64;
        for lease in g.values_mut() {
            if lease.is_expired(now) {
                lease.released = true;
                lease.proxy_url.clear();
                n += 1;
            }
        }
        n
    }

    pub async fn active_count(&self, now: DateTime<Utc>) -> usize {
        self.inner
            .lock()
            .await
            .values()
            .filter(|l| l.is_active(now))
            .count()
    }

    /// 测试辅助：按 TTL 直接构造。
    pub async fn acquire_with_ttl(
        &self,
        profile_id: Uuid,
        purpose: EgressPurpose,
        proxy_url: impl Into<String>,
        provider_id: Uuid,
        ttl: Duration,
        now: DateTime<Utc>,
    ) -> DynamicLease {
        self.acquire(
            profile_id,
            AllocatedEndpoint {
                proxy_url: proxy_url.into(),
                ttl,
                purpose,
                provider_id,
                allocated_at: now,
            },
            now,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::egress::EgressPurpose;

    #[tokio::test]
    async fn acquire_release() {
        let mgr = DynamicLeaseManager::new();
        let now = Utc::now();
        let lease = mgr
            .acquire_with_ttl(
                Uuid::new_v4(),
                EgressPurpose::ArticleDetail,
                "http://p",
                Uuid::new_v4(),
                Duration::from_secs(60),
                now,
            )
            .await;
        assert!(lease.is_active(now));
        mgr.release(lease.id).await.unwrap();
        let got = mgr.get(lease.id).await.unwrap();
        assert!(got.released);
        assert!(got.proxy_url.is_empty());
        assert_eq!(mgr.active_count(now).await, 0);
    }

    #[tokio::test]
    async fn ttl_reclaim() {
        let mgr = DynamicLeaseManager::new();
        let now = Utc::now();
        let lease = mgr
            .acquire_with_ttl(
                Uuid::new_v4(),
                EgressPurpose::AssetDownload,
                "http://p",
                Uuid::new_v4(),
                Duration::from_secs(10),
                now,
            )
            .await;
        let later = now + chrono::Duration::seconds(11);
        assert!(lease.is_expired(later));
        let n = mgr.reclaim_expired(later).await;
        assert_eq!(n, 1);
        assert_eq!(mgr.active_count(later).await, 0);
        let got = mgr.get(lease.id).await.unwrap();
        assert!(got.released);
    }

    #[tokio::test]
    async fn renew_extends_ttl() {
        let mgr = DynamicLeaseManager::new();
        let now = Utc::now();
        let lease = mgr
            .acquire_with_ttl(
                Uuid::new_v4(),
                EgressPurpose::ArticleDetail,
                "http://p",
                Uuid::new_v4(),
                Duration::from_secs(30),
                now,
            )
            .await;
        let mid = now + chrono::Duration::seconds(10);
        let renewed = mgr
            .renew(lease.id, Duration::from_secs(60), mid)
            .await
            .unwrap();
        assert!(renewed.is_active(mid + chrono::Duration::seconds(50)));
        assert!(!renewed.is_expired(mid + chrono::Duration::seconds(50)));
    }
}
