//! 连接器错误与标准 reason_code。

use serde::{Deserialize, Serialize};

/// 连接器错误：对外暴露稳定 `reason_code`，消息已脱敏。
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{reason_code}: {message}")]
pub struct ConnectorError {
    /// 形如 `module.reason` 的业务码。
    pub reason_code: String,
    /// 可对外展示的脱敏说明。
    pub message: String,
}

impl ConnectorError {
    pub fn new(reason_code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            reason_code: reason_code.into(),
            message: message.into(),
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(codes::UNSUPPORTED, message)
    }

    pub fn needs_relogin(message: impl Into<String>) -> Self {
        Self::new(codes::NEEDS_RELOGIN, message)
    }

    pub fn unauthorized_enumeration(message: impl Into<String>) -> Self {
        Self::new(codes::UNAUTHORIZED_ENUMERATION, message)
    }

    pub fn capability_disabled(message: impl Into<String>) -> Self {
        Self::new(codes::CAPABILITY_DISABLED, message)
    }

    pub fn platform_restricted(message: impl Into<String>) -> Self {
        Self::new(codes::PLATFORM_RESTRICTED, message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(codes::FAILED, message)
    }
}

/// 稳定 reason_code 常量。
pub mod codes {
    pub const UNSUPPORTED: &str = "connector.unsupported";
    pub const CAPABILITY_DISABLED: &str = "connector.capability_disabled";
    pub const FAILED: &str = "connector.failed";
    pub const UNAUTHORIZED_ENUMERATION: &str = "mp_admin.unauthorized_enumeration";
    pub const NEEDS_RELOGIN: &str = "collector.needs_relogin";
    pub const INACTIVE: &str = "collector.inactive";
    pub const COOLING: &str = "collector.cooling";
    pub const BUDGET_EXHAUSTED: &str = "collector.budget_exhausted";
    pub const EGRESS_UNHEALTHY: &str = "egress.unhealthy";
    pub const NO_ACCOUNT: &str = "connector.no_available_account";
    pub const PLATFORM_RESTRICTED: &str = "connector.platform_restricted";
}

pub type ConnectorResult<T> = Result<T, ConnectorError>;
