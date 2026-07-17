//! Aero Signal 长任务管道：创建、lease、进度、重试、取消与幂等。
//!
//! HTTP 层只创建/查询 job；具体抓取/OCR 由后续模组注册 handler。

mod error;
mod handler;
pub mod kinds;
pub mod progress;
mod registry;
mod retry;
pub mod safe_context;
#[cfg(test)]
mod tests;
mod types;
mod worker;

#[cfg(feature = "postgres")]
pub mod postgres;

pub use error::{JobError, JobResult};
pub use handler::{JobContext, JobHandler, ProgressSender, ProgressUpdate};
pub use kinds::{
    KIND_BUILD_DAILY_PLAN, KIND_COLLECTOR_PROBE, KIND_DAILY_PREFLIGHT, KIND_FETCH_ARTICLE_DETAIL,
    KIND_FETCH_SOURCE, KIND_HISTORY_BACKFILL, KIND_OCR_ARTICLE_IMAGE, KIND_REFETCH_ARTICLE,
    KIND_RETENTION_CLEANUP, KIND_RETRY_ARTICLE_ASSETS, KIND_SOURCE_VALIDATION,
    KIND_SOURCE_VALIDATION_AGGREGATE, SIGNAL_JOB_KINDS,
};
pub use registry::{FakeHandler, JobHandlerRegistry, NotImplementedHandler};
pub use retry::{RetryDecision, RetryPolicy};
pub use safe_context::{SafeContextError, filter_safe_context, validate_safe_context};
pub use types::{CreateJob, Job, JobId, JobKind, JobStatus};
pub use worker::{JobRepository, MemoryRepo, Worker};
