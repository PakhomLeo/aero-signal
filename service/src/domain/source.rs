//! 监测公众号、首次验证与抓取规则领域模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::domain::settings::WorkingWindow;

/// 监测公众号状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Verifying,
    PendingConfiguration,
    Active,
    Paused,
    NeedsAction,
    Archived,
}

impl SourceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verifying => "verifying",
            Self::PendingConfiguration => "pending_configuration",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::NeedsAction => "needs_action",
            Self::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "verifying" => Ok(Self::Verifying),
            "pending_configuration" => Ok(Self::PendingConfiguration),
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            "needs_action" => Ok(Self::NeedsAction),
            "archived" => Ok(Self::Archived),
            other => Err(format!("未知监测公众号状态: {other}")),
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Verifying, Self::PendingConfiguration)
                | (Self::Verifying, Self::NeedsAction)
                | (Self::NeedsAction, Self::Verifying)
                | (Self::NeedsAction, Self::PendingConfiguration)
                | (Self::PendingConfiguration, Self::Active)
                | (Self::PendingConfiguration, Self::NeedsAction)
                | (Self::Active, Self::Paused)
                | (Self::Active, Self::NeedsAction)
                | (Self::Active, Self::Archived)
                | (Self::Paused, Self::Active)
                | (Self::Paused, Self::Archived)
                | (Self::NeedsAction, Self::Active)
                | (Self::NeedsAction, Self::Archived)
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, String> {
        if self == next {
            return Ok(self);
        }
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(format!(
                "非法监测公众号状态跳转: {} → {}",
                self.as_str(),
                next.as_str()
            ))
        }
    }
}

/// 抓取等级。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceLevel {
    LO0,
    LO1,
    L1,
    L2,
    L3,
    L4,
}

impl SourceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LO0 => "LO0",
            Self::LO1 => "LO1",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
            Self::L4 => "L4",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "LO0" => Ok(Self::LO0),
            "LO1" => Ok(Self::LO1),
            "L1" => Ok(Self::L1),
            "L2" => Ok(Self::L2),
            "L3" => Ok(Self::L3),
            "L4" => Ok(Self::L4),
            other => Err(format!("未知等级: {other}")),
        }
    }

    /// 基础心跳间隔（天）。L4 不参与自动计划。
    pub fn base_interval_days(self) -> Option<i64> {
        match self {
            Self::LO0 | Self::LO1 => Some(1),
            Self::L1 => Some(2),
            Self::L2 => Some(7),
            Self::L3 => Some(30),
            Self::L4 => None,
        }
    }

    pub fn never_auto_demotes(self) -> bool {
        matches!(self, Self::LO0 | Self::LO1)
    }

    pub fn participates_in_auto_plan(self) -> bool {
        !matches!(self, Self::L4)
    }
}

/// 标识类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceIdType {
    Biz,
    Fakeid,
    Alias,
    WereadMpId,
}

impl SourceIdType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Biz => "biz",
            Self::Fakeid => "fakeid",
            Self::Alias => "alias",
            Self::WereadMpId => "weread_mp_id",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "biz" => Ok(Self::Biz),
            "fakeid" => Ok(Self::Fakeid),
            "alias" => Ok(Self::Alias),
            "weread_mp_id" => Ok(Self::WereadMpId),
            other => Err(format!("未知标识类型: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdConfidence {
    Strong,
    Weak,
}

impl IdConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::Weak => "weak",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "strong" => Ok(Self::Strong),
            "weak" => Ok(Self::Weak),
            other => Err(format!("未知置信度: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAccount {
    pub id: Uuid,
    pub display_name: String,
    pub status: SourceStatus,
    pub preferred_connector: Option<String>,
    pub added_at: DateTime<Utc>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAccountId {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub id_type: SourceIdType,
    pub id_value: String,
    pub confidence: IdConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationRunStatus {
    Running,
    Succeeded,
    Failed,
    Expired,
}

impl ValidationRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Expired => "expired",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "expired" => Ok(Self::Expired),
            other => Err(format!("未知验证 run 状态: {other}")),
        }
    }
}

/// 单篇验证候选结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationCandidateResult {
    pub canonical_url: String,
    pub title: Option<String>,
    pub identity_key: Option<String>,
    pub status: String,
    pub error_code: Option<String>,
    pub article_id: Option<Uuid>,
    /// 对应的详情抓取任务；用于刷新恢复和聚合，不含敏感信息。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceValidationRun {
    pub id: Uuid,
    pub input_url: String,
    pub input_hash: String,
    pub parsed_source_json: Value,
    pub connector_account_id: Option<Uuid>,
    pub status: ValidationRunStatus,
    pub candidate_count: u8,
    pub successful_count: u8,
    pub available_count: u8,
    pub missing_reason: Option<String>,
    pub candidates: Vec<ValidationCandidateResult>,
    pub source_account_id: Option<Uuid>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountFetchRules {
    pub source_account_id: Uuid,
    pub base_level: SourceLevel,
    pub connector_override: Option<String>,
    pub enabled: bool,
    pub first_heartbeat_at: DateTime<Utc>,
    pub next_heartbeat_at: DateTime<Utc>,
    pub working_windows: Vec<WorkingWindow>,
    pub interval_override_hours: Option<i32>,
    pub consecutive_empty_fetches: i32,
    pub consecutive_technical_failures: i32,
    pub low_activity: bool,
    pub promotion_until: Option<DateTime<Utc>>,
    pub manual_next_heartbeat_at: Option<DateTime<Utc>>,
    pub auto_promotion_enabled: bool,
    pub updated_at: DateTime<Utc>,
}

/// 心跳执行结果（写入 source_heartbeat_events.outcome）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatOutcome {
    NewArticle,
    EmptySuccess,
    TechnicalFailure,
    AccessBlocked,
    Manual,
}

impl HeartbeatOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NewArticle => "new_article",
            Self::EmptySuccess => "empty_success",
            Self::TechnicalFailure => "technical_failure",
            Self::AccessBlocked => "access_blocked",
            Self::Manual => "manual",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "new_article" => Ok(Self::NewArticle),
            "empty_success" => Ok(Self::EmptySuccess),
            "technical_failure" => Ok(Self::TechnicalFailure),
            "access_blocked" => Ok(Self::AccessBlocked),
            "manual" => Ok(Self::Manual),
            other => Err(format!("未知心跳 outcome: {other}")),
        }
    }
}

/// 可解释心跳事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceHeartbeatEvent {
    pub id: Uuid,
    pub source_account_id: Uuid,
    /// Stable business key supplied by the fetch command.  It is scoped to a
    /// source account and is used to make heartbeat application idempotent.
    pub event_key: String,
    pub outcome: HeartbeatOutcome,
    pub level_before: SourceLevel,
    pub level_after: SourceLevel,
    pub empty_count_after: i32,
    pub previous_next_at: Option<DateTime<Utc>>,
    pub next_heartbeat_at: Option<DateTime<Utc>>,
    pub reason_code: String,
    pub created_at: DateTime<Utc>,
}

/// 对外公开视图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAccountPublic {
    pub id: Uuid,
    pub display_name: String,
    pub status: SourceStatus,
    pub preferred_connector: Option<String>,
    pub version: i64,
    pub strong_ids: Vec<SourceAccountIdPublic>,
    pub added_at: DateTime<Utc>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    pub fetch_rules: Option<AccountFetchRulesPublic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceAccountIdPublic {
    pub id_type: SourceIdType,
    pub id_value: String,
    pub confidence: IdConfidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountFetchRulesPublic {
    pub base_level: SourceLevel,
    pub enabled: bool,
    pub first_heartbeat_at: DateTime<Utc>,
    pub next_heartbeat_at: DateTime<Utc>,
    pub working_windows: Vec<WorkingWindow>,
    pub connector_override: Option<String>,
    pub interval_override_hours: Option<i32>,
    pub manual_next_heartbeat_at: Option<DateTime<Utc>>,
    pub auto_promotion_enabled: bool,
    pub updated_at: DateTime<Utc>,
}
