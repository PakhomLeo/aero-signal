//! PostgreSQL 管理员/会话/设置/审计仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::{AdminSession, Administrator, ScheduleSettings, WorkingWindow};
use crate::repository::administrator::{AdminSessionRecord, AdministratorRepository};
use crate::repository::audit::{AuditLogRecord, AuditRepository};
use crate::repository::settings::SettingsRepository;

#[derive(Clone)]
pub struct PgAdministratorRepository {
    pool: PgPool,
}

impl PgAdministratorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_admin(row: &PgRow) -> Result<Administrator, String> {
    Ok(Administrator {
        username: row.try_get("username").map_err(|e| e.to_string())?,
        password_hash: row.try_get("password_hash").map_err(|e| e.to_string())?,
        must_change_password: row
            .try_get("must_change_password")
            .map_err(|e| e.to_string())?,
        password_changed_at: row
            .try_get("password_changed_at")
            .map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_session(row: &PgRow) -> Result<AdminSession, String> {
    Ok(AdminSession {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        session_hash: row.try_get("session_hash").map_err(|e| e.to_string())?,
        expires_at: row.try_get("expires_at").map_err(|e| e.to_string())?,
        revoked_at: row.try_get("revoked_at").map_err(|e| e.to_string())?,
        ip_hash: row.try_get("ip_hash").map_err(|e| e.to_string())?,
        user_agent_hash: row.try_get("user_agent_hash").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

#[async_trait]
impl AdministratorRepository for PgAdministratorRepository {
    async fn get_administrator(&self) -> Result<Option<Administrator>, String> {
        let row = sqlx::query(
            r#"
            SELECT username, password_hash, must_change_password,
                   password_changed_at, created_at, updated_at
            FROM system_administrator
            WHERE singleton = TRUE
            "#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_admin(&r)).transpose()
    }

    async fn insert_administrator(&self, admin: &Administrator) -> Result<(), String> {
        let result = sqlx::query(
            r#"
            INSERT INTO system_administrator (
                singleton, username, password_hash, must_change_password,
                password_changed_at, created_at, updated_at
            ) VALUES (TRUE, $1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(&admin.username)
        .bind(&admin.password_hash)
        .bind(admin.must_change_password)
        .bind(admin.password_changed_at)
        .bind(admin.created_at)
        .bind(admin.updated_at)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") || msg.contains("23505") {
                    Err("administrator already exists".into())
                } else {
                    Err(msg)
                }
            }
        }
    }

    async fn update_administrator(&self, admin: &Administrator) -> Result<(), String> {
        let r = sqlx::query(
            r#"
            UPDATE system_administrator SET
                username = $1,
                password_hash = $2,
                must_change_password = $3,
                password_changed_at = $4,
                updated_at = $5
            WHERE singleton = TRUE
            "#,
        )
        .bind(&admin.username)
        .bind(&admin.password_hash)
        .bind(admin.must_change_password)
        .bind(admin.password_changed_at)
        .bind(admin.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            return Err("administrator missing".into());
        }
        Ok(())
    }

    async fn insert_session(&self, session: &AdminSessionRecord) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO admin_sessions (
                id, session_hash, expires_at, revoked_at,
                ip_hash, user_agent_hash, created_at
            ) VALUES ($1, $2, $3, NULL, $4, $5, $6)
            "#,
        )
        .bind(session.id)
        .bind(&session.session_hash)
        .bind(session.expires_at)
        .bind(&session.ip_hash)
        .bind(&session.user_agent_hash)
        .bind(session.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn find_session_by_hash(
        &self,
        session_hash: &str,
    ) -> Result<Option<AdminSession>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, session_hash, expires_at, revoked_at,
                   ip_hash, user_agent_hash, created_at
            FROM admin_sessions
            WHERE session_hash = $1
            "#,
        )
        .bind(session_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_session(&r)).transpose()
    }

    async fn revoke_session(&self, session_id: Uuid, at: DateTime<Utc>) -> Result<(), String> {
        let r = sqlx::query(
            r#"
            UPDATE admin_sessions
            SET revoked_at = $2
            WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(session_id)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM admin_sessions WHERE id = $1")
                    .bind(session_id)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(|e| e.to_string())?;
            if exists.is_none() {
                return Err("session not found".into());
            }
        }
        Ok(())
    }

    async fn revoke_other_sessions(
        &self,
        keep_session_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, String> {
        let r = sqlx::query(
            r#"
            UPDATE admin_sessions
            SET revoked_at = $2
            WHERE id <> $1 AND revoked_at IS NULL
            "#,
        )
        .bind(keep_session_id)
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(r.rows_affected())
    }

    async fn revoke_all_sessions(&self, at: DateTime<Utc>) -> Result<u64, String> {
        let r = sqlx::query(
            r#"
            UPDATE admin_sessions
            SET revoked_at = $1
            WHERE revoked_at IS NULL
            "#,
        )
        .bind(at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(r.rows_affected())
    }
}

#[async_trait]
impl SettingsRepository for PgAdministratorRepository {
    async fn get_schedule(&self) -> Result<ScheduleSettings, String> {
        let row = sqlx::query(
            r#"
            SELECT timezone, preflight_time, plan_start_time, working_windows,
                   image_min_side_px, image_min_area_px,
                   ocr_min_confidence::float8 AS ocr_min_confidence
            FROM system_settings
            WHERE singleton = TRUE
            "#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        let windows_json: serde_json::Value =
            row.try_get("working_windows").map_err(|e| e.to_string())?;
        let working_windows: Vec<WorkingWindow> =
            serde_json::from_value(windows_json).map_err(|e| e.to_string())?;

        Ok(ScheduleSettings {
            timezone: row.try_get("timezone").map_err(|e| e.to_string())?,
            preflight_time: row.try_get("preflight_time").map_err(|e| e.to_string())?,
            plan_start_time: row.try_get("plan_start_time").map_err(|e| e.to_string())?,
            working_windows,
            image_min_side_px: row
                .try_get("image_min_side_px")
                .map_err(|e| e.to_string())?,
            image_min_area_px: row
                .try_get("image_min_area_px")
                .map_err(|e| e.to_string())?,
            ocr_min_confidence: row
                .try_get("ocr_min_confidence")
                .map_err(|e| e.to_string())?,
        })
    }

    async fn update_schedule(&self, settings: &ScheduleSettings) -> Result<(), String> {
        let windows = serde_json::to_value(&settings.working_windows).map_err(|e| e.to_string())?;
        let r = sqlx::query(
            r#"
            UPDATE system_settings SET
                timezone = $1,
                preflight_time = $2,
                plan_start_time = $3,
                working_windows = $4,
                image_min_side_px = $5,
                image_min_area_px = $6,
                ocr_min_confidence = $7,
                updated_at = now()
            WHERE singleton = TRUE
            "#,
        )
        .bind(&settings.timezone)
        .bind(&settings.preflight_time)
        .bind(&settings.plan_start_time)
        .bind(windows)
        .bind(settings.image_min_side_px)
        .bind(settings.image_min_area_px)
        .bind(settings.ocr_min_confidence)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            return Err("system_settings missing".into());
        }
        Ok(())
    }
}

#[async_trait]
impl AuditRepository for PgAdministratorRepository {
    async fn append(&self, record: &AuditLogRecord) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO audit_logs (
                id, actor_type, actor_id, action, resource_type, resource_id,
                result, details_json, created_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(record.id)
        .bind(&record.actor_type)
        .bind(&record.actor_id)
        .bind(&record.action)
        .bind(&record.resource_type)
        .bind(&record.resource_id)
        .bind(&record.result)
        .bind(&record.details_json)
        .bind(record.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list_recent(&self, limit: i64) -> Result<Vec<AuditLogRecord>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, actor_type, actor_id, action, resource_type, resource_id,
                   result, details_json, created_at
            FROM audit_logs
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;

        rows.into_iter()
            .map(|row| {
                Ok(AuditLogRecord {
                    id: row.try_get("id").map_err(|e| e.to_string())?,
                    actor_type: row.try_get("actor_type").map_err(|e| e.to_string())?,
                    actor_id: row.try_get("actor_id").map_err(|e| e.to_string())?,
                    action: row.try_get("action").map_err(|e| e.to_string())?,
                    resource_type: row.try_get("resource_type").map_err(|e| e.to_string())?,
                    resource_id: row.try_get("resource_id").map_err(|e| e.to_string())?,
                    result: row.try_get("result").map_err(|e| e.to_string())?,
                    details_json: row.try_get("details_json").map_err(|e| e.to_string())?,
                    created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
                })
            })
            .collect()
    }
}
