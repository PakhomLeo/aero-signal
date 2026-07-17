//! Job 错误类型（`module.reason` 约定）。

use thiserror::Error;

use crate::types::JobId;

/// 带 `module.reason` 码的任务错误。
#[derive(Debug, Clone, Error)]
#[error("{code}: {message}")]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    /// 为 true 时表示需要人工介入（needs_action），不可自动重试。
    pub needs_action: bool,
    pub job_id: Option<JobId>,
}

impl JobError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
            needs_action: false,
            job_id: None,
        }
    }

    pub fn with_job_id(mut self, job_id: JobId) -> Self {
        self.job_id = Some(job_id);
        self
    }

    pub fn retryable(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, true)
    }

    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message, false)
    }

    pub fn action_required(
        job_id: JobId,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            needs_action: true,
            job_id: Some(job_id),
        }
    }

    pub fn needs_action(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            needs_action: true,
            job_id: None,
        }
    }
}

pub type JobResult<T> = Result<T, JobError>;
