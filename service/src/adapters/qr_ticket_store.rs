//! QR ticket 存储：接口按 Redis key 设计（前缀 `signal:qr:`），默认进程内实现。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::collector::QrLoginStatus;

/// Redis / 内存 key 前缀。
pub const QR_TICKET_KEY_PREFIX: &str = "signal:qr:";

pub fn qr_ticket_key(ticket: &str) -> String {
    format!("{QR_TICKET_KEY_PREFIX}{ticket}")
}

/// Ticket 元数据（敏感 provider state 只存 secret_ref，不进 PG JSON）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrTicketRecord {
    pub ticket: String,
    pub administrator_session_hash: String,
    pub connector: String,
    pub fixed_egress_id: Uuid,
    pub provider_state_secret_ref: Option<String>,
    pub qr_data_url: String,
    pub poll_after_ms: u64,
    pub status: QrLoginStatus,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    /// finalize 成功后写入，供幂等 poll 返回。
    pub collector_account_id: Option<Uuid>,
    pub display_name: Option<String>,
    pub external_account_ref: Option<String>,
    pub last_error_code: Option<String>,
}

#[async_trait]
pub trait QrTicketStore: Send + Sync {
    async fn put(&self, record: &QrTicketRecord, ttl: Duration) -> Result<(), String>;
    async fn get(&self, ticket: &str) -> Result<Option<QrTicketRecord>, String>;
    /// CAS：仅当当前 status == expected 时更新为 next；成功返回更新后记录。
    async fn compare_and_set_status(
        &self,
        ticket: &str,
        expected: QrLoginStatus,
        next: QrLoginStatus,
    ) -> Result<Option<QrTicketRecord>, String>;
    async fn update(&self, record: &QrTicketRecord) -> Result<(), String>;
    async fn delete(&self, ticket: &str) -> Result<(), String>;
}

struct MemoryEntry {
    record: QrTicketRecord,
    expires_at: DateTime<Utc>,
}

/// 进程内 ticket 存储（接口对齐 Redis key）。
#[derive(Clone, Default)]
pub struct MemoryQrTicketStore {
    inner: Arc<Mutex<HashMap<String, MemoryEntry>>>,
}

impl MemoryQrTicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn purge_expired(map: &mut HashMap<String, MemoryEntry>, now: DateTime<Utc>) {
        map.retain(|_, e| e.expires_at > now);
    }
}

#[async_trait]
impl QrTicketStore for MemoryQrTicketStore {
    async fn put(&self, record: &QrTicketRecord, ttl: Duration) -> Result<(), String> {
        let now = Utc::now();
        let expires_at =
            now + chrono::Duration::from_std(ttl).unwrap_or(chrono::Duration::seconds(120));
        let mut guard = self.inner.lock().await;
        Self::purge_expired(&mut guard, now);
        guard.insert(
            qr_ticket_key(&record.ticket),
            MemoryEntry {
                record: record.clone(),
                expires_at,
            },
        );
        Ok(())
    }

    async fn get(&self, ticket: &str) -> Result<Option<QrTicketRecord>, String> {
        let now = Utc::now();
        let mut guard = self.inner.lock().await;
        Self::purge_expired(&mut guard, now);
        Ok(guard.get(&qr_ticket_key(ticket)).map(|e| e.record.clone()))
    }

    async fn compare_and_set_status(
        &self,
        ticket: &str,
        expected: QrLoginStatus,
        next: QrLoginStatus,
    ) -> Result<Option<QrTicketRecord>, String> {
        let now = Utc::now();
        let mut guard = self.inner.lock().await;
        Self::purge_expired(&mut guard, now);
        let key = qr_ticket_key(ticket);
        let Some(entry) = guard.get_mut(&key) else {
            return Ok(None);
        };
        if entry.record.status != expected {
            return Ok(None);
        }
        entry.record.status = entry.record.status.transition(next)?;
        Ok(Some(entry.record.clone()))
    }

    async fn update(&self, record: &QrTicketRecord) -> Result<(), String> {
        let now = Utc::now();
        let mut guard = self.inner.lock().await;
        Self::purge_expired(&mut guard, now);
        let key = qr_ticket_key(&record.ticket);
        if let Some(entry) = guard.get_mut(&key) {
            entry.record = record.clone();
            Ok(())
        } else {
            Err("ticket 不存在或已过期".into())
        }
    }

    async fn delete(&self, ticket: &str) -> Result<(), String> {
        self.inner.lock().await.remove(&qr_ticket_key(ticket));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::collector::QrLoginStatus;

    fn sample(ticket: &str) -> QrTicketRecord {
        let now = Utc::now();
        QrTicketRecord {
            ticket: ticket.into(),
            administrator_session_hash: "hash".into(),
            connector: "fake".into(),
            fixed_egress_id: Uuid::nil(),
            provider_state_secret_ref: Some("sec_x".into()),
            qr_data_url: "data:image/png;base64,x".into(),
            poll_after_ms: 1000,
            status: QrLoginStatus::QrWaiting,
            expires_at: now + chrono::Duration::seconds(120),
            created_at: now,
            collector_account_id: None,
            display_name: None,
            external_account_ref: None,
            last_error_code: None,
        }
    }

    #[tokio::test]
    async fn cas_prevents_double_finalize() {
        let store = MemoryQrTicketStore::new();
        let rec = sample("t1");
        store.put(&rec, Duration::from_secs(60)).await.unwrap();
        let a = store
            .compare_and_set_status("t1", QrLoginStatus::QrWaiting, QrLoginStatus::QrScanned)
            .await
            .unwrap();
        assert!(a.is_some());
        let b = store
            .compare_and_set_status("t1", QrLoginStatus::QrWaiting, QrLoginStatus::QrScanned)
            .await
            .unwrap();
        assert!(b.is_none());
    }
}
