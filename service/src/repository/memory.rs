//! 内存仓储：单元测试与无 DB 启动路径。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::{AdminSession, Administrator, ScheduleSettings};
use crate::repository::administrator::{AdminSessionRecord, AdministratorRepository};
use crate::repository::audit::{AuditLogRecord, AuditRepository};
use crate::repository::settings::SettingsRepository;

#[derive(Default)]
struct Inner {
    admin: Option<Administrator>,
    sessions: Vec<AdminSession>,
    settings: ScheduleSettings,
    audits: Vec<AuditLogRecord>,
}

/// 线程安全内存存储，实现管理员/会话/设置/审计。
#[derive(Clone, Default)]
pub struct MemoryAdministratorStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryAdministratorStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn audit_count(&self) -> usize {
        self.inner.lock().await.audits.len()
    }

    pub async fn audits(&self) -> Vec<AuditLogRecord> {
        self.inner.lock().await.audits.clone()
    }
}

#[async_trait]
impl AdministratorRepository for MemoryAdministratorStore {
    async fn get_administrator(&self) -> Result<Option<Administrator>, String> {
        Ok(self.inner.lock().await.admin.clone())
    }

    async fn insert_administrator(&self, admin: &Administrator) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.admin.is_some() {
            return Err("administrator already exists".into());
        }
        g.admin = Some(admin.clone());
        Ok(())
    }

    async fn update_administrator(&self, admin: &Administrator) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.admin.is_none() {
            return Err("administrator missing".into());
        }
        g.admin = Some(admin.clone());
        Ok(())
    }

    async fn insert_session(&self, session: &AdminSessionRecord) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.sessions
            .iter()
            .any(|s| s.session_hash == session.session_hash)
        {
            return Err("duplicate session hash".into());
        }
        g.sessions.push(AdminSession {
            id: session.id,
            session_hash: session.session_hash.clone(),
            expires_at: session.expires_at,
            revoked_at: None,
            ip_hash: session.ip_hash.clone(),
            user_agent_hash: session.user_agent_hash.clone(),
            created_at: session.created_at,
        });
        Ok(())
    }

    async fn find_session_by_hash(
        &self,
        session_hash: &str,
    ) -> Result<Option<AdminSession>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .sessions
            .iter()
            .find(|s| s.session_hash == session_hash)
            .cloned())
    }

    async fn revoke_session(&self, session_id: Uuid, at: DateTime<Utc>) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if let Some(s) = g.sessions.iter_mut().find(|s| s.id == session_id) {
            if s.revoked_at.is_none() {
                s.revoked_at = Some(at);
            }
            Ok(())
        } else {
            Err("session not found".into())
        }
    }

    async fn revoke_other_sessions(
        &self,
        keep_session_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, String> {
        let mut g = self.inner.lock().await;
        let mut n = 0u64;
        for s in g.sessions.iter_mut() {
            if s.id != keep_session_id && s.revoked_at.is_none() {
                s.revoked_at = Some(at);
                n += 1;
            }
        }
        Ok(n)
    }

    async fn revoke_all_sessions(&self, at: DateTime<Utc>) -> Result<u64, String> {
        let mut g = self.inner.lock().await;
        let mut n = 0u64;
        for s in g.sessions.iter_mut() {
            if s.revoked_at.is_none() {
                s.revoked_at = Some(at);
                n += 1;
            }
        }
        Ok(n)
    }
}

#[async_trait]
impl SettingsRepository for MemoryAdministratorStore {
    async fn get_schedule(&self) -> Result<ScheduleSettings, String> {
        Ok(self.inner.lock().await.settings.clone())
    }

    async fn update_schedule(&self, settings: &ScheduleSettings) -> Result<(), String> {
        self.inner.lock().await.settings = settings.clone();
        Ok(())
    }
}

#[async_trait]
impl AuditRepository for MemoryAdministratorStore {
    async fn append(&self, record: &AuditLogRecord) -> Result<(), String> {
        self.inner.lock().await.audits.push(record.clone());
        Ok(())
    }

    async fn list_recent(&self, limit: i64) -> Result<Vec<AuditLogRecord>, String> {
        let g = self.inner.lock().await;
        let n = limit.max(0) as usize;
        Ok(g.audits.iter().rev().take(n).cloned().collect())
    }
}
