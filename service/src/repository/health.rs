//! 健康快照仓储 trait。

use async_trait::async_trait;
use chrono::NaiveDate;
use uuid::Uuid;

use crate::domain::health::{HealthCheckRun, ServiceHealthComponent, ServiceHealthSnapshot};

#[async_trait]
pub trait HealthSnapshotRepository: Send + Sync {
    /// 按日期读取快照（不含组件）。
    async fn get_by_date(&self, date: NaiveDate) -> Result<Option<ServiceHealthSnapshot>, String>;

    /// 最新快照（按 snapshot_date 降序）。
    async fn get_latest(&self) -> Result<Option<ServiceHealthSnapshot>, String>;

    async fn list_components(
        &self,
        snapshot_id: Uuid,
    ) -> Result<Vec<ServiceHealthComponent>, String>;

    /// 插入快照 + 组件。若 `snapshot_date` 已存在则返回错误（禁止 update）。
    async fn insert_snapshot(
        &self,
        snapshot: &ServiceHealthSnapshot,
        components: &[ServiceHealthComponent],
    ) -> Result<(), String>;

    /// 显式禁止更新：任何 update 调用均失败。
    async fn update_snapshot(&self, _snapshot: &ServiceHealthSnapshot) -> Result<(), String> {
        Err("health.snapshot_immutable: 成功写入后禁止更新当日快照".into())
    }

    async fn insert_check_run(&self, run: &HealthCheckRun) -> Result<(), String>;

    async fn list_check_runs(&self, limit: usize) -> Result<Vec<HealthCheckRun>, String>;
}
