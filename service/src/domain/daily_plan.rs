//! 每日抓取计划领域模型（M12）。

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 计划行状态（与 PRD / 迁移 CHECK 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DailyPlanStatus {
    Planned,
    Running,
    Succeeded,
    RetryWait,
    Failed,
    NeedsAction,
    Skipped,
}

impl DailyPlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::RetryWait => "retry_wait",
            Self::Failed => "failed",
            Self::NeedsAction => "needs_action",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "planned" => Ok(Self::Planned),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "retry_wait" => Ok(Self::RetryWait),
            "failed" => Ok(Self::Failed),
            "needs_action" => Ok(Self::NeedsAction),
            "skipped" => Ok(Self::Skipped),
            other => Err(format!("未知计划状态: {other}")),
        }
    }

    /// 重建计划仅可覆盖「planned 且未开始」。
    pub fn is_rebuildable(self) -> bool {
        matches!(self, Self::Planned)
    }

    pub fn is_started(self) -> bool {
        !matches!(self, Self::Planned | Self::Skipped)
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Planned, Self::Running)
                | (Self::Planned, Self::Skipped)
                | (Self::Running, Self::Succeeded)
                | (Self::Running, Self::Failed)
                | (Self::Running, Self::RetryWait)
                | (Self::Running, Self::NeedsAction)
                | (Self::RetryWait, Self::Running)
                | (Self::RetryWait, Self::Failed)
                | (Self::RetryWait, Self::NeedsAction)
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
                "非法计划状态跳转: {} → {}",
                self.as_str(),
                next.as_str()
            ))
        }
    }
}

/// 单日单源计划行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyFetchPlan {
    pub id: Uuid,
    pub plan_date: NaiveDate,
    pub source_account_id: Uuid,
    pub health_snapshot_id: Uuid,
    pub planned_run_at: DateTime<Utc>,
    pub status: DailyPlanStatus,
    pub reason_code: String,
    pub heartbeat_event_id: Option<Uuid>,
    pub fetch_job_id: Option<Uuid>,
    pub collector_account_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DailyFetchPlan {
    pub fn mark_skipped(&mut self, reason: &str, now: DateTime<Utc>) -> Result<(), String> {
        self.status = self.status.transition(DailyPlanStatus::Skipped)?;
        self.reason_code = reason.to_string();
        self.updated_at = now;
        Ok(())
    }

    pub fn mark_running(&mut self, now: DateTime<Utc>) -> Result<(), String> {
        self.status = self.status.transition(DailyPlanStatus::Running)?;
        self.updated_at = now;
        Ok(())
    }
}
