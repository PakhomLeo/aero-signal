//! PostgreSQL 保留策略仓储（M15）。
//!
//! `list_policies` / `is_asset_referenced` / `is_outbox_undelivered` 已接真实 SQL；
//! `list_expired_candidates` / `hard_delete` / `upsert_policy` 按表结构实现扫描与删除。
//!
//! ## `failed_validation_asset` 首期空转（诚实收口）
//!
//! 迁移种子里有该 `target_kind`，但**无独立落库表**。本仓储对
//! `RetentionTargetKind::FailedValidationAsset`：
//! - `list_expired_candidates` → 恒返回空集
//! - `hard_delete` → 恒返回 `false`（不执行 DELETE）
//!
//! 另见迁移 `20260710000001_disable_failed_validation_asset.sql`：将种子策略
//! `enabled=false`，避免清理任务反复扫到「启用但无表」的策略。待验证失败资产
//! 有独立表后再接线扫描/删除并重新启用。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::retention::{RetentionCandidate, RetentionPolicy, RetentionTargetKind};
use crate::repository::retention::RetentionRepository;

#[derive(Clone)]
pub struct PgRetentionRepository {
    pool: PgPool,
}

impl PgRetentionRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_policy(row: &PgRow) -> Result<RetentionPolicy, String> {
    let kind_s: String = row.try_get("target_kind").map_err(|e| e.to_string())?;
    let target_kind = RetentionTargetKind::parse(&kind_s)
        .ok_or_else(|| format!("unknown retention target_kind: {kind_s}"))?;
    Ok(RetentionPolicy {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        target_kind,
        retention_days: row.try_get("retention_days").map_err(|e| e.to_string())?,
        hard_delete_after_days: row
            .try_get("hard_delete_after_days")
            .map_err(|e| e.to_string())?,
        enabled: row.try_get("enabled").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_candidate(kind: RetentionTargetKind, row: &PgRow) -> Result<RetentionCandidate, String> {
    let article_id: Option<Uuid> = row.try_get("article_id").map_err(|e| e.to_string())?;
    Ok(RetentionCandidate {
        target_kind: kind,
        record_id: row.try_get("id").map_err(|e| e.to_string())?,
        article_id,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

#[async_trait]
impl RetentionRepository for PgRetentionRepository {
    async fn list_policies(&self) -> Result<Vec<RetentionPolicy>, String> {
        let rows = sqlx::query(
            r#"
            SELECT id, target_kind, retention_days, hard_delete_after_days, enabled, updated_at
            FROM retention_policies
            ORDER BY target_kind ASC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_policy).collect()
    }

    async fn upsert_policy(&self, policy: &RetentionPolicy) -> Result<(), String> {
        sqlx::query(
            r#"
            INSERT INTO retention_policies (
                id, target_kind, retention_days, hard_delete_after_days, enabled, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (target_kind) DO UPDATE SET
                retention_days = EXCLUDED.retention_days,
                hard_delete_after_days = EXCLUDED.hard_delete_after_days,
                enabled = EXCLUDED.enabled,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(policy.id)
        .bind(policy.target_kind.as_str())
        .bind(policy.retention_days)
        .bind(policy.hard_delete_after_days)
        .bind(policy.enabled)
        .bind(policy.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn list_expired_candidates(
        &self,
        kind: RetentionTargetKind,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RetentionCandidate>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = match kind {
            RetentionTargetKind::RawHtml
            | RetentionTargetKind::Screenshot
            | RetentionTargetKind::BodyImage => {
                let asset_kind = kind.as_str();
                sqlx::query(
                    r#"
                    SELECT id, article_id, created_at
                    FROM article_assets
                    WHERE asset_kind = $1
                      AND created_at < $2
                    ORDER BY created_at ASC
                    LIMIT $3
                    "#,
                )
                .bind(asset_kind)
                .bind(older_than)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| e.to_string())?
            }
            RetentionTargetKind::OcrText => {
                // OCR 文本挂在 article_ocr_blocks；按 block 行过期扫描。
                sqlx::query(
                    r#"
                    SELECT id, article_id, created_at
                    FROM article_ocr_blocks
                    WHERE created_at < $1
                    ORDER BY created_at ASC
                    LIMIT $2
                    "#,
                )
                .bind(older_than)
                .bind(limit)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| e.to_string())?
            }
            RetentionTargetKind::FailedValidationAsset => {
                // 首期空转：无独立 failed_validation_asset 表，返回空集避免误删。
                // 策略种子已由 20260710000001 置 enabled=false；有表后再接线。
                Vec::new()
            }
            RetentionTargetKind::ApiCallLog => sqlx::query(
                r#"
                SELECT id, NULL::uuid AS article_id, created_at
                FROM api_call_logs
                WHERE created_at < $1
                ORDER BY created_at ASC
                LIMIT $2
                "#,
            )
            .bind(older_than)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?,
            RetentionTargetKind::AuditLog => sqlx::query(
                r#"
                SELECT id, NULL::uuid AS article_id, created_at
                FROM audit_logs
                WHERE created_at < $1
                ORDER BY created_at ASC
                LIMIT $2
                "#,
            )
            .bind(older_than)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?,
            RetentionTargetKind::ChangeEvent => sqlx::query(
                r#"
                SELECT id, article_id, created_at
                FROM article_change_events
                WHERE created_at < $1
                ORDER BY created_at ASC
                LIMIT $2
                "#,
            )
            .bind(older_than)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?,
            RetentionTargetKind::DeliveryOutbox => sqlx::query(
                r#"
                SELECT id, article_id, created_at
                FROM delivery_outbox
                WHERE created_at < $1
                ORDER BY created_at ASC
                LIMIT $2
                "#,
            )
            .bind(older_than)
            .bind(limit)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| e.to_string())?,
        };

        rows.iter().map(|r| map_candidate(kind, r)).collect()
    }

    async fn hard_delete(&self, kind: RetentionTargetKind, id: Uuid) -> Result<bool, String> {
        let result = match kind {
            RetentionTargetKind::RawHtml
            | RetentionTargetKind::Screenshot
            | RetentionTargetKind::BodyImage => {
                sqlx::query("DELETE FROM article_assets WHERE id = $1 AND asset_kind = $2")
                    .bind(id)
                    .bind(kind.as_str())
                    .execute(&self.pool)
                    .await
            }
            RetentionTargetKind::OcrText => {
                sqlx::query("DELETE FROM article_ocr_blocks WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
            }
            RetentionTargetKind::FailedValidationAsset => {
                // 首期空转：无独立表，不执行删除。
                return Ok(false);
            }
            RetentionTargetKind::ApiCallLog => {
                sqlx::query("DELETE FROM api_call_logs WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
            }
            RetentionTargetKind::AuditLog => {
                sqlx::query("DELETE FROM audit_logs WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
            }
            RetentionTargetKind::ChangeEvent => {
                sqlx::query("DELETE FROM article_change_events WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
            }
            RetentionTargetKind::DeliveryOutbox => {
                sqlx::query("DELETE FROM delivery_outbox WHERE id = $1")
                    .bind(id)
                    .execute(&self.pool)
                    .await
            }
        }
        .map_err(|e| e.to_string())?;
        Ok(result.rows_affected() > 0)
    }

    async fn is_asset_referenced(&self, asset_id: Uuid) -> Result<bool, String> {
        // 引用守卫：
        // 1) 当前 revision 的 change_event / outbox payload 含 asset_refs
        // 2) OCR block 仍指向该资产
        // 3) image_decisions 仍挂着该资产
        // 4) 未投递 outbox 的 payload 引用该资产
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM article_assets a
                JOIN articles art ON art.id = a.article_id
                JOIN article_change_events e
                  ON e.article_id = art.id
                 AND e.article_revision = art.current_revision
                WHERE a.id = $1
                  AND e.payload_json ? 'asset_refs'
                  AND e.payload_json->'asset_refs' @> to_jsonb($1::text)
            )
            OR EXISTS (
                SELECT 1 FROM article_ocr_blocks WHERE asset_id = $1
            )
            OR EXISTS (
                SELECT 1 FROM article_image_decisions WHERE asset_id = $1
            )
            OR EXISTS (
                SELECT 1
                FROM delivery_outbox o
                WHERE o.status IN ('pending', 'delivering')
                  AND o.payload_json ? 'asset_refs'
                  AND o.payload_json->'asset_refs' @> to_jsonb($1::text)
            ) AS referenced
            "#,
        )
        .bind(asset_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(row.try_get("referenced").map_err(|e| e.to_string())?)
    }

    async fn is_outbox_undelivered(&self, outbox_id: Uuid) -> Result<bool, String> {
        let row = sqlx::query(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM delivery_outbox
                WHERE id = $1
                  AND status IN ('pending', 'delivering')
            ) AS undelivered
            "#,
        )
        .bind(outbox_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(row.try_get("undelivered").map_err(|e| e.to_string())?)
    }
}
