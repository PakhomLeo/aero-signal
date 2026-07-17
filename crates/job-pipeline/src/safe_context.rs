//! safe_context 字段白名单：禁止 Cookie、token、正文等敏感键。

use serde_json::{Map, Value};
use thiserror::Error;

/// 允许写入 `safe_context_json` 的顶层键。
pub const ALLOWED_KEYS: &[&str] = &[
    "job_kind",
    "http_status",
    "object_count",
    "egress_type",
    "error_code",
    "error_codes",
    "attempt",
    "stage",
    "source_id",
    "article_id",
    "collector_id",
    "status",
    "counts",
    "duration_ms",
];

/// 明确拒绝的敏感键（大小写不敏感匹配）。
pub const DENIED_KEYS: &[&str] = &[
    "cookie",
    "cookies",
    "token",
    "access_token",
    "refresh_token",
    "authorization",
    "password",
    "secret",
    "proxy_password",
    "proxy_secret",
    "body",
    "content",
    "content_md",
    "html",
    "raw_html",
    "qr_url",
    "qrcode",
    "session",
    "credential",
    "credentials",
];

const MAX_DEPTH: usize = 4;
const MAX_NODES: usize = 128;
const MAX_STRING_BYTES: usize = 2048;

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SafeContextError {
    #[error("safe_context 必须是 JSON 对象")]
    NotObject,
    #[error("safe_context 含敏感键: {0}")]
    DeniedKey(String),
    #[error("safe_context 含未允许键: {0}")]
    UnknownKey(String),
}

fn key_denied(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    DENIED_KEYS.iter().any(|d| lower == *d || lower.contains(d))
}

fn key_allowed(key: &str) -> bool {
    ALLOWED_KEYS.contains(&key)
}

/// 校验 safe_context：必须是对象，且仅含白名单键、不含敏感键。
pub fn validate_safe_context(value: &Value) -> Result<(), SafeContextError> {
    let obj = value.as_object().ok_or(SafeContextError::NotObject)?;
    let mut nodes = 0;
    for key in obj.keys() {
        if key_denied(key) {
            return Err(SafeContextError::DeniedKey(key.clone()));
        }
        if !key_allowed(key) {
            return Err(SafeContextError::UnknownKey(key.clone()));
        }
    }
    validate_nested(value, 0, &mut nodes)?;
    Ok(())
}

fn validate_nested(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), SafeContextError> {
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_NODES || depth > MAX_DEPTH {
        return Err(SafeContextError::UnknownKey(
            "safe_context_too_large".into(),
        ));
    }
    match value {
        Value::Object(obj) => {
            for (key, child) in obj {
                if key_denied(key) {
                    return Err(SafeContextError::DeniedKey(key.clone()));
                }
                validate_nested(child, depth + 1, nodes)?;
            }
        }
        Value::Array(values) => {
            for child in values {
                validate_nested(child, depth + 1, nodes)?;
            }
        }
        Value::String(value) if value.len() > MAX_STRING_BYTES => {
            return Err(SafeContextError::UnknownKey(
                "safe_context_string_too_large".into(),
            ));
        }
        _ => {}
    }
    Ok(())
}

/// 过滤：去掉敏感键与未知键，仅保留白名单字段。
pub fn filter_safe_context(value: &Value) -> Result<Value, SafeContextError> {
    let obj = value.as_object().ok_or(SafeContextError::NotObject)?;
    let mut out = Map::new();
    for (key, v) in obj {
        if key_denied(key) {
            return Err(SafeContextError::DeniedKey(key.clone()));
        }
        if key_allowed(key) {
            out.insert(key.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_whitelist_keys() {
        let v = json!({
            "job_kind": "fetch_source",
            "http_status": 200,
            "object_count": 3,
            "egress_type": "fixed",
            "error_code": "ok"
        });
        assert!(validate_safe_context(&v).is_ok());
        let filtered = filter_safe_context(&v).unwrap();
        assert_eq!(filtered["job_kind"], "fetch_source");
    }

    #[test]
    fn rejects_cookie() {
        let v = json!({"cookie": "sid=abc"});
        assert_eq!(
            validate_safe_context(&v),
            Err(SafeContextError::DeniedKey("cookie".into()))
        );
    }

    #[test]
    fn rejects_token() {
        let v = json!({"token": "secret"});
        assert!(matches!(
            validate_safe_context(&v),
            Err(SafeContextError::DeniedKey(_))
        ));
    }

    #[test]
    fn rejects_body_content() {
        let v = json!({"content_md": "# hello"});
        assert!(matches!(
            validate_safe_context(&v),
            Err(SafeContextError::DeniedKey(_))
        ));
        let v2 = json!({"body": "<html>"});
        assert!(matches!(
            validate_safe_context(&v2),
            Err(SafeContextError::DeniedKey(_))
        ));
    }

    #[test]
    fn rejects_unknown_key() {
        let v = json!({"random_field": 1});
        assert_eq!(
            validate_safe_context(&v),
            Err(SafeContextError::UnknownKey("random_field".into()))
        );
    }

    #[test]
    fn rejects_non_object() {
        assert_eq!(
            validate_safe_context(&json!([])),
            Err(SafeContextError::NotObject)
        );
    }

    #[test]
    fn rejects_nested_sensitive_keys_and_oversized_values() {
        assert!(matches!(
            validate_safe_context(&json!({"counts": {"nested": {"access_token": "x"}}})),
            Err(SafeContextError::DeniedKey(_))
        ));
        assert!(matches!(
            validate_safe_context(&json!({"counts": "x".repeat(2049)})),
            Err(SafeContextError::UnknownKey(_))
        ));
    }
}
