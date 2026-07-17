//! PostgreSQL 监测公众号仓储。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::settings::WorkingWindow;
use crate::domain::source::{
    AccountFetchRules, HeartbeatOutcome, IdConfidence, SourceAccount, SourceAccountId,
    SourceHeartbeatEvent, SourceIdType, SourceLevel, SourceStatus, SourceValidationRun,
    ValidationCandidateResult, ValidationRunStatus,
};
use crate::repository::source::{
    AppliedHeartbeatOutcome, ApplyHeartbeatOutcome, ApplyHeartbeatRules, CompleteValidationRun,
    CreateSourceAccount, CreateSourceAccountId, CreateValidationRun, FieldPatch,
    PatchSourceAggregate, PatchedSourceAggregate, SaveValidationProgress, SourceRepository,
    UpsertFetchRules,
};

#[derive(Clone)]
pub struct PgSourceRepository {
    pool: PgPool,
}

impl PgSourceRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_source(row: &PgRow) -> Result<SourceAccount, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    Ok(SourceAccount {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        display_name: row.try_get("display_name").map_err(|e| e.to_string())?,
        status: SourceStatus::parse(&status_s)?,
        preferred_connector: row
            .try_get("preferred_connector")
            .map_err(|e| e.to_string())?,
        added_at: row.try_get("added_at").map_err(|e| e.to_string())?,
        last_fetch_at: row.try_get("last_fetch_at").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
        version: row.try_get("version").map_err(|e| e.to_string())?,
    })
}

fn map_source_id(row: &PgRow) -> Result<SourceAccountId, String> {
    let id_type_s: String = row.try_get("id_type").map_err(|e| e.to_string())?;
    let confidence_s: String = row.try_get("confidence").map_err(|e| e.to_string())?;
    Ok(SourceAccountId {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        id_type: SourceIdType::parse(&id_type_s)?,
        id_value: row.try_get("id_value").map_err(|e| e.to_string())?,
        confidence: IdConfidence::parse(&confidence_s)?,
    })
}

fn map_validation_run(row: &PgRow) -> Result<SourceValidationRun, String> {
    let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
    let candidates_json: serde_json::Value =
        row.try_get("candidates_json").map_err(|e| e.to_string())?;
    let candidates: Vec<ValidationCandidateResult> =
        serde_json::from_value(candidates_json).map_err(|e| e.to_string())?;
    let candidate_count: i16 = row.try_get("candidate_count").map_err(|e| e.to_string())?;
    let successful_count: i16 = row.try_get("successful_count").map_err(|e| e.to_string())?;
    let available_count: i16 = row.try_get("available_count").map_err(|e| e.to_string())?;
    Ok(SourceValidationRun {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        input_url: row.try_get("input_url").map_err(|e| e.to_string())?,
        input_hash: row.try_get("input_hash").map_err(|e| e.to_string())?,
        parsed_source_json: row
            .try_get("parsed_source_json")
            .map_err(|e| e.to_string())?,
        connector_account_id: row
            .try_get("connector_account_id")
            .map_err(|e| e.to_string())?,
        status: ValidationRunStatus::parse(&status_s)?,
        candidate_count: candidate_count as u8,
        successful_count: successful_count as u8,
        available_count: available_count as u8,
        missing_reason: row.try_get("missing_reason").map_err(|e| e.to_string())?,
        candidates,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        expires_at: row.try_get("expires_at").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
        completed_at: row.try_get("completed_at").map_err(|e| e.to_string())?,
    })
}

fn map_fetch_rules(row: &PgRow) -> Result<AccountFetchRules, String> {
    let level_s: String = row.try_get("base_level").map_err(|e| e.to_string())?;
    let windows_json: serde_json::Value =
        row.try_get("working_windows").map_err(|e| e.to_string())?;
    let working_windows: Vec<WorkingWindow> =
        serde_json::from_value(windows_json).map_err(|e| e.to_string())?;
    Ok(AccountFetchRules {
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        base_level: SourceLevel::parse(&level_s)?,
        connector_override: row
            .try_get("connector_override")
            .map_err(|e| e.to_string())?,
        enabled: row.try_get("enabled").map_err(|e| e.to_string())?,
        first_heartbeat_at: row
            .try_get("first_heartbeat_at")
            .map_err(|e| e.to_string())?,
        next_heartbeat_at: row
            .try_get("next_heartbeat_at")
            .map_err(|e| e.to_string())?,
        working_windows,
        interval_override_hours: row
            .try_get("interval_override_hours")
            .map_err(|e| e.to_string())?,
        consecutive_empty_fetches: row
            .try_get("consecutive_empty_fetches")
            .map_err(|e| e.to_string())?,
        consecutive_technical_failures: row
            .try_get("consecutive_technical_failures")
            .map_err(|e| e.to_string())?,
        low_activity: row.try_get("low_activity").map_err(|e| e.to_string())?,
        promotion_until: row.try_get("promotion_until").map_err(|e| e.to_string())?,
        manual_next_heartbeat_at: row
            .try_get("manual_next_heartbeat_at")
            .map_err(|e| e.to_string())?,
        auto_promotion_enabled: row
            .try_get("auto_promotion_enabled")
            .map_err(|e| e.to_string())?,
        updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
    })
}

fn map_heartbeat_event(row: &PgRow) -> Result<SourceHeartbeatEvent, String> {
    let outcome_s: String = row.try_get("outcome").map_err(|e| e.to_string())?;
    let before_s: String = row.try_get("level_before").map_err(|e| e.to_string())?;
    let after_s: String = row.try_get("level_after").map_err(|e| e.to_string())?;
    Ok(SourceHeartbeatEvent {
        id: row.try_get("id").map_err(|e| e.to_string())?,
        source_account_id: row
            .try_get("source_account_id")
            .map_err(|e| e.to_string())?,
        event_key: row.try_get("event_key").map_err(|e| e.to_string())?,
        outcome: HeartbeatOutcome::parse(&outcome_s)?,
        level_before: SourceLevel::parse(&before_s)?,
        level_after: SourceLevel::parse(&after_s)?,
        empty_count_after: row
            .try_get("empty_count_after")
            .map_err(|e| e.to_string())?,
        previous_next_at: row.try_get("previous_next_at").map_err(|e| e.to_string())?,
        next_heartbeat_at: row
            .try_get("next_heartbeat_at")
            .map_err(|e| e.to_string())?,
        reason_code: row.try_get("reason_code").map_err(|e| e.to_string())?,
        created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
    })
}

const SOURCE_COLS: &str = r#"
    id, display_name, status, preferred_connector, added_at, last_fetch_at,
    created_at, updated_at, version
"#;

const SOURCE_ID_COLS: &str = r#"
    id, source_account_id, id_type, id_value, confidence
"#;

const VALIDATION_COLS: &str = r#"
    id, input_url, input_hash, parsed_source_json, connector_account_id, status,
    candidate_count, successful_count, available_count, missing_reason,
    candidates_json, source_account_id, expires_at, created_at, completed_at
"#;

const RULES_COLS: &str = r#"
    source_account_id, base_level, connector_override, enabled,
    first_heartbeat_at, next_heartbeat_at, working_windows, interval_override_hours,
    consecutive_empty_fetches, consecutive_technical_failures, low_activity,
    promotion_until, manual_next_heartbeat_at, auto_promotion_enabled, updated_at
"#;

const HEARTBEAT_COLS: &str = r#"
    id, source_account_id, event_key, outcome, level_before, level_after, empty_count_after,
    previous_next_at, next_heartbeat_at, reason_code, created_at
"#;

#[async_trait]
impl SourceRepository for PgSourceRepository {
    async fn insert_source(&self, cmd: CreateSourceAccount) -> Result<SourceAccount, String> {
        let account = SourceAccount {
            id: cmd.id,
            display_name: cmd.display_name,
            status: cmd.status,
            preferred_connector: cmd.preferred_connector,
            added_at: cmd.now,
            last_fetch_at: None,
            created_at: cmd.now,
            updated_at: cmd.now,
            version: 1,
        };
        sqlx::query(
            r#"
            INSERT INTO source_accounts (
                id, display_name, status, preferred_connector, added_at, last_fetch_at,
                created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
            "#,
        )
        .bind(account.id)
        .bind(&account.display_name)
        .bind(account.status.as_str())
        .bind(&account.preferred_connector)
        .bind(account.added_at)
        .bind(account.last_fetch_at)
        .bind(account.created_at)
        .bind(account.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(account)
    }

    async fn get_source(&self, id: Uuid) -> Result<Option<SourceAccount>, String> {
        let row = sqlx::query(&format!(
            "SELECT {SOURCE_COLS} FROM source_accounts WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_source(&r)).transpose()
    }

    async fn update_source_status(
        &self,
        id: Uuid,
        status: SourceStatus,
        now: DateTime<Utc>,
    ) -> Result<SourceAccount, String> {
        let mut account = self
            .get_source(id)
            .await?
            .ok_or_else(|| "source.not_found: 监测公众号不存在".to_string())?;
        account.status = account.status.transition(status)?;
        account.updated_at = now;
        account.version += 1;
        sqlx::query(
            r#"
            UPDATE source_accounts SET status = $2, updated_at = $3, version = version + 1 WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(account.status.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(account)
    }

    async fn patch_source(
        &self,
        id: Uuid,
        display_name: Option<String>,
        preferred_connector: Option<Option<String>>,
        status: Option<SourceStatus>,
        now: DateTime<Utc>,
    ) -> Result<SourceAccount, String> {
        let mut account = self
            .get_source(id)
            .await?
            .ok_or_else(|| "source.not_found: 监测公众号不存在".to_string())?;
        if let Some(name) = display_name {
            account.display_name = name;
        }
        if let Some(conn) = preferred_connector {
            account.preferred_connector = conn;
        }
        if let Some(st) = status {
            account.status = account.status.transition(st)?;
        }
        account.updated_at = now;
        account.version += 1;
        sqlx::query(
            r#"
            UPDATE source_accounts SET
                display_name = $2,
                preferred_connector = $3,
                status = $4,
                updated_at = $5,
                version = version + 1
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(&account.display_name)
        .bind(&account.preferred_connector)
        .bind(account.status.as_str())
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(account)
    }

    async fn patch_source_aggregate(
        &self,
        cmd: PatchSourceAggregate,
    ) -> Result<PatchedSourceAggregate, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let current_row = sqlx::query(&format!(
            "SELECT {SOURCE_COLS} FROM source_accounts WHERE id = $1"
        ))
        .bind(cmd.source_account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        let current = map_source(&current_row)?;
        let previous_account = current.clone();
        let status = match cmd.status {
            Some(next) => current.status.transition(next)?,
            None => current.status,
        };
        let display_name = cmd.display_name.unwrap_or(current.display_name);
        let preferred_connector = match cmd.preferred_connector {
            FieldPatch::Keep => current.preferred_connector,
            FieldPatch::Set(value) => Some(value),
            FieldPatch::Clear => None,
        };
        let account_row = sqlx::query(&format!(
            r#"
            UPDATE source_accounts SET display_name=$3, preferred_connector=$4,
                status=$5, updated_at=$6, version=version+1
            WHERE id=$1 AND version=$2
            RETURNING {SOURCE_COLS}
        "#
        ))
        .bind(cmd.source_account_id)
        .bind(cmd.expected_version)
        .bind(display_name)
        .bind(preferred_connector)
        .bind(status.as_str())
        .bind(cmd.now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let Some(account_row) = account_row else {
            return Err("source.version_conflict: source account was modified".into());
        };
        let current_rules_row = sqlx::query(&format!(
            "SELECT {RULES_COLS} FROM account_fetch_rules WHERE source_account_id=$1"
        ))
        .bind(cmd.source_account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.fetch_rules_not_found: fetch rules not found".to_string())?;
        let current_rules = map_fetch_rules(&current_rules_row)?;
        let previous_rules = current_rules.clone();
        let connector = match cmd.connector_override {
            FieldPatch::Keep => current_rules.connector_override,
            FieldPatch::Set(value) => Some(value),
            FieldPatch::Clear => None,
        };
        let interval = match cmd.interval_override_hours {
            FieldPatch::Keep => current_rules.interval_override_hours,
            FieldPatch::Set(value) => Some(value),
            FieldPatch::Clear => None,
        };
        let (next, manual_next) = match cmd.manual_next_heartbeat_at {
            FieldPatch::Keep => match cmd.next_heartbeat_at {
                Some(value) => (value, Some(value)),
                None => (
                    current_rules.next_heartbeat_at,
                    current_rules.manual_next_heartbeat_at,
                ),
            },
            FieldPatch::Set(value) => (value, Some(value)),
            FieldPatch::Clear => (current_rules.next_heartbeat_at, None),
        };
        let windows =
            serde_json::to_value(cmd.working_windows.unwrap_or(current_rules.working_windows))
                .map_err(|e| e.to_string())?;
        let rules_row = sqlx::query(&format!(
            r#"
            UPDATE account_fetch_rules SET base_level=$2, working_windows=$3,
                connector_override=$4, interval_override_hours=$5,
                next_heartbeat_at=$6, manual_next_heartbeat_at=$7,
                enabled=$8, auto_promotion_enabled=$9, updated_at=$10
            WHERE source_account_id=$1 RETURNING {RULES_COLS}
        "#
        ))
        .bind(cmd.source_account_id)
        .bind(cmd.base_level.unwrap_or(current_rules.base_level).as_str())
        .bind(windows)
        .bind(connector)
        .bind(interval)
        .bind(next)
        .bind(manual_next)
        .bind(cmd.enabled.unwrap_or(current_rules.enabled))
        .bind(
            cmd.auto_promotion_enabled
                .unwrap_or(current_rules.auto_promotion_enabled),
        )
        .bind(cmd.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let result = PatchedSourceAggregate {
            previous_account,
            previous_rules,
            account: map_source(&account_row)?,
            rules: map_fetch_rules(&rules_row)?,
        };
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(result)
    }

    async fn list_sources(&self) -> Result<Vec<SourceAccount>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {SOURCE_COLS} FROM source_accounts ORDER BY added_at DESC"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_source).collect()
    }

    async fn find_by_strong_id(
        &self,
        id_type: SourceIdType,
        id_value: &str,
    ) -> Result<Option<(SourceAccount, SourceAccountId)>, String> {
        let row = sqlx::query(
            r#"
            SELECT
                s.id AS s_id, s.display_name, s.status, s.preferred_connector,
                s.added_at, s.last_fetch_at, s.created_at, s.updated_at, s.version,
                i.id AS i_id, i.source_account_id, i.id_type, i.id_value, i.confidence
            FROM source_account_ids i
            JOIN source_accounts s ON s.id = i.source_account_id
            WHERE i.id_type = $1 AND i.id_value = $2
            "#,
        )
        .bind(id_type.as_str())
        .bind(id_value)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        let Some(row) = row else {
            return Ok(None);
        };
        let status_s: String = row.try_get("status").map_err(|e| e.to_string())?;
        let account = SourceAccount {
            id: row.try_get("s_id").map_err(|e| e.to_string())?,
            display_name: row.try_get("display_name").map_err(|e| e.to_string())?,
            status: SourceStatus::parse(&status_s)?,
            preferred_connector: row
                .try_get("preferred_connector")
                .map_err(|e| e.to_string())?,
            added_at: row.try_get("added_at").map_err(|e| e.to_string())?,
            last_fetch_at: row.try_get("last_fetch_at").map_err(|e| e.to_string())?,
            created_at: row.try_get("created_at").map_err(|e| e.to_string())?,
            updated_at: row.try_get("updated_at").map_err(|e| e.to_string())?,
            version: row.try_get("version").map_err(|e| e.to_string())?,
        };
        let id_type_s: String = row.try_get("id_type").map_err(|e| e.to_string())?;
        let confidence_s: String = row.try_get("confidence").map_err(|e| e.to_string())?;
        let sid = SourceAccountId {
            id: row.try_get("i_id").map_err(|e| e.to_string())?,
            source_account_id: row
                .try_get("source_account_id")
                .map_err(|e| e.to_string())?,
            id_type: SourceIdType::parse(&id_type_s)?,
            id_value: row.try_get("id_value").map_err(|e| e.to_string())?,
            confidence: IdConfidence::parse(&confidence_s)?,
        };
        Ok(Some((account, sid)))
    }

    async fn insert_source_id(
        &self,
        cmd: CreateSourceAccountId,
    ) -> Result<SourceAccountId, String> {
        let rec = SourceAccountId {
            id: cmd.id,
            source_account_id: cmd.source_account_id,
            id_type: cmd.id_type,
            id_value: cmd.id_value,
            confidence: cmd.confidence,
        };
        sqlx::query(
            r#"
            INSERT INTO source_account_ids (
                id, source_account_id, id_type, id_value, confidence
            ) VALUES ($1,$2,$3,$4,$5)
            "#,
        )
        .bind(rec.id)
        .bind(rec.source_account_id)
        .bind(rec.id_type.as_str())
        .bind(&rec.id_value)
        .bind(rec.confidence.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rec)
    }

    async fn list_source_ids(
        &self,
        source_account_id: Uuid,
    ) -> Result<Vec<SourceAccountId>, String> {
        let rows = sqlx::query(&format!(
            "SELECT {SOURCE_ID_COLS} FROM source_account_ids WHERE source_account_id = $1"
        ))
        .bind(source_account_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_source_id).collect()
    }

    async fn insert_validation_run(
        &self,
        cmd: CreateValidationRun,
    ) -> Result<SourceValidationRun, String> {
        if let Some(existing) = self.get_validation_run_by_hash(&cmd.input_hash).await? {
            return Ok(existing);
        }
        let run = SourceValidationRun {
            id: cmd.id,
            input_url: cmd.input_url,
            input_hash: cmd.input_hash,
            parsed_source_json: cmd.parsed_source_json,
            connector_account_id: cmd.connector_account_id,
            status: ValidationRunStatus::Running,
            candidate_count: 0,
            successful_count: 0,
            available_count: 0,
            missing_reason: None,
            candidates: vec![],
            source_account_id: None,
            expires_at: cmd.expires_at,
            created_at: cmd.now,
            completed_at: None,
        };
        let candidates_json = serde_json::to_value(&run.candidates).map_err(|e| e.to_string())?;
        sqlx::query(
            r#"
            INSERT INTO source_validation_runs (
                id, input_url, input_hash, parsed_source_json, connector_account_id, status,
                candidate_count, successful_count, available_count, missing_reason,
                candidates_json, source_account_id, expires_at, created_at, completed_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            "#,
        )
        .bind(run.id)
        .bind(&run.input_url)
        .bind(&run.input_hash)
        .bind(&run.parsed_source_json)
        .bind(run.connector_account_id)
        .bind(run.status.as_str())
        .bind(run.candidate_count as i16)
        .bind(run.successful_count as i16)
        .bind(run.available_count as i16)
        .bind(&run.missing_reason)
        .bind(&candidates_json)
        .bind(run.source_account_id)
        .bind(run.expires_at)
        .bind(run.created_at)
        .bind(run.completed_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(run)
    }

    async fn get_validation_run(&self, id: Uuid) -> Result<Option<SourceValidationRun>, String> {
        let row = sqlx::query(&format!(
            "SELECT {VALIDATION_COLS} FROM source_validation_runs WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_validation_run(&r)).transpose()
    }

    async fn get_validation_run_by_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<SourceValidationRun>, String> {
        let row = sqlx::query(&format!(
            "SELECT {VALIDATION_COLS} FROM source_validation_runs WHERE input_hash = $1"
        ))
        .bind(input_hash)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_validation_run(&r)).transpose()
    }

    async fn complete_validation_run(
        &self,
        cmd: CompleteValidationRun,
    ) -> Result<SourceValidationRun, String> {
        let candidates_json = serde_json::to_value(&cmd.candidates).map_err(|e| e.to_string())?;
        let row = sqlx::query(&format!(
            r#"
            UPDATE source_validation_runs SET
                status = $2,
                candidate_count = $3,
                successful_count = $4,
                available_count = $5,
                missing_reason = $6,
                candidates_json = $7,
                source_account_id = $8,
                completed_at = $9
            WHERE id = $1
            RETURNING {VALIDATION_COLS}
            "#
        ))
        .bind(cmd.run_id)
        .bind(cmd.status.as_str())
        .bind(cmd.candidate_count as i16)
        .bind(cmd.successful_count as i16)
        .bind(cmd.available_count as i16)
        .bind(&cmd.missing_reason)
        .bind(&candidates_json)
        .bind(cmd.source_account_id)
        .bind(cmd.completed_at)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.validation_not_found: 验证 run 不存在".to_string())?;
        map_validation_run(&row)
    }

    async fn save_validation_progress(
        &self,
        cmd: SaveValidationProgress,
    ) -> Result<SourceValidationRun, String> {
        let candidates_json = serde_json::to_value(&cmd.candidates).map_err(|e| e.to_string())?;
        let row = sqlx::query(&format!(
            r#"
            UPDATE source_validation_runs SET
                candidate_count = $2,
                available_count = $3,
                missing_reason = $4,
                candidates_json = $5,
                source_account_id = $6
            WHERE id = $1 AND status = 'running'
            RETURNING {VALIDATION_COLS}
            "#
        ))
        .bind(cmd.run_id)
        .bind(cmd.candidate_count as i16)
        .bind(cmd.available_count as i16)
        .bind(&cmd.missing_reason)
        .bind(&candidates_json)
        .bind(cmd.source_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        if let Some(row) = row {
            map_validation_run(&row)
        } else {
            self.get_validation_run(cmd.run_id)
                .await?
                .ok_or_else(|| "source.validation_not_found: 验证 run 不存在".to_string())
        }
    }

    async fn upsert_fetch_rules(&self, cmd: UpsertFetchRules) -> Result<AccountFetchRules, String> {
        let windows = serde_json::to_value(&cmd.working_windows).map_err(|e| e.to_string())?;
        let rules = AccountFetchRules {
            source_account_id: cmd.source_account_id,
            base_level: cmd.base_level,
            connector_override: cmd.connector_override,
            enabled: true,
            first_heartbeat_at: cmd.first_heartbeat_at,
            next_heartbeat_at: cmd.next_heartbeat_at,
            working_windows: cmd.working_windows,
            interval_override_hours: None,
            consecutive_empty_fetches: 0,
            consecutive_technical_failures: 0,
            low_activity: false,
            promotion_until: None,
            manual_next_heartbeat_at: None,
            auto_promotion_enabled: true,
            updated_at: cmd.now,
        };
        sqlx::query(
            r#"
            INSERT INTO account_fetch_rules (
                source_account_id, base_level, connector_override, enabled,
                first_heartbeat_at, next_heartbeat_at, working_windows,
                interval_override_hours, consecutive_empty_fetches,
                consecutive_technical_failures, low_activity,
                promotion_until, manual_next_heartbeat_at, auto_promotion_enabled, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
            ON CONFLICT (source_account_id) DO UPDATE SET
                base_level = EXCLUDED.base_level,
                connector_override = EXCLUDED.connector_override,
                enabled = EXCLUDED.enabled,
                first_heartbeat_at = EXCLUDED.first_heartbeat_at,
                next_heartbeat_at = EXCLUDED.next_heartbeat_at,
                working_windows = EXCLUDED.working_windows,
                interval_override_hours = EXCLUDED.interval_override_hours,
                consecutive_empty_fetches = EXCLUDED.consecutive_empty_fetches,
                consecutive_technical_failures = EXCLUDED.consecutive_technical_failures,
                low_activity = EXCLUDED.low_activity,
                promotion_until = EXCLUDED.promotion_until,
                manual_next_heartbeat_at = EXCLUDED.manual_next_heartbeat_at,
                auto_promotion_enabled = EXCLUDED.auto_promotion_enabled,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(rules.source_account_id)
        .bind(rules.base_level.as_str())
        .bind(&rules.connector_override)
        .bind(rules.enabled)
        .bind(rules.first_heartbeat_at)
        .bind(rules.next_heartbeat_at)
        .bind(&windows)
        .bind(rules.interval_override_hours)
        .bind(rules.consecutive_empty_fetches)
        .bind(rules.consecutive_technical_failures)
        .bind(rules.low_activity)
        .bind(rules.promotion_until)
        .bind(rules.manual_next_heartbeat_at)
        .bind(rules.auto_promotion_enabled)
        .bind(rules.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(rules)
    }

    async fn get_fetch_rules(
        &self,
        source_account_id: Uuid,
    ) -> Result<Option<AccountFetchRules>, String> {
        let row = sqlx::query(&format!(
            "SELECT {RULES_COLS} FROM account_fetch_rules WHERE source_account_id = $1"
        ))
        .bind(source_account_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        row.map(|r| map_fetch_rules(&r)).transpose()
    }

    async fn apply_heartbeat_outcome(
        &self,
        cmd: ApplyHeartbeatOutcome,
    ) -> Result<AppliedHeartbeatOutcome, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;

        // Lock the source aggregate first. This serializes concurrent heartbeat
        // commands for the same source and makes the idempotency check race-free.
        let source_row = sqlx::query(&format!(
            "SELECT {SOURCE_COLS} FROM source_accounts WHERE id = $1 FOR UPDATE"
        ))
        .bind(cmd.source_account_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        let source = map_source(&source_row)?;

        if let Some(row) = sqlx::query(&format!(
            "SELECT {HEARTBEAT_COLS} FROM source_heartbeat_events WHERE source_account_id = $1 AND event_key = $2"
        ))
        .bind(cmd.source_account_id)
        .bind(&cmd.event.event_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        {
            let existing = map_heartbeat_event(&row)?;
            if existing.outcome != cmd.event.outcome {
                return Err("source.heartbeat_event_conflict: event key already used".into());
            }
            let rules_row = sqlx::query(&format!(
                "SELECT {RULES_COLS} FROM account_fetch_rules WHERE source_account_id = $1"
            ))
            .bind(cmd.source_account_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            let rules = map_fetch_rules(&rules_row)?;
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(AppliedHeartbeatOutcome {
                rules,
                event: existing,
            });
        }

        if source.version != cmd.expected_version {
            return Err("source.version_conflict: source account was modified".into());
        }
        let next_status = match cmd.source_status {
            Some(status) => source.status.transition(status)?,
            None => source.status,
        };
        let source_row = sqlx::query(&format!(
            "UPDATE source_accounts SET status=$2, version=version+1, updated_at=$3 WHERE id=$1 RETURNING {SOURCE_COLS}"
        ))
        .bind(cmd.source_account_id)
        .bind(next_status.as_str())
        .bind(cmd.rules.now)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

        let rules_row = sqlx::query(&format!(
            r#"
            UPDATE account_fetch_rules SET
                base_level = $2,
                next_heartbeat_at = $3,
                consecutive_empty_fetches = $4,
                consecutive_technical_failures = $5,
                low_activity = $6,
                promotion_until = $7,
                manual_next_heartbeat_at = $8,
                updated_at = $9
            WHERE source_account_id = $1
            RETURNING {RULES_COLS}
            "#
        ))
        .bind(cmd.source_account_id)
        .bind(cmd.rules.base_level.as_str())
        .bind(cmd.rules.next_heartbeat_at)
        .bind(cmd.rules.consecutive_empty_fetches)
        .bind(cmd.rules.consecutive_technical_failures)
        .bind(cmd.rules.low_activity)
        .bind(cmd.rules.promotion_until)
        .bind(cmd.rules.manual_next_heartbeat_at)
        .bind(cmd.rules.now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.fetch_rules_not_found: fetch rules not found".to_string())?;
        let rules = map_fetch_rules(&rules_row)?;

        sqlx::query(
            r#"
            INSERT INTO source_heartbeat_events (
                id, source_account_id, event_key, outcome, level_before, level_after,
                empty_count_after, previous_next_at, next_heartbeat_at, reason_code, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(cmd.event.id)
        .bind(cmd.event.source_account_id)
        .bind(&cmd.event.event_key)
        .bind(cmd.event.outcome.as_str())
        .bind(cmd.event.level_before.as_str())
        .bind(cmd.event.level_after.as_str())
        .bind(cmd.event.empty_count_after)
        .bind(cmd.event.previous_next_at)
        .bind(cmd.event.next_heartbeat_at)
        .bind(&cmd.event.reason_code)
        .bind(cmd.event.created_at)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        tx.commit().await.map_err(|e| e.to_string())?;
        let _ = source_row;
        Ok(AppliedHeartbeatOutcome {
            rules,
            event: cmd.event,
        })
    }

    async fn apply_heartbeat_rules(
        &self,
        patch: ApplyHeartbeatRules,
    ) -> Result<AccountFetchRules, String> {
        let mut tx = self.pool.begin().await.map_err(|e| e.to_string())?;
        let touched =
            sqlx::query("UPDATE source_accounts SET version=version+1, updated_at=$2 WHERE id=$1")
                .bind(patch.source_account_id)
                .bind(patch.now)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
        if touched.rows_affected() != 1 {
            return Err("source.not_found: source account not found".into());
        }
        let row = sqlx::query(&format!(
            r#"
            UPDATE account_fetch_rules SET
                base_level = $2,
                next_heartbeat_at = $3,
                consecutive_empty_fetches = $4,
                consecutive_technical_failures = $5,
                low_activity = $6,
                promotion_until = $7,
                manual_next_heartbeat_at = $8,
                updated_at = $9
            WHERE source_account_id = $1
            RETURNING {RULES_COLS}
            "#
        ))
        .bind(patch.source_account_id)
        .bind(patch.base_level.as_str())
        .bind(patch.next_heartbeat_at)
        .bind(patch.consecutive_empty_fetches)
        .bind(patch.consecutive_technical_failures)
        .bind(patch.low_activity)
        .bind(patch.promotion_until)
        .bind(patch.manual_next_heartbeat_at)
        .bind(patch.now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "source.fetch_rules_not_found: 抓取规则不存在".to_string())?;
        let rules = map_fetch_rules(&row)?;
        tx.commit().await.map_err(|e| e.to_string())?;
        Ok(rules)
    }

    async fn insert_heartbeat_event(
        &self,
        event: SourceHeartbeatEvent,
    ) -> Result<SourceHeartbeatEvent, String> {
        sqlx::query(
            r#"
            INSERT INTO source_heartbeat_events (
                id, source_account_id, event_key, outcome, level_before, level_after,
                empty_count_after, previous_next_at, next_heartbeat_at,
                reason_code, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            "#,
        )
        .bind(event.id)
        .bind(event.source_account_id)
        .bind(&event.event_key)
        .bind(event.outcome.as_str())
        .bind(event.level_before.as_str())
        .bind(event.level_after.as_str())
        .bind(event.empty_count_after)
        .bind(event.previous_next_at)
        .bind(event.next_heartbeat_at)
        .bind(&event.reason_code)
        .bind(event.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        Ok(event)
    }

    async fn list_heartbeat_events(
        &self,
        source_account_id: Uuid,
        limit: usize,
    ) -> Result<Vec<SourceHeartbeatEvent>, String> {
        let rows = sqlx::query(&format!(
            r#"
            SELECT {HEARTBEAT_COLS}
            FROM source_heartbeat_events
            WHERE source_account_id = $1
            ORDER BY created_at DESC
            LIMIT $2
            "#
        ))
        .bind(source_account_id)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| e.to_string())?;
        rows.iter().map(map_heartbeat_event).collect()
    }
}
