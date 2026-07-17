use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFailureCategory {
    CredentialsInvalid,
    RiskControl,
    PlatformRestriction,
    RateLimited,
    NetworkTemporary,
    ContentWithdrawn,
    ContentInvalid,
    StorageTemporary,
    StoragePermanent,
    Configuration,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountEffect {
    None,
    Cooldown,
    NeedsRelogin,
    NeedsAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEffect {
    None,
    TechnicalFailure,
    AccessBlocked,
    ContentUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFailure {
    pub code: String,
    pub category: CaptureFailureCategory,
    pub retryable: bool,
    pub account_effect: AccountEffect,
    pub source_effect: SourceEffect,
    pub retry_after_ms: Option<u64>,
    pub message: String,
    pub safe_context: BTreeMap<String, String>,
}

impl CaptureFailure {
    pub fn from_safe_message(message: impl Into<String>) -> Self {
        let message = message.into();
        let code = message
            .split_once(':')
            .map_or(message.as_str(), |(code, _)| code)
            .trim()
            .to_string();
        Self::from_code_message(code, message)
    }

    pub fn from_code_message(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let message = message.into();
        let (category, retryable, account_effect, source_effect) = classify(&code);
        let retry_after_ms = parse_retry_after_ms(&message);
        Self {
            code,
            category,
            retryable,
            account_effect,
            source_effect,
            retry_after_ms,
            message,
            safe_context: BTreeMap::new(),
        }
    }
}

impl std::fmt::Display for CaptureFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CaptureFailure {}

impl From<String> for CaptureFailure {
    fn from(value: String) -> Self {
        Self::from_safe_message(value)
    }
}

impl From<&str> for CaptureFailure {
    fn from(value: &str) -> Self {
        Self::from_safe_message(value)
    }
}

fn classify(code: &str) -> (CaptureFailureCategory, bool, AccountEffect, SourceEffect) {
    use AccountEffect as Account;
    use CaptureFailureCategory as Category;
    use SourceEffect as Source;

    match code {
        "collector.needs_relogin" | "connector.credentials_invalid" => (
            Category::CredentialsInvalid,
            false,
            Account::NeedsRelogin,
            Source::AccessBlocked,
        ),
        "crawler.blocked_by_protection" | "crawler.login_challenge" => (
            Category::RiskControl,
            false,
            Account::NeedsAction,
            Source::AccessBlocked,
        ),
        "connector.platform_restricted" => (
            Category::PlatformRestriction,
            false,
            Account::NeedsAction,
            Source::AccessBlocked,
        ),
        "crawler.rate_limited" | "crawler.local_rate_limited" => (
            Category::RateLimited,
            true,
            Account::Cooldown,
            Source::AccessBlocked,
        ),
        "crawler.fetch_failed"
        | "crawler.http_failed"
        | "crawler.http_timeout"
        | "crawler.http_body_failed"
        | "egress.network_unreachable"
        | "egress.timeout" => (
            Category::NetworkTemporary,
            true,
            Account::None,
            Source::TechnicalFailure,
        ),
        "crawler.content_withdrawn" => (
            Category::ContentWithdrawn,
            false,
            Account::None,
            Source::ContentUnavailable,
        ),
        "crawler.empty_article" | "crawler.content_invalid" => (
            Category::ContentInvalid,
            false,
            Account::None,
            Source::ContentUnavailable,
        ),
        "crawler.store_failed" | "storage.temporary" => (
            Category::StorageTemporary,
            true,
            Account::None,
            Source::TechnicalFailure,
        ),
        "storage.permanent" => (
            Category::StoragePermanent,
            false,
            Account::None,
            Source::TechnicalFailure,
        ),
        "job.cancelled" | "job_pipeline.cancelled" | "crawler.cancelled" => {
            (Category::Cancelled, false, Account::None, Source::None)
        }
        value
            if value.starts_with("connector.")
                || value.starts_with("crawler.url_")
                || value.starts_with("crawler.proxy_") =>
        {
            (
                Category::Configuration,
                false,
                Account::NeedsAction,
                Source::TechnicalFailure,
            )
        }
        _ => (
            Category::Unknown,
            false,
            Account::None,
            Source::TechnicalFailure,
        ),
    }
}

fn parse_retry_after_ms(message: &str) -> Option<u64> {
    let marker = "retry_after_ms=";
    let start = message.find(marker)? + marker.len();
    let digits: String = message[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_without_message_substring_matching() {
        let failure =
            CaptureFailure::from_safe_message("crawler.local_rate_limited: retry_after_ms=750");
        assert_eq!(failure.category, CaptureFailureCategory::RateLimited);
        assert!(failure.retryable);
        assert_eq!(failure.account_effect, AccountEffect::Cooldown);
        assert_eq!(failure.retry_after_ms, Some(750));
    }

    #[test]
    fn credential_failure_has_distinct_effects() {
        let failure = CaptureFailure::from_code_message(
            "collector.needs_relogin",
            "collector session expired",
        );
        assert_eq!(failure.category, CaptureFailureCategory::CredentialsInvalid);
        assert_eq!(failure.account_effect, AccountEffect::NeedsRelogin);
        assert_eq!(failure.source_effect, SourceEffect::AccessBlocked);
    }

    #[test]
    fn required_categories_have_explicit_retry_and_effect_policy() {
        let cases = [
            (
                "connector.platform_restricted",
                CaptureFailureCategory::PlatformRestriction,
                false,
            ),
            (
                "crawler.blocked_by_protection",
                CaptureFailureCategory::RiskControl,
                false,
            ),
            (
                "crawler.http_timeout",
                CaptureFailureCategory::NetworkTemporary,
                true,
            ),
            (
                "crawler.content_withdrawn",
                CaptureFailureCategory::ContentWithdrawn,
                false,
            ),
            (
                "crawler.content_invalid",
                CaptureFailureCategory::ContentInvalid,
                false,
            ),
            (
                "crawler.store_failed",
                CaptureFailureCategory::StorageTemporary,
                true,
            ),
            (
                "storage.permanent",
                CaptureFailureCategory::StoragePermanent,
                false,
            ),
            (
                "connector.unsupported",
                CaptureFailureCategory::Configuration,
                false,
            ),
            (
                "crawler.cancelled",
                CaptureFailureCategory::Cancelled,
                false,
            ),
        ];
        for (code, category, retryable) in cases {
            let failure = CaptureFailure::from_code_message(code, "safe message");
            assert_eq!(failure.category, category, "{code}");
            assert_eq!(failure.retryable, retryable, "{code}");
        }
    }
}
