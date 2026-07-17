//! 探测结果记录器：写入内存快照，供健康组件消费。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::ProbeResult;

/// 单次探测记录（已脱敏）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeRecord {
    pub account_id: Uuid,
    pub connector: String,
    pub result: ProbeResult,
    pub recorded_at: DateTime<Utc>,
}

/// 将 capability probe 结果写入 collector 状态视图。
pub trait ProbeRecorder: Send + Sync {
    fn record(&self, account_id: Uuid, connector: &str, result: ProbeResult, now: DateTime<Utc>);
    fn latest(&self, account_id: Uuid) -> Option<ProbeRecord>;
    fn list_by_connector(&self, connector: &str) -> Vec<ProbeRecord>;
}

/// 内存实现（测试与早期健康组件）。
#[derive(Default, Clone)]
pub struct MemoryProbeRecorder {
    inner: Arc<Mutex<BTreeMap<Uuid, ProbeRecord>>>,
}

impl MemoryProbeRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn all(&self) -> Vec<ProbeRecord> {
        let guard = self.inner.lock().expect("probe recorder lock");
        guard.values().cloned().collect()
    }
}

impl ProbeRecorder for MemoryProbeRecorder {
    fn record(&self, account_id: Uuid, connector: &str, result: ProbeResult, now: DateTime<Utc>) {
        let mut guard = self.inner.lock().expect("probe recorder lock");
        guard.insert(
            account_id,
            ProbeRecord {
                account_id,
                connector: connector.to_string(),
                result,
                recorded_at: now,
            },
        );
    }

    fn latest(&self, account_id: Uuid) -> Option<ProbeRecord> {
        let guard = self.inner.lock().expect("probe recorder lock");
        guard.get(&account_id).cloned()
    }

    fn list_by_connector(&self, connector: &str) -> Vec<ProbeRecord> {
        let guard = self.inner.lock().expect("probe recorder lock");
        guard
            .values()
            .filter(|r| r.connector == connector)
            .cloned()
            .collect()
    }
}
