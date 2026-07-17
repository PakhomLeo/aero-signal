//! Job 领域类型与状态机。

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct JobId(uuid::Uuid);

impl JobId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub fn from_raw(id: uuid::Uuid) -> Self {
        Self(id)
    }

    pub fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl Default for JobId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for JobId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(uuid::Uuid::parse_str(s)?))
    }
}

/// 任务种类字符串（如 `daily_preflight`）。
pub type JobKind = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    RetryWait,
    Succeeded,
    Failed,
    NeedsAction,
    Cancelling,
    Cancelled,
    DeadLetter,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::RetryWait => "retry_wait",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::NeedsAction => "needs_action",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::DeadLetter => "dead_letter",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "retry_wait" => Self::RetryWait,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "needs_action" => Self::NeedsAction,
            "cancelling" => Self::Cancelling,
            "cancelled" => Self::Cancelled,
            "dead_letter" => Self::DeadLetter,
            _ => return None,
        })
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::DeadLetter
        )
    }

    pub fn can_cancel(&self) -> bool {
        matches!(
            self,
            Self::Queued | Self::Running | Self::RetryWait | Self::NeedsAction
        )
    }

    pub fn can_retry(&self) -> bool {
        matches!(self, Self::Failed | Self::DeadLetter)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub status: JobStatus,
    pub payload_json: serde_json::Value,
    pub idempotency_key: Option<String>,
    pub progress_percent: f64,
    pub progress_message: String,
    pub progress_stage: String,
    pub attempt_count: u32,
    pub max_attempts: u32,
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub locked_by: Option<String>,
    pub locked_until: Option<chrono::DateTime<chrono::Utc>>,
    pub lease_heartbeat_at: Option<chrono::DateTime<chrono::Utc>>,
    pub execution_timeout_at: Option<chrono::DateTime<chrono::Utc>>,
    pub priority: i32,
    pub dependency_keys: Vec<String>,
    pub cost_budget_limit: Option<f64>,
    pub cancel_requested_at: Option<chrono::DateTime<chrono::Utc>>,
    pub cancelled_at: Option<chrono::DateTime<chrono::Utc>>,
    pub resume_token: Option<String>,
    pub result_json: Option<serde_json::Value>,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub action_code: Option<String>,
    pub action_message: Option<String>,
    pub action_url_or_command: Option<String>,
    /// 仅允许脱敏字段的上下文（见 `safe_context`）。
    pub safe_context_json: Option<serde_json::Value>,
    pub created_by: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateJob {
    pub kind: JobKind,
    pub payload_json: serde_json::Value,
    pub idempotency_key: Option<String>,
    pub max_attempts: Option<u32>,
    pub priority: Option<i32>,
    pub dependency_keys: Vec<String>,
    pub cost_budget_limit: Option<f64>,
    pub resume_token: Option<String>,
    pub safe_context_json: Option<serde_json::Value>,
    pub created_by: Option<String>,
    /// 到点调度；`None` 表示立即可抢占。
    pub next_run_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl CreateJob {
    /// 便捷构造：kind + 幂等键 + payload。
    pub fn new(
        kind: impl Into<String>,
        idempotency_key: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            kind: kind.into(),
            payload_json: payload,
            idempotency_key: Some(idempotency_key.into()),
            max_attempts: None,
            priority: None,
            dependency_keys: vec![],
            cost_budget_limit: None,
            resume_token: None,
            safe_context_json: None,
            created_by: None,
            next_run_at: None,
        }
    }
}
