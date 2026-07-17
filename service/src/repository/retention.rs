//! 保留策略仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::retention::{RetentionCandidate, RetentionPolicy, RetentionTargetKind};

#[async_trait]
pub trait RetentionRepository: Send + Sync {
    async fn list_policies(&self) -> Result<Vec<RetentionPolicy>, String>;

    async fn upsert_policy(&self, policy: &RetentionPolicy) -> Result<(), String>;

    /// 列出超过保留期的候选（不含引用检查）。
    async fn list_expired_candidates(
        &self,
        kind: RetentionTargetKind,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RetentionCandidate>, String>;

    /// 物理删除已通过引用检查的记录。
    async fn hard_delete(&self, kind: RetentionTargetKind, id: Uuid) -> Result<bool, String>;

    /// 资产是否仍被当前文章 revision 的 asset_refs / OCR / 未投递 outbox 引用。
    async fn is_asset_referenced(&self, asset_id: Uuid) -> Result<bool, String>;

    /// outbox 是否仍为 pending/delivering（不可静默删）。
    async fn is_outbox_undelivered(&self, outbox_id: Uuid) -> Result<bool, String>;
}
