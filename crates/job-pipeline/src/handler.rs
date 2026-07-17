//! Job handler 契约。

use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::error::JobResult;
use crate::types::JobId;

/// Handler 执行期间可用的上下文。
#[derive(Debug, Clone)]
pub struct JobContext {
    pub worker_id: String,
    pub payload_json: serde_json::Value,
    /// 进度更新通道；worker 负责持久化。
    pub progress_tx: ProgressSender,
    /// Worker 轮询持久化取消状态并设置该标记；长任务应在阶段边界检查。
    pub cancel_requested: Arc<AtomicBool>,
}

/// 有界进度发送器。满载时丢弃当前进度，避免业务 handler 因进度洪峰阻塞或无限增长。
#[derive(Clone, Debug)]
pub struct ProgressSender(tokio::sync::mpsc::Sender<ProgressUpdate>);

impl ProgressSender {
    pub fn channel(capacity: usize) -> (Self, tokio::sync::mpsc::Receiver<ProgressUpdate>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity.max(1));
        (Self(sender), receiver)
    }

    pub fn send(
        &self,
        update: ProgressUpdate,
    ) -> Result<(), tokio::sync::mpsc::error::TrySendError<ProgressUpdate>> {
        self.0.try_send(update)
    }
}

impl JobContext {
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_requested.load(Ordering::SeqCst)
    }
}

/// Handler → Worker 的进度/结果更新。
#[derive(Debug, Clone)]
pub enum ProgressUpdate {
    Progress {
        percent: f64,
        message: String,
        stage: String,
    },
    Stage {
        stage: String,
    },
    Message {
        message: String,
    },
    Result(serde_json::Value),
}

/// 各业务模组实现的长任务处理器。
#[async_trait]
pub trait JobHandler: Send + Sync {
    /// 本 handler 处理的 kind（如 `daily_preflight`）。
    fn kind(&self) -> &str;

    /// 执行任务。
    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value>;

    /// 取消回调（可选覆盖）。
    async fn cancel(&self, job_id: JobId, ctx: &JobContext) -> JobResult<()> {
        let _ = (job_id, ctx);
        Ok(())
    }
}
