//! 分组 / Token / 调用日志仓储 trait。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::delivery::{
    ApiCallLog, ApiToken, DeliveryGroup, DeliveryGroupSource, DeliveryGroupStatus,
};

#[derive(Debug, Clone)]
pub struct CreateDeliveryGroup {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PatchDeliveryGroup {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<DeliveryGroupStatus>,
    pub now: DateTime<Utc>,
}

/// Creates a group and optionally binds its sources as one repository command.
///
/// Implementations must commit the group row and all requested bindings
/// atomically: an error must not leave a partially-created group behind.
#[derive(Debug, Clone)]
pub struct CreateDeliveryGroupWithSources {
    pub group: CreateDeliveryGroup,
    pub source_ids: Vec<Uuid>,
}

/// Updates a group and, when present, replaces its source bindings atomically.
#[derive(Debug, Clone)]
pub struct PatchDeliveryGroupWithSources {
    pub group_id: Uuid,
    pub group: PatchDeliveryGroup,
    /// `None` keeps existing bindings; `Some(empty)` clears them.
    pub source_ids: Option<Vec<Uuid>>,
}

#[async_trait]
pub trait DeliveryRepository: Send + Sync {
    async fn create_group(&self, cmd: CreateDeliveryGroup) -> Result<DeliveryGroup, String>;

    /// Atomically creates a delivery group and its initial source bindings.
    async fn create_group_with_sources(
        &self,
        cmd: CreateDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String>;

    async fn get_group(&self, id: Uuid) -> Result<Option<DeliveryGroup>, String>;

    async fn list_groups(&self) -> Result<Vec<DeliveryGroup>, String>;

    async fn patch_group(&self, id: Uuid, cmd: PatchDeliveryGroup)
    -> Result<DeliveryGroup, String>;

    /// Atomically patches group metadata and, when supplied, replaces source
    /// bindings. `Some(empty)` intentionally clears all bindings.
    async fn patch_group_with_sources(
        &self,
        cmd: PatchDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String>;

    async fn bind_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String>;

    /// 用给定集合替换绑定（不改抓取规则、不复制文章）。
    async fn set_group_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String>;

    async fn list_group_sources(&self, group_id: Uuid) -> Result<Vec<DeliveryGroupSource>, String>;

    async fn insert_token(&self, token: ApiToken) -> Result<ApiToken, String>;

    async fn get_token(&self, id: Uuid) -> Result<Option<ApiToken>, String>;

    async fn find_token_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, String>;

    async fn list_tokens_for_group(&self, group_id: Uuid) -> Result<Vec<ApiToken>, String>;

    async fn revoke_token(&self, id: Uuid, now: DateTime<Utc>) -> Result<ApiToken, String>;

    async fn touch_token_used(&self, id: Uuid, now: DateTime<Utc>) -> Result<(), String>;

    async fn insert_call_log(&self, log: ApiCallLog) -> Result<ApiCallLog, String>;

    async fn list_call_logs(&self, token_id: Uuid, limit: usize)
    -> Result<Vec<ApiCallLog>, String>;
}
