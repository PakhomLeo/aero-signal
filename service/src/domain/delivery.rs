//! 分组投递、API Token 与调用日志领域模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::session::{generate_session_token, hash_session_token};

/// Token scope。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ApiScope {
    #[serde(rename = "articles:read")]
    ArticlesRead,
    #[serde(rename = "sources:read")]
    SourcesRead,
    #[serde(rename = "service_health:read")]
    ServiceHealthRead,
}

impl ApiScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArticlesRead => "articles:read",
            Self::SourcesRead => "sources:read",
            Self::ServiceHealthRead => "service_health:read",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "articles:read" => Ok(Self::ArticlesRead),
            "sources:read" => Ok(Self::SourcesRead),
            "service_health:read" => Ok(Self::ServiceHealthRead),
            other => Err(format!("未知 scope: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryGroupStatus {
    Active,
    Disabled,
}

impl DeliveryGroupStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            other => Err(format!("未知分组状态: {other}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryGroup {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub status: DeliveryGroupStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryGroupSource {
    pub delivery_group_id: Uuid,
    pub source_account_id: Uuid,
    pub bound_at: DateTime<Utc>,
}

/// 库内 Token 记录（无明文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToken {
    pub id: Uuid,
    pub delivery_group_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub token_hash: String,
    pub scopes: Vec<ApiScope>,
    pub ip_allowlist: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl ApiToken {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        if self.revoked_at.is_some() {
            return false;
        }
        if let Some(exp) = self.expires_at
            && exp <= now
        {
            return false;
        }
        true
    }

    pub fn has_scope(&self, scope: ApiScope) -> bool {
        self.scopes.contains(&scope)
    }

    /// 空 allowlist = 不限制；否则要求精确匹配。
    pub fn ip_allowed(&self, client_ip: Option<&str>) -> bool {
        if self.ip_allowlist.is_empty() {
            return true;
        }
        match client_ip {
            Some(ip) => self.ip_allowlist.iter().any(|a| a == ip),
            None => false,
        }
    }
}

/// 明文 Token（仅创建时返回一次）。
#[derive(Clone, PartialEq, Eq)]
pub struct PlainApiToken(String);

impl std::fmt::Debug for PlainApiToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlainApiToken([redacted])")
    }
}

impl std::fmt::Display for PlainApiToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[redacted-api-token]")
    }
}

impl PlainApiToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, Clone)]
pub struct IssuedApiToken {
    pub token: ApiToken,
    pub plaintext: PlainApiToken,
}

/// 生成 API Token：`sig_` + URL-safe base64。
pub fn generate_api_token() -> PlainApiToken {
    let raw = generate_session_token().into_inner();
    PlainApiToken(format!("sig_{raw}"))
}

pub fn hash_api_token(token: &str) -> String {
    hash_session_token(token)
}

pub fn token_prefix(token: &str) -> String {
    let chars: String = token.chars().take(12).collect();
    if chars.is_empty() {
        "sig_****".into()
    } else {
        chars
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCallLog {
    pub id: Uuid,
    pub api_token_id: Uuid,
    pub delivery_group_id: Uuid,
    pub method: String,
    pub path: String,
    pub scope_used: Option<String>,
    pub status_code: i32,
    pub duration_ms: i32,
    pub redaction_count: i32,
    pub client_ip_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// 管理端 Token 视图（无 hash / 无明文）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTokenPublic {
    pub id: Uuid,
    pub delivery_group_id: Uuid,
    pub name: String,
    pub token_prefix: String,
    pub scopes: Vec<String>,
    pub ip_allowlist: Vec<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub status: String,
}

impl ApiTokenPublic {
    pub fn from_token(t: &ApiToken, now: DateTime<Utc>) -> Self {
        let status = if t.revoked_at.is_some() {
            "revoked"
        } else if t.expires_at.map(|e| e <= now).unwrap_or(false) {
            "expired"
        } else {
            "active"
        };
        Self {
            id: t.id,
            delivery_group_id: t.delivery_group_id,
            name: t.name.clone(),
            token_prefix: t.token_prefix.clone(),
            scopes: t.scopes.iter().map(|s| s.as_str().to_string()).collect(),
            ip_allowlist: t.ip_allowlist.clone(),
            expires_at: t.expires_at,
            revoked_at: t.revoked_at,
            last_used_at: t.last_used_at,
            created_at: t.created_at,
            status: status.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryGroupPublic {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub status: DeliveryGroupStatus,
    pub source_account_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_redacts_in_debug() {
        let t = PlainApiToken("sig_secret".into());
        assert!(!format!("{t:?}").contains("secret"));
        assert_eq!(t.to_string(), "[redacted-api-token]");
    }

    #[test]
    fn scope_roundtrip() {
        assert_eq!(
            ApiScope::parse("articles:read").unwrap(),
            ApiScope::ArticlesRead
        );
        assert_eq!(ApiScope::ServiceHealthRead.as_str(), "service_health:read");
    }

    #[test]
    fn ip_allowlist_empty_allows_all() {
        let mut t = sample_token();
        t.ip_allowlist = vec![];
        assert!(t.ip_allowed(None));
        assert!(t.ip_allowed(Some("1.2.3.4")));
        t.ip_allowlist = vec!["1.2.3.4".into()];
        assert!(!t.ip_allowed(None));
        assert!(t.ip_allowed(Some("1.2.3.4")));
        assert!(!t.ip_allowed(Some("9.9.9.9")));
    }

    fn sample_token() -> ApiToken {
        ApiToken {
            id: Uuid::new_v4(),
            delivery_group_id: Uuid::new_v4(),
            name: "t".into(),
            token_prefix: "sig_abc".into(),
            token_hash: "h".into(),
            scopes: vec![ApiScope::ArticlesRead],
            ip_allowlist: vec![],
            expires_at: None,
            revoked_at: None,
            last_used_at: None,
            created_at: Utc::now(),
        }
    }
}
