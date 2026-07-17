//! 内存每日计划仓储。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::domain::daily_plan::{DailyFetchPlan, DailyPlanStatus};
use crate::repository::daily_plan::DailyPlanRepository;

#[derive(Default)]
struct State {
    by_id: HashMap<Uuid, DailyFetchPlan>,
    /// (plan_date, source_account_id) → id
    by_key: HashMap<(NaiveDate, Uuid), Uuid>,
}

#[derive(Default, Clone)]
pub struct MemoryDailyPlanStore {
    state: Arc<RwLock<State>>,
}

impl MemoryDailyPlanStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl DailyPlanRepository for MemoryDailyPlanStore {
    async fn list_by_date(&self, date: NaiveDate) -> Result<Vec<DailyFetchPlan>, String> {
        let s = self.state.read().await;
        let mut list: Vec<_> = s
            .by_id
            .values()
            .filter(|p| p.plan_date == date)
            .cloned()
            .collect();
        list.sort_by_key(|p| p.planned_run_at);
        Ok(list)
    }

    async fn get_by_date_source(
        &self,
        date: NaiveDate,
        source_account_id: Uuid,
    ) -> Result<Option<DailyFetchPlan>, String> {
        let s = self.state.read().await;
        Ok(s.by_key
            .get(&(date, source_account_id))
            .and_then(|id| s.by_id.get(id).cloned()))
    }

    async fn get_by_id(&self, id: Uuid) -> Result<Option<DailyFetchPlan>, String> {
        let s = self.state.read().await;
        Ok(s.by_id.get(&id).cloned())
    }

    async fn upsert_rebuildable(
        &self,
        plan: &DailyFetchPlan,
    ) -> Result<(DailyFetchPlan, bool), String> {
        let mut s = self.state.write().await;
        let key = (plan.plan_date, plan.source_account_id);
        if let Some(existing_id) = s.by_key.get(&key).copied() {
            let existing = s
                .by_id
                .get(&existing_id)
                .cloned()
                .ok_or_else(|| "daily_plan.corrupt: 索引损坏".to_string())?;
            if !existing.status.is_rebuildable() {
                return Ok((existing, false));
            }
            // 覆盖 planned 行，保留原 id
            let mut updated = plan.clone();
            updated.id = existing.id;
            updated.created_at = existing.created_at;
            s.by_id.insert(updated.id, updated.clone());
            return Ok((updated, true));
        }
        s.by_key.insert(key, plan.id);
        s.by_id.insert(plan.id, plan.clone());
        Ok((plan.clone(), true))
    }

    async fn update_status(
        &self,
        id: Uuid,
        status: DailyPlanStatus,
        reason_code: Option<String>,
        fetch_job_id: Option<Uuid>,
        now: DateTime<Utc>,
    ) -> Result<DailyFetchPlan, String> {
        let mut s = self.state.write().await;
        let plan = s
            .by_id
            .get_mut(&id)
            .ok_or_else(|| "daily_plan.not_found: 计划不存在".to_string())?;
        if plan.status != status {
            plan.status = plan.status.transition(status)?;
        }
        if let Some(r) = reason_code {
            plan.reason_code = r;
        }
        if let Some(jid) = fetch_job_id {
            plan.fetch_job_id = Some(jid);
        }
        plan.updated_at = now;
        Ok(plan.clone())
    }

    async fn delete_planned_for_date(&self, date: NaiveDate) -> Result<u64, String> {
        let mut s = self.state.write().await;
        let ids: Vec<Uuid> = s
            .by_id
            .values()
            .filter(|p| p.plan_date == date && p.status.is_rebuildable())
            .map(|p| p.id)
            .collect();
        let n = ids.len() as u64;
        for id in ids {
            if let Some(p) = s.by_id.remove(&id) {
                s.by_key.remove(&(p.plan_date, p.source_account_id));
            }
        }
        Ok(n)
    }
}
