//! 管理员与会话仓储接口。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::{AdminSession, Administrator};

/// 会话写入记录。
#[derive(Debug, Clone)]
pub struct AdminSessionRecord {
    pub id: Uuid,
    pub session_hash: String,
    pub expires_at: DateTime<Utc>,
    pub ip_hash: Option<String>,
    pub user_agent_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[async_trait]
pub trait AdministratorRepository: Send + Sync {
    async fn get_administrator(&self) -> Result<Option<Administrator>, String>;

    async fn insert_administrator(&self, admin: &Administrator) -> Result<(), String>;

    async fn update_administrator(&self, admin: &Administrator) -> Result<(), String>;

    async fn insert_session(&self, session: &AdminSessionRecord) -> Result<(), String>;

    async fn find_session_by_hash(
        &self,
        session_hash: &str,
    ) -> Result<Option<AdminSession>, String>;

    async fn revoke_session(&self, session_id: Uuid, at: DateTime<Utc>) -> Result<(), String>;

    /// 撤销除 `keep_session_id` 外的全部未撤销会话。
    async fn revoke_other_sessions(
        &self,
        keep_session_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, String>;

    async fn revoke_all_sessions(&self, at: DateTime<Utc>) -> Result<u64, String>;
}
