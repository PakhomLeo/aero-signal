//! PostgreSQL 采集账号仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::collector::{
    CollectorAccount, CollectorAccountStatus, CollectorEgressBinding, CollectorLoginSession,
};
use crate::repository::collector::CollectorRepository;

#[derive(Clone)]
pub struct PgCollectorRepository {
    pool: PgPool,
}

impl PgCollectorRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_account(row: &PgRow) -> Result<CollectorAccount, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(CollectorAccount {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        connector: row.try_get("connector").map_err(|e| e.to_string())?,
        display_name: row.try_get("display_name").map_err(|e| e.to_string())?,
        external_account_ref: row
            .try_get("external_account_ref")
            .map_err(|e| e.to_string())?,
        status: CollectorAccountStatus::parse(&status_s)?,
        capability_json: row.try_get("capability_json").map_err(|e| e.to_string())?,
        last_probe_at: row.try_get("last_probe_at").map_err(|e| e.to_string())?,
        last_success_at: row.try_get("last_success_at").map_err(|e| e.to_string())?,
        last_failure_at: row.try_get("last_failure_at").map_err(|e| e.to_string())?,
        last_error_code: row.try_get("last_error_code").map_err(|e| e.to_string())?,
        cooldown_until: row.try_get("cooldown_until").map_err(|e| e.to_string())?,
        daily_budget_remaining: row
            .try_get("daily_budget_remaining")
            .map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_session(row: &PgRow) -> Result<CollectorLoginSession, String> {
    Ok(CollectorLoginSession {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        collector_account_id: row
            .try_get("collector_account_id")
            .map_err(|e| e.to_string())?,
        credential_secret_ref: row
            .try_get("credential_secret_ref")
            .map_err(|e| e.to_string())?,
        browser_profile_ref: row
            .try_get("browser_profile_ref")
            .map_err(|e| e.to_string())?,
        authorized_mp_ids: serde_json::from_value(
            row.try_get("authorized_mp_ids")
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?,
        expires_at: row.try_get("expires_at").map_err(|e| e.to_string())?,
        confirmed_at: row.try_get("confirmed_at").map_err(|e| e.to_string())?,
        revoked_at: row.try_get("revoked_at").map_err(|e| e.to_string())?,
    })
}

fn map_binding(row: &PgRow) -> Result<CollectorEgressBinding, String> {
    Ok(CollectorEgressBinding {
        collector_account_id: row
            .try_get("collector_account_id")
            .map_err(|e| e.to_string())?,
        fixed_egress_id: row.try_get("fixed_egress_id").map_err(|e| e.to_string())?,
        bound_at: row.try_get("bound_at").map_err(|e| e.to_string())?,
        changed_at: row.try_get("changed_at").map_err(|e| e.to_string())?,
        requires_relogin: row.try_get("requires_relogin").map_err(|e| e.to_string())?,
    })
}

const ACCOUNT_COLS: &str = r#"
    id, connector, display_name, external_account_ref, status, capability_json,
    last_probe_at, last_success_at, last_failure_at, last_error_code,
    cooldown_until, daily_budget_remaining,
    created_at, updated_at
"#;

const SESSION_COLS: &str = r#"
    id, collector_account_id, credential_secret_ref, browser_profile_ref,
    authorized_mp_ids, expires_at, confirmed_at, revoked_at
"#;

const BINDING_COLS: &str = r#"
    collector_account_id, fixed_egress_id, bound_at, changed_at, requires_relogin
"#;

async fn upsert_account_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &CollectorAccount,
) -> Result<(), String> {
    sqlx::query(
        r#"
        INSERT INTO collector_accounts (
            id, connector, display_name, external_account_ref, status, capability_json,
            last_probe_at, last_success_at, last_failure_at, last_error_code,
            cooldown_until, daily_budget_remaining, created_at, updated_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
        ON CONFLICT (id) DO UPDATE SET
            connector = EXCLUDED.connector,
            display_name = EXCLUDED.display_name,
            external_account_ref = EXCLUDED.external_account_ref,
            status = EXCLUDED.status,
            capability_json = EXCLUDED.capability_json,
            last_probe_at = EXCLUDED.last_probe_at,
            last_success_at = EXCLUDED.last_success_at,
            last_failure_at = EXCLUDED.last_failure_at,
            last_error_code = EXCLUDED.last_error_code,
            cooldown_until = EXCLUDED.cooldown_until,
            daily_budget_remaining = EXCLUDED.daily_budget_remaining,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(account.id)
    .bind(&account.connector)
    .bind(&account.display_name)
    .bind(&account.external_account_ref)
    .bind(account.status.as_str())
    .bind(&account.capability_json)
    .bind(account.last_probe_at)
    .bind(account.last_success_at)
    .bind(account.last_failure_at)
    .bind(&account.last_error_code)
    .bind(account.cooldown_until)
    .bind(account.daily_budget_remaining)
    .bind(account.created_at)
    .bind(account.updated_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn insert_session_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    session: &CollectorLoginSession,
) -> Result<(), String> {
    sqlx::query(
        r#"
        INSERT INTO collector_login_sessions (
            id, collector_account_id, credential_secret_ref, browser_profile_ref,
            authorized_mp_ids, expires_at, confirmed_at, revoked_at
        ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
        "#,
    )
    .bind(session.id)
    .bind(session.collector_account_id)
    .bind(&session.credential_secret_ref)
    .bind(&session.browser_profile_ref)
    .bind(serde_json::to_value(&session.authorized_mp_ids).map_err(|e| e.to_string())?)
    .bind(session.expires_at)
    .bind(session.confirmed_at)
    .bind(session.revoked_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn revoke_sessions_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    collector_account_id: Uuid,
    revoked_at: DateTime<Utc>,
) -> Result<(), String> {
    sqlx::query(
        r#"
        UPDATE collector_login_sessions
        SET revoked_at = $2
        WHERE collector_account_id = $1 AND revoked_at IS NULL
        "#,
    )
    .bind(collector_account_id)
    .bind(revoked_at)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

async fn upsert_binding_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    binding: &CollectorEgressBinding,
) -> Result<(), String> {
    sqlx::query(
        r#"
        INSERT INTO collector_egress_bindings (
            collector_account_id, fixed_egress_id, bound_at, changed_at, requires_relogin
        ) VALUES ($1,$2,$3,$4,$5)
        ON CONFLICT (collector_account_id) DO UPDATE SET
            fixed_egress_id = EXCLUDED.fixed_egress_id,
            bound_at = EXCLUDED.bound_at,
            changed_at = EXCLUDED.changed_at,
            requires_relogin = EXCLUDED.requires_relogin
        "#,
    )
    .bind(binding.collector_account_id)
    .bind(binding.fixed_egress_id)
    .bind(binding.bound_at)
    .bind(binding.changed_at)
    .bind(binding.requires_relogin)
    .execute(&mut **tx)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[async_trait]
impl CollectorRepository for PgCollectorRepository {
    async fn list_accounts(&self) -> Result<Vec<CollectorAccount>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLS} FROM collector_accounts ORDER BY created_at ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_account).collect()
    }

    async fn get_account(&self, id: Uuid) -> Result<Option<CollectorAccount>, String> {
        let row = sqlx::query(&format!(
            "SELECT {ACCOUNT_COLS} FROM collector_accounts WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_account(&r)).transpose()
    }

    async fn insert_account(&self, account: &CollectorAccount) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO collector_accounts (
                id, connector, display_name, external_account_ref, status, capability_json,
                last_probe_at, last_success_at, last_failure_at, last_error_code,
                cooldown_until, daily_budget_remaining, created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)
            "#,
        )
        .bind(account.id)
        .bind(&account.connector)
        .bind(&account.display_name)
        .bind(&account.external_account_ref)
        .bind(account.status.as_str())
        .bind(&account.capability_json)
        .bind(account.last_probe_at)
        .bind(account.last_success_at)
        .bind(account.last_failure_at)
        .bind(&account.last_error_code)
        .bind(account.cooldown_until)
        .bind(account.daily_budget_remaining)
        .bind(account.created_at)
        .bind(account.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn update_account(&self, account: &CollectorAccount) -> Result<(), String> {
        let r = sqlx::query(
            r#"
            UPDATE collector_accounts SET
                connector = $2,
                display_name = $3,
                external_account_ref = $4,
                status = $5,
                capability_json = $6,
                last_probe_at = $7,
                last_success_at = $8,
                last_failure_at = $9,
                last_error_code = $10,
                cooldown_until = $11,
                daily_budget_remaining = $12,
                updated_at = $13
            WHERE id = $1
            "#,
        )
        .bind(account.id)
        .bind(&account.connector)
        .bind(&account.display_name)
        .bind(&account.external_account_ref)
        .bind(account.status.as_str())
        .bind(&account.capability_json)
        .bind(account.last_probe_at)
        .bind(account.last_success_at)
        .bind(account.last_failure_at)
        .bind(&account.last_error_code)
        .bind(account.cooldown_until)
        .bind(account.daily_budget_remaining)
        .bind(account.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            return Err("采集账号不存在".into());
        }
        Ok(())
    }

    async fn get_active_login_session(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorLoginSession>, String> {
        let row = sqlx::query(&format!(
            r#"
            SELECT {SESSION_COLS}
            FROM collector_login_sessions
            WHERE collector_account_id = $1 AND revoked_at IS NULL
            ORDER BY confirmed_at DESC
            LIMIT 1
            "#
        ))
        .bind(collector_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_session(&r)).transpose()
    }

    async fn insert_login_session(&self, session: &CollectorLoginSession) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO collector_login_sessions (
                id, collector_account_id, credential_secret_ref, browser_profile_ref,
                authorized_mp_ids, expires_at, confirmed_at, revoked_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(session.id)
        .bind(session.collector_account_id)
        .bind(&session.credential_secret_ref)
        .bind(&session.browser_profile_ref)
        .bind(serde_json::to_value(&session.authorized_mp_ids).map_err(|e| e.to_string())?)
        .bind(session.expires_at)
        .bind(session.confirmed_at)
        .bind(session.revoked_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn revoke_login_sessions(
        &self,
        collector_account_id: Uuid,
        revoked_at: DateTime<Utc>,
    ) -> Result<(), String> {
        sqlx::query(
            r#"
            UPDATE collector_login_sessions
            SET revoked_at = $2
            WHERE collector_account_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(collector_account_id)
        .bind(revoked_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn get_egress_binding(
        &self,
        collector_account_id: Uuid,
    ) -> Result<Option<CollectorEgressBinding>, String> {
        let row = sqlx::query(&format!(
            "SELECT {BINDING_COLS} FROM collector_egress_bindings WHERE collector_account_id = $1"
        ))
        .bind(collector_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_binding(&r)).transpose()
    }

    async fn upsert_egress_binding(&self, binding: &CollectorEgressBinding) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO collector_egress_bindings (
                collector_account_id, fixed_egress_id, bound_at, changed_at, requires_relogin
            ) VALUES ($1,$2,$3,$4,$5)
            ON CONFLICT (collector_account_id) DO UPDATE SET
                fixed_egress_id = EXCLUDED.fixed_egress_id,
                bound_at = EXCLUDED.bound_at,
                changed_at = EXCLUDED.changed_at,
                requires_relogin = EXCLUDED.requires_relogin
            "#,
        )
        .bind(binding.collector_account_id)
        .bind(binding.fixed_egress_id)
        .bind(binding.bound_at)
        .bind(binding.changed_at)
        .bind(binding.requires_relogin)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn persist_login_success(
        &self,
        account: &CollectorAccount,
        session: &CollectorLoginSession,
        binding: &CollectorEgressBinding,
        revoke_previous: bool,
    ) -> Result<(), String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        upsert_account_tx(&mut tx, account).await?;
        if revoke_previous {
            revoke_sessions_tx(&mut tx, account.id, Utc::now()).await?;
        }
        insert_session_tx(&mut tx, session).await?;
        upsert_binding_tx(&mut tx, binding).await?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(())
    }
}
