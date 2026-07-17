//! JobHandlerRegistry：按 kind 注册与查找 handler。

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::{JobError, JobResult};
use crate::handler::{JobContext, JobHandler};
use crate::kinds::SIGNAL_JOB_KINDS;
use crate::types::JobId;
use async_trait::async_trait;

/// 按 kind 索引的 handler 注册表。
#[derive(Default)]
pub struct JobHandlerRegistry {
    handlers: HashMap<String, Arc<dyn JobHandler>>,
}

impl JobHandlerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn JobHandler>) {
        self.handlers.insert(handler.kind().to_string(), handler);
    }

    pub fn get(&self, kind: &str) -> Option<Arc<dyn JobHandler>> {
        self.handlers.get(kind).cloned()
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.handlers.contains_key(kind)
    }

    pub fn kinds(&self) -> Vec<String> {
        let mut keys: Vec<_> = self.handlers.keys().cloned().collect();
        keys.sort();
        keys
    }

    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }

    /// 为 Signal 全部预定义 kind 注册占位 NotImplemented handler。
    pub fn with_signal_placeholders() -> Self {
        let mut reg = Self::new();
        for kind in SIGNAL_JOB_KINDS {
            reg.register(Arc::new(NotImplementedHandler {
                kind: (*kind).to_string(),
            }));
        }
        reg
    }

    /// 校验 SIGNAL_JOB_KINDS 是否全部已注册。
    pub fn assert_signal_kinds_registered(&self) -> JobResult<()> {
        for kind in SIGNAL_JOB_KINDS {
            if !self.contains(kind) {
                return Err(JobError::permanent(
                    "job_pipeline.handler_missing",
                    format!("缺少 handler: {kind}"),
                ));
            }
        }
        Ok(())
    }
}

/// 占位 handler：业务模组接入前返回 NotImplemented。
pub struct NotImplementedHandler {
    pub kind: String,
}

#[async_trait]
impl JobHandler for NotImplementedHandler {
    fn kind(&self) -> &str {
        &self.kind
    }

    async fn execute(&self, _job_id: JobId, _ctx: JobContext) -> JobResult<serde_json::Value> {
        Err(JobError::needs_action(
            "job_pipeline.not_implemented",
            format!("handler `{}` 尚未实现", self.kind),
        ))
    }
}

/// 测试用 Fake handler：按 payload.mode 走成功/失败/取消路径。
pub struct FakeHandler {
    pub kind: String,
}

#[async_trait]
impl JobHandler for FakeHandler {
    fn kind(&self) -> &str {
        &self.kind
    }

    async fn execute(&self, job_id: JobId, ctx: JobContext) -> JobResult<serde_json::Value> {
        let mode = ctx
            .payload_json
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("success");

        match mode {
            "success" => {
                let _ = ctx
                    .progress_tx
                    .send(crate::handler::ProgressUpdate::Progress {
                        percent: 100.0,
                        message: "ok".into(),
                        stage: "complete".into(),
                    });
                Ok(serde_json::json!({
                    "ok": true,
                    "job_id": job_id.to_string(),
                    "kind": self.kind,
                }))
            }
            "fail" => Err(JobError::permanent(
                "job_pipeline.fake_fail",
                "fake handler forced failure",
            )),
            "retry" => Err(JobError::retryable(
                "job_pipeline.fake_retry",
                "fake handler forced retry",
            )),
            "needs_action" => Err(JobError::needs_action(
                "job_pipeline.fake_needs_action",
                "fake handler needs action",
            )),
            "cancel" => Err(JobError::permanent(
                "job_pipeline.fake_cancelled",
                "fake handler cancelled path",
            )),
            other => Err(JobError::permanent(
                "job_pipeline.fake_unknown_mode",
                format!("unknown fake mode: {other}"),
            )),
        }
    }

    async fn cancel(&self, _job_id: JobId, _ctx: &JobContext) -> JobResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_placeholders_cover_all_kinds() {
        let reg = JobHandlerRegistry::with_signal_placeholders();
        assert!(reg.assert_signal_kinds_registered().is_ok());
        assert_eq!(reg.len(), SIGNAL_JOB_KINDS.len());
        for kind in SIGNAL_JOB_KINDS {
            assert!(reg.contains(kind), "missing {kind}");
        }
    }
}
