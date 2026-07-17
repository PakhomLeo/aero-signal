//! 采集账号仓储 trait。

use async_trait::async_trait;
use uuid::Uuid;

use crate::domain::collector::{CollectorAccount, CollectorEgressBinding, CollectorLoginSession};

/// 采集账号 + 登录会话 + 出口绑定。
#[async_trait]
pub trait CollectorRepository: Send + Sync {
    async fn list_accounts(&self) -> Result<Vec<CollectorAccount>, String>;
    async fn get_account(&self, id: Uuid) -> Result<Option<CollectorAccount>, String>;
    async fn insert_account(&self, account: &CollectorAccount) -> Result<(), String>;
    async fn update_account(&self, account: &CollectorAccount) -> Result<(), String>;

    async fn get_active_login_session(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorLoginSession>, String>;
    async fn insert_login_session(&self, session: &CollectorLoginSession) -> Result<(), String>;
    async fn revoke_login_sessions(
        &self,
        collector_account_id: Uuid,
        revoked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), String>;

    async fn get_egress_binding(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorEgressBinding>, String>;
    async fn upsert_egress_binding(&self, binding: &CollectorEgressBinding) -> Result<(), String>;

    /// probe 成功后事务写入三表（内存实现为顺序写）。
    async fn persist_login_success(
        &self,
        account: &CollectorAccount,
        session: &CollectorLoginSession,
        binding: &CollectorEgressBinding,
        revoke_previous: bool,
    ) -> Result<(), String>;
}
