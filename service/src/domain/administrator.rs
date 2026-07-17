//! 唯一系统管理员领域模型。

use chrono::{DateTime, Utc};

/// 部署 bootstrap 凭据（仅内存短暂持有，禁止日志）。
#[derive(Clone)]
pub struct BootstrapCredentials {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for BootstrapCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapCredentials")
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// 唯一管理员（singleton）。
#[derive(Debug, Clone)]
pub struct Administrator {
    pub username: String,
    pub password_hash: String,
    pub must_change_password: bool,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Administrator {
    pub fn public_view(&self) -> AdministratorPublic {
        AdministratorPublic {
            username: self.username.clone(),
            must_change_password: self.must_change_password,
            password_changed_at: self.password_changed_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }
}

/// 对外可见的管理员信息（无 hash）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct AdministratorPublic {
    pub username: String,
    pub must_change_password: bool,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
