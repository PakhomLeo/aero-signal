//! 审计仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AuditLogRecord {
    pub id: Uuid,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub result: String,
    pub details_json: Value,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AuditRepository: Send + Sync {
    async fn append(&self, record: &AuditLogRecord) -> Result<(), String>;

    async fn list_recent(&self, limit: i64) -> Result<Vec<AuditLogRecord>, String>;
}
