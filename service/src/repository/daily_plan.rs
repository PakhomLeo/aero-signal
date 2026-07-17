//! 每日计划仓储 trait。

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::domain::daily_plan::{DailyFetchPlan, DailyPlanStatus};

#[async_trait]
pub trait DailyPlanRepository: Send + Sync {
    async fn list_by_date(&self, date: NaiveDate) -> Result<Vec<DailyFetchPlan>, String>;

    async fn get_by_date_source(
        &self,
        date: NaiveDate,
        source_account_id: Uuid,
    ) -> Result<Option<DailyFetchPlan>, String>;

    async fn get_by_id(&self, id: Uuid) -> Result<Option<DailyFetchPlan>, String>;

    /// 插入或覆盖：仅当不存在，或已存在且 status=planned（可重建）。
    /// 若已存在且不可重建，返回 Ok(existing) 且 `created=false`。
    async fn upsert_rebuildable(
        &self,
        plan: &DailyFetchPlan,
    ) -> Result<(DailyFetchPlan, bool), String>;

    async fn update_status(
        &self,
        id: Uuid,
        status: DailyPlanStatus,
        reason_code: Option<String>,
        fetch_job_id: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<DailyFetchPlan, String>;

    /// 删除指定日期下全部 planned 行（重建前清理，不触碰已开始）。
    async fn delete_planned_for_date(&self, date: NaiveDate) -> Result<u64, String>;
}
