//! PostgreSQL 出口与供应商仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::egress::{
    EgressKind, EgressProfile, EgressPurpose, EgressStatus, ProxyProvider, ProxyProviderMode,
};
use crate::repository::egress::EgressRepository;

#[derive(Clone)]
pub struct PgEgressRepository {
    pool: PgPool,
}

impl PgEgressRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_provider(row: &PgRow) -> Result<ProxyProvider, String> {
    let mode_s: String = row.try_get("mode").map_err(|e| e.to_string())?;
    Ok(ProxyProvider {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        name: row.try_get("name").map_err(|e| e.to_string())?,
        mode: ProxyProviderMode::parse(&mode_s)?,
        api_base_url: row.try_get("api_base_url").map_err(|e| e.to_string())?,
        auth_secret_ref: row.try_get("auth_secret_ref").map_err(|e| e.to_string())?,
        request_template_json: row
            .try_get("request_template_json")
            .map_err(|e| e.to_string())?,
        response_mapping_json: row
            .try_get("response_mapping_json")
            .map_err(|e| e.to_string())?,
        enabled: row.try_get("enabled").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_profile(row: &PgRow) -> Result<EgressProfile, String> {
    let kind_s: String = row.try_get("kind").map_err(|e| e.to_string())?;
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    let purposes_raw: Vec<String> = row.try_get("allowed_purposes").map_err(|e| e.to_string())?;
    let mut allowed_purposes = Vec::with_capacity(purposes_raw.len());
    for s in purposes_raw {
        allowed_purposes.push(EgressPurpose::parse(&s)?);
    }
    Ok(EgressProfile {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        name: row.try_get("name").map_err(|e| e.to_string())?,
        kind: EgressKind::parse(&kind_s)?,
        provider_id: row.try_get("provider_id").map_err(|e| e.to_string())?,
        endpoint_secret_ref: row
            .try_get("endpoint_secret_ref")
            .map_err(|e| e.to_string())?,
        priority: row.try_get("priority").map_err(|e| e.to_string())?,
        allowed_purposes,
        status: EgressStatus::parse(&status_s)?,
        last_preflight_status: row
            .try_get("last_preflight_status")
            .map_err(|e| e.to_string())?,
        last_preflight_at: row
            .try_get("last_preflight_at")
            .map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn purpose_strings(p: &[EgressPurpose]) -> Vec<String> {
    p.iter().map(|x| x.as_str().to_string()).collect()
}

#[async_trait]
impl EgressRepository for PgEgressRepository {
    async fn list_proxy_providers(&self) -> Result<Vec<ProxyProvider>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, mode, api_base_url, auth_secret_ref,
                   request_template_json, response_mapping_json,
                   enabled, created_at, updated_at
            FROM proxy_providers
            ORDER BY created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_provider).collect()
    }

    async fn get_proxy_provider(&self, id: Uuid) -> Result<Option<ProxyProvider>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, name, mode, api_base_url, auth_secret_ref,
                   request_template_json, response_mapping_json,
                   enabled, created_at, updated_at
            FROM proxy_providers WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_provider(&r)).transpose()
    }

    async fn insert_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO proxy_providers (
                id, name, mode, api_base_url, auth_secret_ref,
                request_template_json, response_mapping_json,
                enabled, created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)
            "#,
        )
        .bind(p.id)
        .bind(&p.name)
        .bind(p.mode.as_str())
        .bind(&p.api_base_url)
        .bind(&p.auth_secret_ref)
        .bind(&p.request_template_json)
        .bind(&p.response_mapping_json)
        .bind(p.enabled)
        .bind(p.created_at)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn update_proxy_provider(&self, p: &ProxyProvider) -> Result<(), String> {
        let r = sqlx::query(
            r#"
            UPDATE proxy_providers SET
                name = $2, mode = $3, api_base_url = $4, auth_secret_ref = $5,
                request_template_json = $6, response_mapping_json = $7,
                enabled = $8, updated_at = $9
            WHERE id = $1
            "#,
        )
        .bind(p.id)
        .bind(&p.name)
        .bind(p.mode.as_str())
        .bind(&p.api_base_url)
        .bind(&p.auth_secret_ref)
        .bind(&p.request_template_json)
        .bind(&p.response_mapping_json)
        .bind(p.enabled)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            return Err("proxy_provider not found".into());
        }
        Ok(())
    }

    async fn list_egress_profiles(&self) -> Result<Vec<EgressProfile>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, name, kind, provider_id, endpoint_secret_ref, priority,
                   allowed_purposes, status, last_preflight_status, last_preflight_at,
                   created_at, updated_at
            FROM egress_profiles
            ORDER BY priority ASC, created_at ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_profile).collect()
    }

    async fn get_egress_profile(&self, id: Uuid) -> Result<Option<EgressProfile>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, name, kind, provider_id, endpoint_secret_ref, priority,
                   allowed_purposes, status, last_preflight_status, last_preflight_at,
                   created_at, updated_at
            FROM egress_profiles WHERE id = $1
            "#,
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_profile(&r)).transpose()
    }

    async fn get_egress_profile_by_name(
        &self,
        name: &str,
    ) -> Result<Option<EgressProfile>, String> {
        let row = sqlx::query(
            r#"
            SELECT id, name, kind, provider_id, endpoint_secret_ref, priority,
                   allowed_purposes, status, last_preflight_status, last_preflight_at,
                   created_at, updated_at
            FROM egress_profiles WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_profile(&r)).transpose()
    }

    async fn insert_egress_profile(&self, p: &EgressProfile) -> Result<(), String> {
        let purposes = purpose_strings(&p.allowed_purposes);
        sqlx::query(
            r#"
            INSERT INTO egress_profiles (
                id, name, kind, provider_id, endpoint_secret_ref, priority,
                allowed_purposes, status, last_preflight_status, last_preflight_at,
                created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
            "#,
        )
        .bind(p.id)
        .bind(&p.name)
        .bind(p.kind.as_str())
        .bind(p.provider_id)
        .bind(&p.endpoint_secret_ref)
        .bind(p.priority)
        .bind(&purposes)
        .bind(p.status.as_str())
        .bind(&p.last_preflight_status)
        .bind(p.last_preflight_at)
        .bind(p.created_at)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn update_egress_profile(&self, p: &EgressProfile) -> Result<(), String> {
        let purposes = purpose_strings(&p.allowed_purposes);
        let r = sqlx::query(
            r#"
            UPDATE egress_profiles SET
                name = $2, kind = $3, provider_id = $4, endpoint_secret_ref = $5,
                priority = $6, allowed_purposes = $7, status = $8,
                last_preflight_status = $9, last_preflight_at = $10, updated_at = $11
            WHERE id = $1
            "#,
        )
        .bind(p.id)
        .bind(&p.name)
        .bind(p.kind.as_str())
        .bind(p.provider_id)
        .bind(&p.endpoint_secret_ref)
        .bind(p.priority)
        .bind(&purposes)
        .bind(p.status.as_str())
        .bind(&p.last_preflight_status)
        .bind(p.last_preflight_at)
        .bind(p.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if r.rows_affected() == 0 {
            return Err("egress_profile not found".into());
        }
        let _: DateTime<Utc> = p.updated_at;
        Ok(())
    }
}
