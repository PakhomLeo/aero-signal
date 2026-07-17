//! 监测公众号仓储 trait。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::domain::settings::WorkingWindow;
use crate::domain::source::{
    AccountFetchRules, IdConfidence, SourceAccount, SourceAccountId, SourceHeartbeatEvent,
    SourceIdType, SourceLevel, SourceStatus, SourceValidationRun, ValidationCandidateResult,
    ValidationRunStatus,
};

#[derive(Debug, Clone)]
pub struct ApplyHeartbeatOutcome {
    pub source_account_id: Uuid,
    /// Version read before the decision was made.  The repository must reject
    /// the command if another writer changed the source in the meantime.
    pub expected_version: i64,
    pub rules: ApplyHeartbeatRules,
    pub event: SourceHeartbeatEvent,
    pub source_status: Option<SourceStatus>,
}

#[derive(Debug, Clone)]
pub struct AppliedHeartbeatOutcome {
    pub rules: AccountFetchRules,
    pub event: SourceHeartbeatEvent,
}

#[derive(Debug, Clone)]
pub struct CreateSourceAccount {
    pub id: Uuid,
    pub display_name: String,
    pub status: SourceStatus,
    pub preferred_connector: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CreateSourceAccountId {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub id_type: SourceIdType,
    pub id_value: String,
    pub confidence: IdConfidence,
}

#[derive(Debug, Clone)]
pub struct CreateValidationRun {
    pub id: Uuid,
    pub input_url: String,
    pub input_hash: String,
    pub parsed_source_json: serde_json::Value,
    pub connector_account_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct CompleteValidationRun {
    pub run_id: Uuid,
    pub status: ValidationRunStatus,
    pub candidate_count: u8,
    pub successful_count: u8,
    pub available_count: u8,
    pub missing_reason: Option<String>,
    pub candidates: Vec<ValidationCandidateResult>,
    pub source_account_id: Option<Uuid>,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SaveValidationProgress {
    pub run_id: Uuid,
    pub candidate_count: u8,
    pub available_count: u8,
    pub missing_reason: Option<String>,
    pub candidates: Vec<ValidationCandidateResult>,
    pub source_account_id: Uuid,
}

#[derive(Debug, Clone)]
pub struct UpsertFetchRules {
    pub source_account_id: Uuid,
    pub base_level: SourceLevel,
    pub working_windows: Vec<WorkingWindow>,
    pub first_heartbeat_at: DateTime<Utc>,
    pub next_heartbeat_at: DateTime<Utc>,
    pub connector_override: Option<String>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub enum FieldPatch<T> {
    Keep,
    Set(T),
    Clear,
}

#[derive(Debug, Clone)]
pub struct PatchSourceAggregate {
    pub source_account_id: Uuid,
    pub expected_version: i64,
    pub display_name: Option<String>,
    pub preferred_connector: FieldPatch<String>,
    pub status: Option<SourceStatus>,
    pub base_level: Option<SourceLevel>,
    pub working_windows: Option<Vec<WorkingWindow>>,
    pub connector_override: FieldPatch<String>,
    pub interval_override_hours: FieldPatch<i32>,
    /// `None` means keep the existing scheduling switch; `Some` sets it.
    pub enabled: Option<bool>,
    /// Tri-state manual schedule override. Clearing the override deliberately
    /// keeps the computed `next_heartbeat_at` unchanged until the heartbeat
    /// service recalculates it.
    pub manual_next_heartbeat_at: FieldPatch<DateTime<Utc>>,
    /// Legacy alias retained for clients that still send `next_heartbeat_at`.
    /// It is applied only when `manual_next_heartbeat_at` is `Keep`.
    pub next_heartbeat_at: Option<DateTime<Utc>>,
    pub auto_promotion_enabled: Option<bool>,
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PatchedSourceAggregate {
    pub previous_account: SourceAccount,
    pub previous_rules: AccountFetchRules,
    pub account: SourceAccount,
    pub rules: AccountFetchRules,
}

#[async_trait]
pub trait SourceRepository: Send + Sync {
    async fn insert_source(&self, cmd: CreateSourceAccount) -> Result<SourceAccount, String>;

    async fn get_source(&self, id: Uuid) -> Result<Option<SourceAccount>, String>;

    async fn update_source_status(
        &self,
        id: Uuid,
        status: SourceStatus,
        now: DateTime<Utc>,
    ) -> Result<SourceAccount, String>;

    async fn patch_source(
        &self,
        id: Uuid,
        display_name: Option<String>,
        preferred_connector: Option<Option<String>>,
        status: Option<SourceStatus>,
        now: DateTime<Utc>,
    ) -> Result<SourceAccount, String>;

    async fn patch_source_aggregate(
        &self,
        cmd: PatchSourceAggregate,
    ) -> Result<PatchedSourceAggregate, String>;

    async fn list_sources(&self) -> Result<Vec<SourceAccount>, String>;

    async fn find_by_strong_id(
        &self,
        id_type: SourceIdType,
        id_value: &str,
    ) -> Result<Option<(SourceAccount, SourceAccountId)>, String>;

    async fn insert_source_id(&self, cmd: CreateSourceAccountId)
    -> Result<SourceAccountId, String>;

    async fn list_source_ids(
        &self,
        source_account_id: Uuid,
    ) -> Result<Vec<SourceAccountId>, String>;

    async fn insert_validation_run(
        &self,
        cmd: CreateValidationRun,
    ) -> Result<SourceValidationRun, String>;

    async fn get_validation_run(&self, id: Uuid) -> Result<Option<SourceValidationRun>, String>;

    async fn get_validation_run_by_hash(
        &self,
        input_hash: &str,
    ) -> Result<Option<SourceValidationRun>, String>;

    async fn complete_validation_run(
        &self,
        cmd: CompleteValidationRun,
    ) -> Result<SourceValidationRun, String>;

    async fn save_validation_progress(
        &self,
        cmd: SaveValidationProgress,
    ) -> Result<SourceValidationRun, String>;

    async fn upsert_fetch_rules(&self, cmd: UpsertFetchRules) -> Result<AccountFetchRules, String>;

    async fn get_fetch_rules(
        &self,
        source_account_id: Uuid,
    ) -> Result<Option<AccountFetchRules>, String>;

    /// 由 HeartbeatService 唯一调用：更新 next/等级/空抓等字段。
    async fn apply_heartbeat_rules(
        &self,
        patch: ApplyHeartbeatRules,
    ) -> Result<AccountFetchRules, String>;

    /// Atomically apply heartbeat rules, append the business event, and (when
    /// requested) transition the source status.  Implementations must treat
    /// `event.event_key` as an idempotency key scoped to the source account.
    async fn apply_heartbeat_outcome(
        &self,
        cmd: ApplyHeartbeatOutcome,
    ) -> Result<AppliedHeartbeatOutcome, String>;

    async fn insert_heartbeat_event(
        &self,
        event: SourceHeartbeatEvent,
    ) -> Result<SourceHeartbeatEvent, String>;

    async fn list_heartbeat_events(
        &self,
        source_account_id: Uuid,
        limit: usize,
    ) -> Result<Vec<SourceHeartbeatEvent>, String>;
}

#[derive(Debug, Clone)]
pub struct ApplyHeartbeatRules {
    pub source_account_id: Uuid,
    pub base_level: SourceLevel,
    pub next_heartbeat_at: DateTime<Utc>,
    pub consecutive_empty_fetches: i32,
    pub consecutive_technical_failures: i32,
    pub low_activity: bool,
    pub promotion_until: Option<DateTime<Utc>>,
    pub manual_next_heartbeat_at: Option<DateTime<Utc>>,
    pub now: DateTime<Utc>,
}
