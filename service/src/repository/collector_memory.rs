//! 内存采集账号仓储。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::collector::{CollectorAccount, CollectorEgressBinding, CollectorLoginSession};
use crate::repository::collector::CollectorRepository;

#[derive(Default)]
struct Inner {
    accounts: Vec<CollectorAccount>,
    sessions: Vec<CollectorLoginSession>,
    bindings: Vec<CollectorEgressBinding>,
}

#[derive(Clone, Default)]
pub struct MemoryCollectorStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryCollectorStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl CollectorRepository for MemoryCollectorStore {
    async fn list_accounts(&self) -> Result<Vec<CollectorAccount>, String> {
        Ok(self.inner.lock().await.accounts.clone())
    }

    async fn get_account(&self, id: Uuid) -> Result<Option<CollectorAccount>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .accounts
            .iter()
            .find(|a| a.id == id)
            .cloned())
    }

    async fn insert_account(&self, account: &CollectorAccount) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.accounts.iter().any(|a| a.id == account.id) {
            return Err("采集账号已存在".into());
        }
        g.accounts.push(account.clone());
        Ok(())
    }

    async fn update_account(&self, account: &CollectorAccount) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let Some(slot) = g.accounts.iter_mut().find(|a| a.id == account.id) else {
            return Err("采集账号不存在".into());
        };
        *slot = account.clone();
        Ok(())
    }

    async fn get_active_login_session(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorLoginSession>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .sessions
            .iter()
            .find(|s| s.collector_account_id == collector_account_id && s.revoked_at.is_none())
            .cloned())
    }

    async fn insert_login_session(&self, session: &CollectorLoginSession) -> Result<(), String> {
        self.inner.lock().await.sessions.push(session.clone());
        Ok(())
    }

    async fn revoke_login_sessions(
        &self,
        collector_account_id: Uuid,
        revoked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        for s in g.sessions.iter_mut() {
            if s.collector_account_id == collector_account_id && s.revoked_at.is_none() {
                s.revoked_at = Some(revoked_at);
            }
        }
        Ok(())
    }

    async fn get_egress_binding(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorEgressBinding>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .bindings
            .iter()
            .find(|b| b.collector_account_id == collector_account_id)
            .cloned())
    }

    async fn upsert_egress_binding(&self, binding: &CollectorEgressBinding) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if let Some(slot) = g
            .bindings
            .iter_mut()
            .find(|b| b.collector_account_id == binding.collector_account_id)
        {
            *slot = binding.clone();
        } else {
            g.bindings.push(binding.clone());
        }
        Ok(())
    }

    async fn persist_login_success(
        &self,
        account: &CollectorAccount,
        session: &CollectorLoginSession,
        binding: &CollectorEgressBinding,
        revoke_previous: bool,
    ) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        let now = Utc::now();
        if let Some(slot) = g.accounts.iter_mut().find(|a| a.id == account.id) {
            *slot = account.clone();
        } else {
            g.accounts.push(account.clone());
        }
        if revoke_previous {
            for s in g.sessions.iter_mut() {
                if s.collector_account_id == account.id && s.revoked_at.is_none() {
                    s.revoked_at = Some(now);
                }
            }
        }
        g.sessions.push(session.clone());
        if let Some(slot) = g
            .bindings
            .iter_mut()
            .find(|b| b.collector_account_id == binding.collector_account_id)
        {
            *slot = binding.clone();
        } else {
            g.bindings.push(binding.clone());
        }
        Ok(())
    }
}
