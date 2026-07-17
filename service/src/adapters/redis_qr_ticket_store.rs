//! Redis QR ticket 存储：多进程 HTTP + Worker 共享短期会话。

use std::time::Duration;

use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use serde_json;

use crate::domain::collector::QrLoginStatus;

use super::qr_ticket_store::{QrTicketRecord, QrTicketStore, qr_ticket_key};

/// Redis 实现；key 形如 `{prefix}qr:{ticket}`。
#[derive(Clone)]
pub struct RedisQrTicketStore {
    conn: ConnectionManager,
    key_prefix: String,
}

impl RedisQrTicketStore {
    pub async fn connect(redis_url: &str, key_prefix: &str) -> Result<Self, String> {
        let client = redis::Client::open(redis_url).map_err(|e| e.to_string())?;
        let conn = ConnectionManager::new(client)
            .await
            .map_err(|e| format!("连接 Redis 失败: {e}"))?;
        Ok(Self {
            conn,
            key_prefix: key_prefix.to_string(),
        })
    }

    fn full_key(&self, ticket: &str) -> String {
        format!("{}{}", self.key_prefix, qr_ticket_key(ticket))
    }

    async fn get_raw(&self, ticket: &str) -> Result<Option<QrTicketRecord>, String> {
        let key = self.full_key(ticket);
        let mut conn = self.conn.clone();
        let raw: Option<String> = conn.get(&key).await.map_err(|e| e.to_string())?;
        match raw {
            Some(s) => serde_json::from_str(&s)
                .map_err(|e| e.to_string())
                .map(Some),
            None => Ok(None),
        }
    }

    async fn set_raw(&self, record: &QrTicketRecord, ttl: Option<Duration>) -> Result<(), String> {
        let key = self.full_key(&record.ticket);
        let payload = serde_json::to_string(record).map_err(|e| e.to_string())?;
        let mut conn = self.conn.clone();
        if let Some(ttl) = ttl {
            let secs = ttl.as_secs().max(1);
            conn.set_ex::<_, _, ()>(&key, payload, secs)
                .await
                .map_err(|e| e.to_string())?;
        } else {
            conn.set::<_, _, ()>(&key, payload)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[async_trait]
impl QrTicketStore for RedisQrTicketStore {
    async fn put(&self, record: &QrTicketRecord, ttl: Duration) -> Result<(), String> {
        self.set_raw(record, Some(ttl)).await
    }

    async fn get(&self, ticket: &str) -> Result<Option<QrTicketRecord>, String> {
        self.get_raw(ticket).await
    }

    async fn compare_and_set_status(
        &self,
        ticket: &str,
        expected: QrLoginStatus,
        next: QrLoginStatus,
    ) -> Result<Option<QrTicketRecord>, String> {
        let key = self.full_key(ticket);
        let mut conn = self.conn.clone();
        // Keep the read/check/write sequence inside Redis so concurrent
        // workers cannot both win the same status transition.
        let script = redis::Script::new(
            r#"
local raw = redis.call('GET', KEYS[1])
if not raw then
    return nil
end
local record = cjson.decode(raw)
if record.status ~= ARGV[1] then
    return nil
end
local allowed = {
    qr_waiting = { qr_scanned = true, expired = true, cancelled = true, failed = true },
    qr_scanned = { finalizing = true, expired = true, cancelled = true, failed = true },
    finalizing = { active = true, failed = true }
}
if not allowed[ARGV[1]] or not allowed[ARGV[1]][ARGV[2]] then
    return redis.error_reply('invalid QR login status transition')
end
record.status = ARGV[2]
local updated = cjson.encode(record)
redis.call('SET', KEYS[1], updated, 'KEEPTTL')
return updated
"#,
        );
        let raw: Option<String> = script
            .key(key)
            .arg(expected.as_str())
            .arg(next.as_str())
            .invoke_async(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        raw.map(|payload| serde_json::from_str(&payload).map_err(|e| e.to_string()))
            .transpose()
    }

    async fn update(&self, record: &QrTicketRecord) -> Result<(), String> {
        let key = self.full_key(&record.ticket);
        let mut conn = self.conn.clone();
        let ttl: i64 = conn.ttl(&key).await.map_err(|e| e.to_string())?;
        let remaining = if ttl > 0 {
            Some(Duration::from_secs(ttl as u64))
        } else {
            None
        };
        self.set_raw(record, remaining).await
    }

    async fn delete(&self, ticket: &str) -> Result<(), String> {
        let key = self.full_key(ticket);
        let mut conn = self.conn.clone();
        conn.del::<_, ()>(&key).await.map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::qr_ticket_store::MemoryQrTicketStore;
    use chrono::Utc;
    use uuid::Uuid;

    async fn redis_store() -> Option<RedisQrTicketStore> {
        let url = match std::env::var("REDIS_URL") {
            Ok(url) if !url.is_empty() => url,
            _ => {
                eprintln!("skip Redis test: REDIS_URL is not set");
                return None;
            }
        };
        Some(
            RedisQrTicketStore::connect(&url, "signal:test:")
                .await
                .expect("connect to test Redis"),
        )
    }

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
    async fn redis_roundtrip_when_available() {
        let url = match std::env::var("REDIS_URL") {
            Ok(u) if !u.is_empty() => u,
            _ => {
                eprintln!("跳过 redis 测试：未设置 REDIS_URL");
                return;
            }
        };
        let store = RedisQrTicketStore::connect(&url, "signal:test:")
            .await
            .unwrap();
        let rec = sample("redis-t1");
        store.put(&rec, Duration::from_secs(60)).await.unwrap();
        let got = store.get("redis-t1").await.unwrap().expect("ticket");
        assert_eq!(got.ticket, "redis-t1");
        store.delete("redis-t1").await.unwrap();

        // 与内存实现行为对齐：CAS 防双 finalize
        let mem = MemoryQrTicketStore::new();
        mem.put(&rec, Duration::from_secs(60)).await.unwrap();
        assert!(
            mem.compare_and_set_status(
                "redis-t1",
                QrLoginStatus::QrWaiting,
                QrLoginStatus::QrScanned
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn redis_cas_has_only_one_concurrent_winner_when_available() {
        let Some(store) = redis_store().await else {
            return;
        };
        let ticket = format!("concurrent-cas-{}", Uuid::new_v4());
        store
            .put(&sample(&ticket), Duration::from_secs(60))
            .await
            .unwrap();

        let first_store = store.clone();
        let first_ticket = ticket.clone();
        let second_store = store.clone();
        let second_ticket = ticket.clone();
        let (first, second) = tokio::join!(
            first_store.compare_and_set_status(
                &first_ticket,
                QrLoginStatus::QrWaiting,
                QrLoginStatus::QrScanned,
            ),
            second_store.compare_and_set_status(
                &second_ticket,
                QrLoginStatus::QrWaiting,
                QrLoginStatus::QrScanned,
            )
        );

        let winners = [first.unwrap(), second.unwrap()]
            .into_iter()
            .filter(Option::is_some)
            .count();
        assert_eq!(winners, 1);
        assert_eq!(
            store.get(&ticket).await.unwrap().unwrap().status,
            QrLoginStatus::QrScanned
        );
        store.delete(&ticket).await.unwrap();
    }

    #[tokio::test]
    async fn redis_cas_preserves_ttl_and_delete_removes_key_when_available() {
        let Some(store) = redis_store().await else {
            return;
        };
        let ticket = format!("ttl-cas-{}", Uuid::new_v4());
        store
            .put(&sample(&ticket), Duration::from_secs(30))
            .await
            .unwrap();
        let key = store.full_key(&ticket);
        let mut conn = store.conn.clone();
        let before_ms: i64 = conn.pttl(&key).await.unwrap();

        assert!(
            store
                .compare_and_set_status(
                    &ticket,
                    QrLoginStatus::QrWaiting,
                    QrLoginStatus::QrScanned,
                )
                .await
                .unwrap()
                .is_some()
        );
        let after_ms: i64 = conn.pttl(&key).await.unwrap();
        assert!(before_ms > 0);
        assert!(after_ms > 0);
        assert!(after_ms <= before_ms);
        assert!(before_ms - after_ms < 2_000);

        store.delete(&ticket).await.unwrap();
        let deleted_ttl: i64 = conn.pttl(&key).await.unwrap();
        assert_eq!(deleted_ttl, -2);
    }
}
