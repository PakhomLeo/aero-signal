//! 日志/追踪脱敏扫描辅助（M15）。
//!
//! 测试与本地扫描用：检测字符串中是否泄漏 cookie、token、二维码原始 URL、proxy password。

/// 禁止出现在日志 fixture / safe_context / 审计 details 中的敏感模式。
pub fn contains_forbidden_secret(text: &str) -> Option<&'static str> {
    let lower = text.to_ascii_lowercase();

    if lower.contains("cookie=") || lower.contains("set-cookie:") {
        return Some("cookie");
    }
    // Bearer / 明文 API token 形态
    if lower.contains("bearer ")
        || lower.contains("authorization: ")
        || text.contains("sig_live_")
        || text.contains("sk_test_")
    {
        return Some("token");
    }
    // 二维码原始内容 URL（微信登录常见），非 data:image
    if (lower.contains("https://mp.weixin.qq.com/")
        || lower.contains("weixin://")
        || lower.contains("https://open.weixin.qq.com/connect/"))
        && !lower.starts_with("data:image")
    {
        return Some("qr_raw_url");
    }
    // 代理密码：user:pass@host 或显式 proxy_password
    if lower.contains("proxy_password")
        || lower.contains("proxy-password")
        || (lower.contains("://")
            && lower.contains("@")
            && (lower.contains("proxy") || looks_like_userinfo_url(&lower)))
    {
        return Some("proxy_password");
    }
    None
}

fn looks_like_userinfo_url(lower: &str) -> bool {
    // scheme://user:pass@host
    if let Some(rest) = lower.split("://").nth(1)
        && let Some(creds) = rest.split('@').next()
    {
        return creds.contains(':') && !creds.is_empty();
    }
    false
}

/// 扫描多行日志 fixture；返回首次命中的 (行号, 类别)。
pub fn scan_log_fixture(fixture: &str) -> Option<(usize, &'static str)> {
    for (i, line) in fixture.lines().enumerate() {
        if let Some(kind) = contains_forbidden_secret(line) {
            return Some((i + 1, kind));
        }
    }
    None
}

/// 扫描 JSON 序列化后的 safe_context / audit details。
pub fn scan_json_value(value: &serde_json::Value) -> Option<&'static str> {
    contains_forbidden_secret(&value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_cookie_in_log_line() {
        let line = r#"INFO collector poll status=waiting cookie=SID=abc123 path=/"#;
        assert_eq!(contains_forbidden_secret(line), Some("cookie"));
    }

    #[test]
    fn rejects_bearer_token() {
        let line = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.fake";
        assert_eq!(contains_forbidden_secret(line), Some("token"));
    }

    #[test]
    fn rejects_qr_raw_url() {
        let line = "qr_content=https://mp.weixin.qq.com/cgi-bin/showqrcode?ticket=TICKET";
        assert_eq!(contains_forbidden_secret(line), Some("qr_raw_url"));
    }

    #[test]
    fn rejects_proxy_password_url() {
        let line = "egress allocate proxy_url=http://user:s3cret@proxy.example:8080";
        assert_eq!(contains_forbidden_secret(line), Some("proxy_password"));
    }

    #[test]
    fn allows_redacted_safe_context() {
        let fixture = r#"
INFO job progress job_id=111 status=running
INFO audit action=admin.login result=success
INFO health error_code=collector.needs_relogin redaction_count=2
INFO qr response has qr_data_url=data:image/png;base64,AAA
"#;
        assert!(scan_log_fixture(fixture).is_none());
    }

    #[test]
    fn scan_fixture_reports_line() {
        let fixture = "ok line\nbad cookie=x=1\n";
        assert_eq!(scan_log_fixture(fixture), Some((2, "cookie")));
    }

    #[test]
    fn scan_json_rejects_embedded_secrets() {
        let v = json!({
            "error_code": "egress.network_unreachable",
            "note": "proxy_password=hunter2"
        });
        assert_eq!(scan_json_value(&v), Some("proxy_password"));
    }

    #[test]
    fn sanitize_output_passes_scan() {
        use crate::domain::health::sanitize_safe_details;
        let raw = json!({
            "cookie": "sid=abc",
            "token": "secret-token",
            "proxy_url": "http://u:p@1.2.3.4:8080",
            "error_code": "collector.needs_relogin",
            "qr_url": "https://mp.weixin.qq.com/cgi-bin/showqrcode?ticket=X"
        });
        let clean = sanitize_safe_details(&raw);
        // sanitize 剔除敏感键后不应再命中
        assert!(scan_json_value(&clean).is_none(), "sanitized={clean}");
    }
}
