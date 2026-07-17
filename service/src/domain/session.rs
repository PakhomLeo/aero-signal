//! 会话 token：随机明文只返回一次，库内只存 hash。

use chrono::{DateTime, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 明文 session token（仅创建时返回给客户端）。
#[derive(Clone, PartialEq, Eq)]
pub struct SessionToken(String);

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionToken([redacted])")
    }
}

impl SessionToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for SessionToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Display 故意不实现完整明文打印到日志；Debug 已隐藏。
        write!(f, "[redacted-session-token]")
    }
}

/// 管理员会话记录（不含明文 token）。
#[derive(Debug, Clone)]
pub struct AdminSession {
    pub id: Uuid,
    pub session_hash: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub ip_hash: Option<String>,
    pub user_agent_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl AdminSession {
    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at > now
    }
}

/// 生成 32 字节随机 token，以 URL-safe base64 编码。
pub fn generate_session_token() -> SessionToken {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    SessionToken(base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        bytes,
    ))
}

/// SHA-256 hex；库内只存此值。
pub fn hash_session_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// 对 IP / UA 做单向 hash，避免明文入库。
pub fn hash_client_hint(value: &str) -> String {
    hash_session_token(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_hash_is_stable_and_not_raw() {
        let token = generate_session_token();
        let h1 = hash_session_token(token.as_str());
        let h2 = hash_session_token(token.as_str());
        assert_eq!(h1, h2);
        assert_ne!(h1, token.as_str());
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn display_redacts_token() {
        let token = SessionToken("secret-token-value".into());
        assert_eq!(token.to_string(), "[redacted-session-token]");
        assert!(!format!("{token:?}").contains("secret-token-value"));
    }

    #[test]
    fn session_active_checks_expiry_and_revoke() {
        let now = Utc::now();
        let mut s = AdminSession {
            id: Uuid::new_v4(),
            session_hash: "abc".into(),
            expires_at: now + chrono::Duration::hours(1),
            revoked_at: None,
            ip_hash: None,
            user_agent_hash: None,
            created_at: now,
        };
        assert!(s.is_active(now));
        s.revoked_at = Some(now);
        assert!(!s.is_active(now));
        s.revoked_at = None;
        s.expires_at = now - chrono::Duration::seconds(1);
        assert!(!s.is_active(now));
    }
}
