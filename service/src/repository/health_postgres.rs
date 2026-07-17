//! PostgreSQL 健康快照仓储。

use async_trait::async_trait;
use chrono::NaiveDate;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::health::{
    HealthCheckRun, HealthComponentStatus, HealthComponentType, OverallHealthStatus,
    ServiceHealthComponent, ServiceHealthSnapshot,
};
use crate::repository::health::HealthSnapshotRepository;

#[derive(Clone)]
pub struct PgHealthSnapshotRepository {
    pool: PgPool,
}

impl PgHealthSnapshotRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_snapshot(row: &PgRow) -> Result<ServiceHealthSnapshot, String> {
    let status_s: String = row.try_get("overall_status").map_err(|e| e.to_string())?;
    Ok(ServiceHealthSnapshot {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        snapshot_date: row.try_get("snapshot_date").map_err(|e| e.to_string())?,
        generated_at: row.try_get("generated_at").map_err(|e| e.to_string())?,
        overall_status: OverallHealthStatus::parse(&status_s)?,
        yesterday_plan_success_count: row
            .try_get("yesterday_plan_success_count")
            .map_err(|e| e.to_string())?,
        yesterday_plan_failure_count: row
            .try_get("yesterday_plan_failure_count")
            .map_err(|e| e.to_string())?,
        yesterday_article_failure_count: row
            .try_get("yesterday_article_failure_count")
            .map_err(|e| e.to_string())?,
        today_eligible_source_count: row
            .try_get("today_eligible_source_count")
            .map_err(|e| e.to_string())?,
        today_blocked_source_count: row
            .try_get("today_blocked_source_count")
            .map_err(|e| e.to_string())?,
        pending_action_count: row
            .try_get("pending_action_count")
            .map_err(|e| e.to_string())?,
        summary_json: row.try_get("summary_json").map_err(|e| e.to_string())?,
    })
}

fn map_component(row: &PgRow) -> Result<ServiceHealthComponent, String> {
    let type_s: String = row.try_get("component_type").map_err(|e| e.to_string())?;
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(ServiceHealthComponent {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        snapshot_id: row.try_get("snapshot_id").map_err(|e| e.to_string())?,
        component_type: HealthComponentType::parse(&type_s)?,
        component_id: row.try_get("component_id").map_err(|e| e.to_string())?,
        status: HealthComponentStatus::parse(&status_s)?,
        reason_code: row.try_get("reason_code").map_err(|e| e.to_string())?,
        checked_at: row.try_get("checked_at").map_err(|e| e.to_string())?,
        safe_details_json: row
            .try_get("safe_details_json")
            .map_err(|e| e.to_string())?,
    })
}

fn map_check_run(row: &PgRow) -> Result<HealthCheckRun, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(HealthCheckRun {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        job_id: row.try_get("job_id").map_err(|e| e.to_string())?,
        target_type: row.try_get("target_type").map_err(|e| e.to_string())?,
        target_id: row.try_get("target_id").map_err(|e| e.to_string())?,
        status: HealthComponentStatus::parse(&status_s)?,
        reason_code: row.try_get("reason_code").map_err(|e| e.to_string())?,
        safe_details_json: row
            .try_get("safe_details_json")
            .map_err(|e| e.to_string())?,
        checked_at: row.try_get("checked_at").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

#[async_trait]
impl HealthSnapshotRepository for PgHealthSnapshotRepository {
    async fn get_by_date(&self, date: NaiveDate) -> Result<Option<ServiceHealthSnapshot>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, snapshot_date, generated_at, overall_status,
                   yesterday_plan_success_count, yesterday_plan_failure_count,
                   yesterday_article_failure_count, today_eligible_source_count,
                   today_blocked_source_count, pending_action_count, summary_json
            FROM service_health_snapshots
            WHERE snapshot_date = $1
            "#,
        )
        .bind(date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_snapshot(&r)).transpose()
    }

    async fn get_latest(&self) -> Result<Option<ServiceHealthSnapshot>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, snapshot_date, generated_at, overall_status,
                   yesterday_plan_success_count, yesterday_plan_failure_count,
                   yesterday_article_failure_count, today_eligible_source_count,
                   today_blocked_source_count, pending_action_count, summary_json
            FROM service_health_snapshots
            ORDER BY snapshot_date DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_snapshot(&r)).transpose()
    }

    async fn list_components(
        &self,
        snapshot_id: Uuid,
    ) -> Result<Vec<ServiceHealthComponent>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, snapshot_id, component_type, component_id, status,
                   reason_code, checked_at, safe_details_json
            FROM service_health_components
            WHERE snapshot_id = $1
            ORDER BY checked_at ASC, id ASC
            "#,
        )
        .bind(snapshot_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_component).collect()
    }

    async fn insert_snapshot(
        &self,
        snapshot: &ServiceHealthSnapshot,
        components: &[ServiceHealthComponent],
    ) -> Result<(), String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let result = sqlx::query(
            r#"
            INSERT INTO service_health_snapshots (
                id, snapshot_date, generated_at, overall_status,
                yesterday_plan_success_count, yesterday_plan_failure_count,
                yesterday_article_failure_count, today_eligible_source_count,
                today_blocked_source_count, pending_action_count, summary_json
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(snapshot.id)
        .bind(snapshot.snapshot_date)
        .bind(snapshot.generated_at)
        .bind(snapshot.overall_status.as_str())
        .bind(snapshot.yesterday_plan_success_count)
        .bind(snapshot.yesterday_plan_failure_count)
        .bind(snapshot.yesterday_article_failure_count)
        .bind(snapshot.today_eligible_source_count)
        .bind(snapshot.today_blocked_source_count)
        .bind(snapshot.pending_action_count)
        .bind(&snapshot.summary_json)
        .execute(&mut *tx)
        .await;

        match result {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") || msg.contains("23505") {
                    if msg.contains("snapshot_date") || msg.contains("service_health_snapshots") {
                        return Err(format!(
                            "health.snapshot_exists: 日期 {} 已有快照，禁止覆盖",
                            snapshot.snapshot_date
                        ));
                    }
                    return Err("health.snapshot_id_exists: 快照 id 已存在".into());
                }
                return Err(msg);
            }
        }

        for c in components {
            sqlx::query(
                r#"
                INSERT INTO service_health_components (
                    id, snapshot_id, component_type, component_id, status,
                    reason_code, checked_at, safe_details_json
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
                "#,
            )
            .bind(c.id)
            .bind(c.snapshot_id)
            .bind(c.component_type.as_str())
            .bind(c.component_id)
            .bind(c.status.as_str())
            .bind(&c.reason_code)
            .bind(c.checked_at)
            .bind(&c.safe_details_json)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn insert_check_run(&self, run: &HealthCheckRun) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO health_check_runs (
                id, job_id, target_type, target_id, status, reason_code,
                safe_details_json, checked_at, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)
            "#,
        )
        .bind(run.id)
        .bind(run.job_id)
        .bind(&run.target_type)
        .bind(run.target_id)
        .bind(run.status.as_str())
        .bind(&run.reason_code)
        .bind(&run.safe_details_json)
        .bind(run.checked_at)
        .bind(run.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list_check_runs(&self, limit: usize) -> Result<Vec<HealthCheckRun>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = sqlx::query(
            r#"
            SELECT id, job_id, target_type, target_id, status, reason_code,
                   safe_details_json, checked_at, created_at
            FROM health_check_runs
            ORDER BY checked_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_check_run).collect()
    }
}
