//! PostgreSQL 每日计划仓储。

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::daily_plan::{DailyFetchPlan, DailyPlanStatus};
use crate::repository::daily_plan::DailyPlanRepository;

#[derive(Clone)]
pub struct PgDailyPlanRepository {
    pool: PgPool,
}

impl PgDailyPlanRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_plan(row: &PgRow) -> Result<DailyFetchPlan, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(DailyFetchPlan {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        plan_date: row.try_get("plan_date").map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        health_snapshot_id: row
            .try_get("health_snapshot_id")
            .map_err(|e| e.to_string())?,
        planned_run_at: row.try_get("planned_run_at").map_err(|e| e.to_string())?,
        status: DailyPlanStatus::parse(&status_s)?,
        reason_code: row.try_get("reason_code").map_err(|e| e.to_string())?,
        heartbeat_event_id: row
            .try_get("heartbeat_event_id")
            .map_err(|e| e.to_string())?,
        fetch_job_id: row.try_get("fetch_job_id").map_err(|e| e.to_string())?,
        collector_account_id: row
            .try_get("collector_account_id")
            .map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

#[async_trait]
impl DailyPlanRepository for PgDailyPlanRepository {
    async fn list_by_date(&self, date: NaiveDate) -> Result<Vec<DailyFetchPlan>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                   status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                   created_at, updated_at
            FROM daily_fetch_plans
            WHERE plan_date = $1
            ORDER BY planned_run_at ASC
            "#,
        )
        .bind(date)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_plan).collect()
    }

    async fn get_by_date_source(
        &self,
        date: NaiveDate,
        source_account_id: Uuid,
    ) -> Result<Option<DailyFetchPlan>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                   status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                   created_at, updated_at
            FROM daily_fetch_plans
            WHERE plan_date = $1 AND source_account_id = $2
            "#,
        )
        .bind(date)
        .bind(source_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_plan(&r)).transpose()
    }

    async fn get_by_id(&self, id: Uuid) -> Result<Option<DailyFetchPlan>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                   status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                   created_at, updated_at
            FROM daily_fetch_plans
            WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_plan(&r)).transpose()
    }

    async fn upsert_rebuildable(
        &self,
        plan: &DailyFetchPlan,
    ) -> Result<(DailyFetchPlan, bool), String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let existing = sqlx::query(
            r#"
            SELECT id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                   status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                   created_at, updated_at
            FROM daily_fetch_plans
            WHERE plan_date = $1 AND source_account_id = $2
            FOR UPDATE
            "#,
        )
        .bind(plan.plan_date)
        .bind(plan.source_account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(row) = existing {
            let existing = map_plan(&row)?;
            if !existing.status.is_rebuildable() {
                tx.commit().await.map_err(|e| e.to_string())?;
                return Ok((existing, false));
            }

            // 覆盖 planned 行，保留原 id / created_at
            sqlx::query(
                r#"
                UPDATE daily_fetch_plans SET
                    health_snapshot_id = $2,
                    planned_run_at = $3,
                    status = $4,
                    reason_code = $5,
                    heartbeat_event_id = $6,
                    fetch_job_id = $7,
                    collector_account_id = $8,
                    updated_at = $9
                WHERE id = $1
                "#,
            )
            .bind(existing.id)
            .bind(plan.health_snapshot_id)
            .bind(plan.planned_run_at)
            .bind(plan.status.as_str())
            .bind(&plan.reason_code)
            .bind(plan.heartbeat_event_id)
            .bind(plan.fetch_job_id)
            .bind(plan.collector_account_id)
            .bind(plan.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

            let updated = DailyFetchPlan {
                id: existing.id,
                plan_date: existing.plan_date,
                source_account_id: existing.source_account_id,
                health_snapshot_id: plan.health_snapshot_id,
                planned_run_at: plan.planned_run_at,
                status: plan.status,
                reason_code: plan.reason_code.clone(),
                heartbeat_event_id: plan.heartbeat_event_id,
                fetch_job_id: plan.fetch_job_id,
                collector_account_id: plan.collector_account_id,
                created_at: existing.created_at,
                updated_at: plan.updated_at,
            };
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok((updated, true));
        }

        sqlx::query(
            r#"
            INSERT INTO daily_fetch_plans (
                id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            "#,
        )
        .bind(plan.id)
        .bind(plan.plan_date)
        .bind(plan.source_account_id)
        .bind(plan.health_snapshot_id)
        .bind(plan.planned_run_at)
        .bind(plan.status.as_str())
        .bind(&plan.reason_code)
        .bind(plan.heartbeat_event_id)
        .bind(plan.fetch_job_id)
        .bind(plan.collector_account_id)
        .bind(plan.created_at)
        .bind(plan.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;
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
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let row = sqlx::query(
            r#"
            SELECT id, plan_date, source_account_id, health_snapshot_id, planned_run_at,
                   status, reason_code, heartbeat_event_id, fetch_job_id, collector_account_id,
                   created_at, updated_at
            FROM daily_fetch_plans
            WHERE id = $1
            FOR UPDATE
            "#,
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "daily_plan.not_found: 计划不存在".to_string())?;

        let mut plan = map_plan(&row)?;
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

        sqlx::query(
            r#"
            UPDATE daily_fetch_plans SET
                status = $2,
                reason_code = $3,
                fetch_job_id = $4,
                updated_at = $5
            WHERE id = $1
            "#,
        )
        .bind(plan.id)
        .bind(plan.status.as_str())
        .bind(&plan.reason_code)
        .bind(plan.fetch_job_id)
        .bind(plan.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(plan)
    }

    async fn delete_planned_for_date(&self, date: NaiveDate) -> Result<u64, String> {
        let r = sqlx::query(
            r#"
            DELETE FROM daily_fetch_plans
            WHERE plan_date = $1 AND status = 'planned'
            "#,
        )
        .bind(date)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(r.rows_affected())
    }
}
