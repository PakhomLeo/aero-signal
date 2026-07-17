//! PostgreSQL 分组 / Token / 调用日志仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::delivery::{
    ApiCallLog, ApiScope, ApiToken, DeliveryGroup, DeliveryGroupSource, DeliveryGroupStatus,
};
use crate::repository::delivery::{
    CreateDeliveryGroup, CreateDeliveryGroupWithSources, DeliveryRepository, PatchDeliveryGroup,
    PatchDeliveryGroupWithSources,
};

#[derive(Clone)]
pub struct PgDeliveryRepository {
    pool: PgPool,
}

impl PgDeliveryRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_group(row: &PgRow) -> Result<DeliveryGroup, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(DeliveryGroup {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        name: row.try_get("name").map_err(|e| e.to_string())?,
        description: row.try_get("description").map_err(|e| e.to_string())?,
        status: DeliveryGroupStatus::parse(&status_s)?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_group_source(row: &PgRow) -> Result<DeliveryGroupSource, String> {
    Ok(DeliveryGroupSource {
        delivery_group_id: row
            .try_get("delivery_group_id")
            .map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        bound_at: row.try_get("bound_at").map_err(|e| e.to_string())?,
    })
}

fn map_token(row: &PgRow) -> Result<ApiToken, String> {
    let scopes_raw: Vec<String> = row.try_get("scopes").map_err(|e| e.to_string())?;
    let mut scopes = Vec::with_capacity(scopes_raw.len());
    for s in scopes_raw {
        scopes.push(ApiScope::parse(&s)?);
    }
    Ok(ApiToken {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        delivery_group_id: row
            .try_get("delivery_group_id")
            .map_err(|e| e.to_string())?,
        name: row.try_get("name").map_err(|e| e.to_string())?,
        token_prefix: row.try_get("token_prefix").map_err(|e| e.to_string())?,
        token_hash: row.try_get("token_hash").map_err(|e| e.to_string())?,
        scopes,
        ip_allowlist: row.try_get("ip_allowlist").map_err(|e| e.to_string())?,
        expires_at: row.try_get("expires_at").map_err(|e| e.to_string())?,
        revoked_at: row.try_get("revoked_at").map_err(|e| e.to_string())?,
        last_used_at: row.try_get("last_used_at").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn map_call_log(row: &PgRow) -> Result<ApiCallLog, String> {
    Ok(ApiCallLog {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        api_token_id: row.try_get("api_token_id").map_err(|e| e.to_string())?,
        delivery_group_id: row
            .try_get("delivery_group_id")
            .map_err(|e| e.to_string())?,
        method: row.try_get("method").map_err(|e| e.to_string())?,
        path: row.try_get("path").map_err(|e| e.to_string())?,
        scope_used: row.try_get("scope_used").map_err(|e| e.to_string())?,
        status_code: row.try_get("status_code").map_err(|e| e.to_string())?,
        duration_ms: row.try_get("duration_ms").map_err(|e| e.to_string())?,
        redaction_count: row.try_get("redaction_count").map_err(|e| e.to_string())?,
        client_ip_hash: row.try_get("client_ip_hash").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

fn scope_strings(scopes: &[ApiScope]) -> Vec<String> {
    scopes.iter().map(|s| s.as_str().to_string()).collect()
}

const GROUP_COLS: &str = "id, name, description, status, created_at, updated_at";
const TOKEN_COLS: &str = r#"
    id, delivery_group_id, name, token_prefix, token_hash, scopes, ip_allowlist,
    expires_at, revoked_at, last_used_at, created_at
"#;

#[async_trait]
impl DeliveryRepository for PgDeliveryRepository {
    async fn create_group(&self, cmd: CreateDeliveryGroup) -> Result<DeliveryGroup, String> {
        let conflict: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM delivery_groups WHERE name = $1)"#)
                .bind(&cmd.name)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        if conflict {
            return Err("delivery.group_name_conflict: 分组名称已存在".into());
        }

        let row = sqlx::query(&format!(
            "INSERT INTO delivery_groups (id, name, description, status, created_at, updated_at)
             VALUES ($1, $2, $3, 'active', $4, $5)
             RETURNING {GROUP_COLS}"
        ))
        .bind(cmd.id)
        .bind(&cmd.name)
        .bind(&cmd.description)
        .bind(cmd.now)
        .bind(cmd.now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        map_group(&row)
    }

    async fn create_group_with_sources(
        &self,
        cmd: CreateDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let conflict: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM delivery_groups WHERE name = $1)"#)
                .bind(&cmd.group.name)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        if conflict {
            return Err("delivery.group_name_conflict: group name already exists".into());
        }

        let row = sqlx::query(&format!(
            "INSERT INTO delivery_groups (id, name, description, status, created_at, updated_at)
             VALUES ($1, $2, $3, 'active', $4, $5)
             RETURNING {GROUP_COLS}"
        ))
        .bind(cmd.group.id)
        .bind(&cmd.group.name)
        .bind(&cmd.group.description)
        .bind(cmd.group.now)
        .bind(cmd.group.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let mut seen = std::collections::HashSet::new();
        for source_id in cmd.source_ids {
            if !seen.insert(source_id) {
                continue;
            }
            sqlx::query(
                r#"
                INSERT INTO delivery_group_sources (
                    delivery_group_id, source_account_id, bound_at
                ) VALUES ($1, $2, $3)
                "#,
            )
            .bind(cmd.group.id)
            .bind(source_id)
            .bind(cmd.group.now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        let group = map_group(&row)?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(group)
    }

    async fn get_group(&self, id: Uuid) -> Result<Option<DeliveryGroup>, String> {
        let row = sqlx::query(&format!(
            "SELECT {GROUP_COLS} FROM delivery_groups WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_group(&r)).transpose()
    }

    async fn list_groups(&self) -> Result<Vec<DeliveryGroup>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {GROUP_COLS} FROM delivery_groups ORDER BY created_at ASC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_group).collect()
    }

    async fn patch_group(
        &self,
        id: Uuid,
        cmd: PatchDeliveryGroup,
    ) -> Result<DeliveryGroup, String> {
        let existing = self
            .get_group(id)
            .await?
            .ok_or_else(|| "delivery.group_not_found: 分组不存在".to_string())?;

        if let Some(ref name) = cmd.name {
            let conflict: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM delivery_groups WHERE name = $1 AND id <> $2
                )
                "#,
            )
            .bind(name)
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
            if conflict {
                return Err("delivery.group_name_conflict: 分组名称已存在".into());
            }
        }

        let name = cmd.name.unwrap_or(existing.name);
        let description = match cmd.description {
            Some(d) => d,
            None => existing.description,
        };
        let status = cmd.status.unwrap_or(existing.status);

        let row = sqlx::query(&format!(
            "UPDATE delivery_groups SET
                name = $2, description = $3, status = $4, updated_at = $5
             WHERE id = $1
             RETURNING {GROUP_COLS}"
        ))
        .bind(id)
        .bind(&name)
        .bind(&description)
        .bind(status.as_str())
        .bind(cmd.now)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        map_group(&row)
    }

    async fn patch_group_with_sources(
        &self,
        cmd: PatchDeliveryGroupWithSources,
    ) -> Result<DeliveryGroup, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let existing_row = sqlx::query(&format!(
            "SELECT {GROUP_COLS} FROM delivery_groups WHERE id = $1"
        ))
        .bind(cmd.group_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let existing = existing_row
            .as_ref()
            .map(map_group)
            .transpose()?
            .ok_or_else(|| "delivery.group_not_found: group not found".to_string())?;

        if let Some(ref name) = cmd.group.name {
            let conflict: bool = sqlx::query_scalar(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM delivery_groups WHERE name = $1 AND id <> $2
                )
                "#,
            )
            .bind(name)
            .bind(cmd.group_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            if conflict {
                return Err("delivery.group_name_conflict: group name already exists".into());
            }
        }

        let name = cmd.group.name.unwrap_or(existing.name);
        let description = match cmd.group.description {
            Some(description) => description,
            None => existing.description,
        };
        let status = cmd.group.status.unwrap_or(existing.status);
        let row = sqlx::query(&format!(
            "UPDATE delivery_groups SET
                name = $2, description = $3, status = $4, updated_at = $5
             WHERE id = $1
             RETURNING {GROUP_COLS}"
        ))
        .bind(cmd.group_id)
        .bind(&name)
        .bind(&description)
        .bind(status.as_str())
        .bind(cmd.group.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        if let Some(source_ids) = cmd.source_ids {
            sqlx::query(r#"DELETE FROM delivery_group_sources WHERE delivery_group_id = $1"#)
                .bind(cmd.group_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;

            let mut seen = std::collections::HashSet::new();
            for source_id in source_ids {
                if !seen.insert(source_id) {
                    continue;
                }
                sqlx::query(
                    r#"
                    INSERT INTO delivery_group_sources (
                        delivery_group_id, source_account_id, bound_at
                    ) VALUES ($1, $2, $3)
                    "#,
                )
                .bind(cmd.group_id)
                .bind(source_id)
                .bind(cmd.group.now)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            }
        }

        let group = map_group(&row)?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(group)
    }

    async fn bind_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String> {
        let exists: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM delivery_groups WHERE id = $1)"#)
                .bind(group_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        if !exists {
            return Err("delivery.group_not_found: 分组不存在".into());
        }

        for sid in source_ids {
            sqlx::query(
                r#"
                INSERT INTO delivery_group_sources (
                    delivery_group_id, source_account_id, bound_at
                ) VALUES ($1, $2, $3)
                ON CONFLICT (delivery_group_id, source_account_id) DO NOTHING
                "#,
            )
            .bind(group_id)
            .bind(sid)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        }

        self.list_group_sources(group_id).await
    }

    async fn set_group_sources(
        &self,
        group_id: Uuid,
        source_ids: &[Uuid],
        now: DateTime<Utc>,
    ) -> Result<Vec<DeliveryGroupSource>, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        let exists: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM delivery_groups WHERE id = $1)"#)
                .bind(group_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        if !exists {
            return Err("delivery.group_not_found: 分组不存在".into());
        }

        sqlx::query(r#"DELETE FROM delivery_group_sources WHERE delivery_group_id = $1"#)
            .bind(group_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;

        let mut seen = std::collections::HashSet::new();
        for sid in source_ids {
            if !seen.insert(*sid) {
                continue;
            }
            sqlx::query(
                r#"
                INSERT INTO delivery_group_sources (
                    delivery_group_id, source_account_id, bound_at
                ) VALUES ($1, $2, $3)
                "#,
            )
            .bind(group_id)
            .bind(sid)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
        }

        let rows = sqlx::query(
            r#"
            SELECT delivery_group_id, source_account_id, bound_at
            FROM delivery_group_sources
            WHERE delivery_group_id = $1
            ORDER BY bound_at ASC
            "#,
        )
        .bind(group_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        tx.commit().await.map_err(|e| e.to_string())?;
        rows.iter().map(map_group_source).collect()
    }

    async fn list_group_sources(&self, group_id: Uuid) -> Result<Vec<DeliveryGroupSource>, String> {
        let rows = sqlx::query(
            r#"
            SELECT delivery_group_id, source_account_id, bound_at
            FROM delivery_group_sources
            WHERE delivery_group_id = $1
            ORDER BY bound_at ASC
            "#,
        )
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_group_source).collect()
    }

    async fn insert_token(&self, token: ApiToken) -> Result<ApiToken, String> {
        let exists: bool =
            sqlx::query_scalar(r#"SELECT EXISTS(SELECT 1 FROM delivery_groups WHERE id = $1)"#)
                .bind(token.delivery_group_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| e.to_string())?;
        if !exists {
            return Err("delivery.group_not_found: 分组不存在".into());
        }

        let scopes = scope_strings(&token.scopes);
        let result = sqlx::query(
            r#"
            INSERT INTO api_tokens (
                id, delivery_group_id, name, token_prefix, token_hash, scopes,
                ip_allowlist, expires_at, revoked_at, last_used_at, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(token.id)
        .bind(token.delivery_group_id)
        .bind(&token.name)
        .bind(&token.token_prefix)
        .bind(&token.token_hash)
        .bind(&scopes)
        .bind(&token.ip_allowlist)
        .bind(token.expires_at)
        .bind(token.revoked_at)
        .bind(token.last_used_at)
        .bind(token.created_at)
        .execute(&self.pool)
        .await;

        match result {
            Ok(_) => Ok(token),
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("unique") || msg.contains("duplicate") || msg.contains("23505") {
                    Err("token.hash_conflict: token hash 冲突".into())
                } else {
                    Err(msg)
                }
            }
        }
    }

    async fn get_token(&self, id: Uuid) -> Result<Option<ApiToken>, String> {
        let row = sqlx::query(&format!(
            "SELECT {TOKEN_COLS} FROM api_tokens WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_token(&r)).transpose()
    }

    async fn find_token_by_hash(&self, token_hash: &str) -> Result<Option<ApiToken>, String> {
        let row = sqlx::query(&format!(
            "SELECT {TOKEN_COLS} FROM api_tokens WHERE token_hash = $1"
        ))
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_token(&r)).transpose()
    }

    async fn list_tokens_for_group(&self, group_id: Uuid) -> Result<Vec<ApiToken>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {TOKEN_COLS} FROM api_tokens
             WHERE delivery_group_id = $1
             ORDER BY created_at DESC"
        ))
        .bind(group_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_token).collect()
    }

    async fn revoke_token(&self, id: Uuid, now: DateTime<Utc>) -> Result<ApiToken, String> {
        let row = sqlx::query(&format!(
            "UPDATE api_tokens SET
                revoked_at = COALESCE(revoked_at, $2)
             WHERE id = $1
             RETURNING {TOKEN_COLS}"
        ))
        .bind(id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "token.not_found: Token 不存在".to_string())?;
        map_token(&row)
    }

    async fn touch_token_used(&self, id: Uuid, now: DateTime<Utc>) -> Result<(), String> {
        sqlx::query(r#"UPDATE api_tokens SET last_used_at = $2 WHERE id = $1"#)
            .bind(id)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn insert_call_log(&self, log: ApiCallLog) -> Result<ApiCallLog, String> {
        sqlx::query(
            r#"
            INSERT INTO api_call_logs (
                id, api_token_id, delivery_group_id, method, path, scope_used,
                status_code, duration_ms, redaction_count, client_ip_hash, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(log.id)
        .bind(log.api_token_id)
        .bind(log.delivery_group_id)
        .bind(&log.method)
        .bind(&log.path)
        .bind(&log.scope_used)
        .bind(log.status_code)
        .bind(log.duration_ms)
        .bind(log.redaction_count)
        .bind(&log.client_ip_hash)
        .bind(log.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(log)
    }

    async fn list_call_logs(
        &self,
        token_id: Uuid,
        limit: usize,
    ) -> Result<Vec<ApiCallLog>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = sqlx::query(
            r#"
            SELECT id, api_token_id, delivery_group_id, method, path, scope_used,
                   status_code, duration_ms, redaction_count, client_ip_hash, created_at
            FROM api_call_logs
            WHERE api_token_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#,
        )
        .bind(token_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_call_log).collect()
    }
}
