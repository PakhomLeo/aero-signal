//! 内存保留策略仓储（测试用）。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::retention::{RetentionCandidate, RetentionPolicy, RetentionTargetKind};
use crate::repository::retention::RetentionRepository;

#[derive(Default)]
struct State {
    policies: HashMap<RetentionTargetKind, RetentionPolicy>,
    candidates: Vec<RetentionCandidate>,
    referenced_assets: HashMap<Uuid, bool>,
    undelivered_outbox: HashMap<Uuid, bool>,
    deleted: Vec<(RetentionTargetKind, Uuid)>,
}

#[derive(Default, Clone)]
pub struct MemoryRetentionStore {
    inner: Arc<Mutex<State>>,
}

impl MemoryRetentionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn seed_defaults(&self, now: DateTime<Utc>) {
        for p in default_policies(now) {
            self.inner.lock().await.policies.insert(p.target_kind, p);
        }
    }

    pub async fn seed_policy(&self, policy: RetentionPolicy) {
        self.inner
            .lock()
            .await
            .policies
            .insert(policy.target_kind, policy);
    }

    pub async fn seed_candidate(&self, c: RetentionCandidate) {
        self.inner.lock().await.candidates.push(c);
    }

    pub async fn mark_asset_referenced(&self, asset_id: Uuid, referenced: bool) {
        self.inner
            .lock()
            .await
            .referenced_assets
            .insert(asset_id, referenced);
    }

    pub async fn mark_outbox_undelivered(&self, outbox_id: Uuid, undelivered: bool) {
        self.inner
            .lock()
            .await
            .undelivered_outbox
            .insert(outbox_id, undelivered);
    }

    pub async fn deleted(&self) -> Vec<(RetentionTargetKind, Uuid)> {
        self.inner.lock().await.deleted.clone()
    }
}

pub fn default_policies(now: DateTime<Utc>) -> Vec<RetentionPolicy> {
    RetentionTargetKind::ALL
        .iter()
        .enumerate()
        .map(|(i, kind)| {
            let days = match kind {
                RetentionTargetKind::RawHtml | RetentionTargetKind::Screenshot => 30,
                RetentionTargetKind::FailedValidationAsset => 14,
                RetentionTargetKind::DeliveryOutbox | RetentionTargetKind::ApiCallLog => 90,
                RetentionTargetKind::AuditLog | RetentionTargetKind::ChangeEvent => 365,
                _ => 180,
            };
            RetentionPolicy {
                id: Uuid::from_u128(0xa000_0000_0000_4000_8000_0000_0000_0001 + i as u128),
                target_kind: *kind,
                retention_days: days,
                hard_delete_after_days: None,
                enabled: true,
                updated_at: now,
            }
        })
        .collect()
}

#[async_trait]
impl RetentionRepository for MemoryRetentionStore {
    async fn list_policies(&self) -> Result<Vec<RetentionPolicy>, String> {
        let g = self.inner.lock().await;
        Ok(g.policies.values().cloned().collect())
    }

    async fn upsert_policy(&self, policy: &RetentionPolicy) -> Result<(), String> {
        self.inner
            .lock()
            .await
            .policies
            .insert(policy.target_kind, policy.clone());
        Ok(())
    }

    async fn list_expired_candidates(
        &self,
        kind: RetentionTargetKind,
        older_than: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<RetentionCandidate>, String> {
        let g = self.inner.lock().await;
        let mut out: Vec<_> = g
            .candidates
            .iter()
            .filter(|c| c.target_kind == kind && c.created_at < older_than)
            .cloned()
            .collect();
        out.truncate(limit);
        Ok(out)
    }

    async fn hard_delete(&self, kind: RetentionTargetKind, id: Uuid) -> Result<bool, String> {
        let mut g = self.inner.lock().await;
        let before = g.candidates.len();
        g.candidates
            .retain(|c| !(c.target_kind == kind && c.record_id == id));
        let deleted = g.candidates.len() < before;
        if deleted {
            g.deleted.push((kind, id));
        }
        Ok(deleted)
    }

    async fn is_asset_referenced(&self, asset_id: Uuid) -> Result<bool, String> {
        Ok(self
            .inner
            .lock()
            .await
            .referenced_assets
            .get(&asset_id)
            .copied()
            .unwrap_or(false))
    }

    async fn is_outbox_undelivered(&self, outbox_id: Uuid) -> Result<bool, String> {
        Ok(self
            .inner
            .lock()
            .await
            .undelivered_outbox
            .get(&outbox_id)
            .copied()
            .unwrap_or(false))
    }
}
