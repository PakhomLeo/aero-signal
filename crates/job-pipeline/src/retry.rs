//! RetryPolicy：按错误类型决定 retry / non_retryable / needs_action。

use crate::error::JobError;

/// 重试决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// 可自动重试（进入 retry_wait）。
    Retry,
    /// 不可重试，标记 failed。
    NonRetryable,
    /// 需要人工介入（needs_action）。
    NeedsAction,
}

/// 默认重试策略。
#[derive(Debug, Clone, Default)]
pub struct RetryPolicy {
    /// 基础退避秒数（指数退避基数）。
    pub base_backoff_secs: u64,
    /// 最大退避秒数。
    pub max_backoff_secs: u64,
}

impl RetryPolicy {
    pub fn new() -> Self {
        Self {
            base_backoff_secs: 10,
            max_backoff_secs: 600,
        }
    }

    /// 根据错误与当前 attempt 决定下一步。
    pub fn decide(&self, err: &JobError, attempt_count: u32, max_attempts: u32) -> RetryDecision {
        if err.needs_action {
            return RetryDecision::NeedsAction;
        }
        if !err.retryable {
            return RetryDecision::NonRetryable;
        }
        if attempt_count >= max_attempts {
            // 可重试但次数耗尽 → 进入 needs_action，便于人工重放。
            return RetryDecision::NeedsAction;
        }
        RetryDecision::Retry
    }

    /// 计算下一次运行时间的退避秒数（指数退避）。
    pub fn backoff_secs(&self, attempt_count: u32) -> u64 {
        let exp = attempt_count.saturating_sub(1).min(10);
        let secs = self.base_backoff_secs.saturating_mul(1u64 << exp);
        secs.min(self.max_backoff_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::JobId;

    #[test]
    fn retryable_within_limit() {
        let policy = RetryPolicy::new();
        let err = JobError::retryable("egress.network_unreachable", "timeout");
        assert_eq!(policy.decide(&err, 1, 3), RetryDecision::Retry);
    }

    #[test]
    fn retryable_exhausted_becomes_needs_action() {
        let policy = RetryPolicy::new();
        let err = JobError::retryable("egress.network_unreachable", "timeout");
        assert_eq!(policy.decide(&err, 3, 3), RetryDecision::NeedsAction);
    }

    #[test]
    fn permanent_is_non_retryable() {
        let policy = RetryPolicy::new();
        let err = JobError::permanent("connector.unsupported", "no");
        assert_eq!(policy.decide(&err, 1, 3), RetryDecision::NonRetryable);
    }

    #[test]
    fn action_required() {
        let policy = RetryPolicy::new();
        let err = JobError::action_required(JobId::new(), "collector.needs_relogin", "login");
        assert_eq!(policy.decide(&err, 1, 3), RetryDecision::NeedsAction);
    }

    #[test]
    fn backoff_grows_then_caps() {
        let policy = RetryPolicy {
            base_backoff_secs: 10,
            max_backoff_secs: 100,
        };
        assert_eq!(policy.backoff_secs(1), 10);
        assert_eq!(policy.backoff_secs(2), 20);
        assert_eq!(policy.backoff_secs(5), 100);
    }
}
