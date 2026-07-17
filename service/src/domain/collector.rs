//! 采集登录账号领域模型与二维码会话状态机。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 采集账号持久化状态（落库）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorAccountStatus {
    Active,
    NeedsRelogin,
    Expired,
    Disabled,
    Failed,
}

impl CollectorAccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::NeedsRelogin => "needs_relogin",
            Self::Expired => "expired",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(Self::Active),
            "needs_relogin" => Ok(Self::NeedsRelogin),
            "expired" => Ok(Self::Expired),
            "disabled" => Ok(Self::Disabled),
            "failed" => Ok(Self::Failed),
            other => Err(format!("未知采集账号状态: {other}")),
        }
    }

    /// 账号级合法跳转。
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Active, Self::NeedsRelogin)
                | (Self::Active, Self::Disabled)
                | (Self::Active, Self::Expired)
                | (Self::Active, Self::Failed)
                | (Self::NeedsRelogin, Self::Active)
                | (Self::NeedsRelogin, Self::Disabled)
                | (Self::NeedsRelogin, Self::Failed)
                | (Self::Failed, Self::NeedsRelogin)
                | (Self::Expired, Self::NeedsRelogin)
                | (Self::Disabled, Self::NeedsRelogin)
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, String> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(format!(
                "非法账号状态跳转: {} → {}",
                self.as_str(),
                next.as_str()
            ))
        }
    }
}

/// 二维码登录会话状态（ticket 生命周期，不进 PG 账号 status）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QrLoginStatus {
    QrWaiting,
    QrScanned,
    Finalizing,
    Active,
    Expired,
    Cancelled,
    Failed,
}

impl QrLoginStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QrWaiting => "qr_waiting",
            Self::QrScanned => "qr_scanned",
            Self::Finalizing => "finalizing",
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "qr_waiting" => Ok(Self::QrWaiting),
            "qr_scanned" => Ok(Self::QrScanned),
            "finalizing" => Ok(Self::Finalizing),
            "active" => Ok(Self::Active),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            other => Err(format!("未知二维码会话状态: {other}")),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Active | Self::Expired | Self::Cancelled | Self::Failed
        )
    }

    /// 二维码会话合法跳转（含 active → needs_relogin 由账号状态机处理；
    /// 此处 needs_relogin 重新开扫对应回到 qr_waiting 由新 ticket 创建）。
    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::QrWaiting, Self::QrScanned)
                | (Self::QrWaiting, Self::Expired)
                | (Self::QrWaiting, Self::Cancelled)
                | (Self::QrWaiting, Self::Failed)
                | (Self::QrScanned, Self::Finalizing)
                | (Self::QrScanned, Self::Expired)
                | (Self::QrScanned, Self::Cancelled)
                | (Self::QrScanned, Self::Failed)
                | (Self::Finalizing, Self::Active)
                | (Self::Finalizing, Self::Failed)
                | (Self::Finalizing, Self::Expired)
                | (Self::Finalizing, Self::Cancelled)
                // 已确认后重复 poll 保持 active（幂等）
                | (Self::Active, Self::Active)
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, String> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(format!(
                "非法二维码会话状态跳转: {} → {}",
                self.as_str(),
                next.as_str()
            ))
        }
    }
}

/// 采集账号实体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorAccount {
    pub id: Uuid,
    pub connector: String,
    pub display_name: String,
    pub external_account_ref: Option<String>,
    pub status: CollectorAccountStatus,
    pub capability_json: Value,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    /// The account cannot be selected before this instant. `None` means no cooldown.
    pub cooldown_until: Option<DateTime<Utc>>,
    /// Remaining requests for the current budget day. `None` means unlimited.
    pub daily_budget_remaining: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl CollectorAccount {
    pub fn transition_status(
        &mut self,
        next: CollectorAccountStatus,
        now: DateTime<Utc>,
    ) -> Result<(), String> {
        self.status = self.status.transition(next)?;
        self.updated_at = now;
        Ok(())
    }
}

/// 登录会话（凭据仅存引用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorLoginSession {
    pub id: Uuid,
    pub collector_account_id: Uuid,
    pub credential_secret_ref: String,
    pub browser_profile_ref: Option<String>,
    /// Public-account IDs authorized by this login session.
    pub authorized_mp_ids: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub confirmed_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// 固定出口绑定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorEgressBinding {
    pub collector_account_id: Uuid,
    pub fixed_egress_id: Option<Uuid>,
    pub bound_at: DateTime<Utc>,
    pub changed_at: Option<DateTime<Utc>>,
    pub requires_relogin: bool,
}

/// API 脱敏账号摘要（无 cookie/token）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorAccountPublic {
    pub id: Uuid,
    pub connector: String,
    pub display_name: String,
    pub external_account_ref: Option<String>,
    pub status: CollectorAccountStatus,
    pub fixed_egress_id: Option<Uuid>,
    pub requires_relogin: bool,
    pub last_probe_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_failure_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 允许的连接器名（含 fake 供测试）。
pub fn validate_connector_name(name: &str) -> Result<(), String> {
    match name {
        "mp_admin" | "weread" | "fake" => Ok(()),
        other => Err(format!("不支持的连接器: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_happy_path_transitions() {
        let mut s = QrLoginStatus::QrWaiting;
        s = s.transition(QrLoginStatus::QrScanned).unwrap();
        s = s.transition(QrLoginStatus::Finalizing).unwrap();
        s = s.transition(QrLoginStatus::Active).unwrap();
        assert_eq!(s, QrLoginStatus::Active);
        assert!(s.transition(QrLoginStatus::Active).is_ok());
    }

    #[test]
    fn qr_rejects_illegal_jumps() {
        assert!(
            QrLoginStatus::QrWaiting
                .transition(QrLoginStatus::Active)
                .is_err()
        );
        assert!(
            QrLoginStatus::QrWaiting
                .transition(QrLoginStatus::Finalizing)
                .is_err()
        );
        assert!(
            QrLoginStatus::Active
                .transition(QrLoginStatus::QrWaiting)
                .is_err()
        );
        assert!(
            QrLoginStatus::Expired
                .transition(QrLoginStatus::Active)
                .is_err()
        );
    }

    #[test]
    fn qr_terminal_from_waiting() {
        assert!(
            QrLoginStatus::QrWaiting
                .transition(QrLoginStatus::Expired)
                .is_ok()
        );
        assert!(
            QrLoginStatus::QrWaiting
                .transition(QrLoginStatus::Cancelled)
                .is_ok()
        );
        assert!(
            QrLoginStatus::QrScanned
                .transition(QrLoginStatus::Failed)
                .is_ok()
        );
    }

    #[test]
    fn account_active_to_needs_relogin_and_disabled() {
        assert!(
            CollectorAccountStatus::Active
                .transition(CollectorAccountStatus::NeedsRelogin)
                .is_ok()
        );
        assert!(
            CollectorAccountStatus::Active
                .transition(CollectorAccountStatus::Disabled)
                .is_ok()
        );
        assert!(
            CollectorAccountStatus::NeedsRelogin
                .transition(CollectorAccountStatus::Active)
                .is_ok()
        );
        assert!(
            CollectorAccountStatus::Disabled
                .transition(CollectorAccountStatus::Active)
                .is_err()
        );
    }
}
