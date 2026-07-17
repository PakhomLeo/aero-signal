//! 保留清理服务：删除前引用检查（M15 骨架）。

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use server_core::Clock;

use crate::audit::{ACTOR_WORKER, AuditWriter};
use crate::domain::retention::{
    RetentionCandidate, RetentionDeleteDecision, RetentionPolicy, RetentionTargetKind,
};
use crate::repository::retention::RetentionRepository;

#[derive(Debug, Clone, Default)]
pub struct RetentionRunSummary {
    pub scanned: u64,
    pub deleted: u64,
    pub skipped_referenced: u64,
    pub skipped_disabled: u64,
}

/// 清理 job 骨架：按策略扫描过期项，引用守卫通过后才 hard_delete。
pub struct RetentionCleanupService {
    repo: Arc<dyn RetentionRepository>,
    clock: Arc<dyn Clock>,
    audit: Option<AuditWriter>,
}

impl RetentionCleanupService {
    pub fn new(repo: Arc<dyn RetentionRepository>, clock: Arc<dyn Clock>) -> Self {
        Self {
            repo,
            clock,
            audit: None,
        }
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    pub async fn list_policies(&self) -> Result<Vec<RetentionPolicy>, String> {
        self.repo.list_policies().await
    }

    /// 对单个候选做引用检查。
    pub async fn decide_delete(
        &self,
        candidate: &RetentionCandidate,
    ) -> Result<RetentionDeleteDecision, String> {
        match candidate.target_kind {
            RetentionTargetKind::RawHtml
            | RetentionTargetKind::Screenshot
            | RetentionTargetKind::BodyImage
            | RetentionTargetKind::OcrText
            | RetentionTargetKind::FailedValidationAsset => {
                if self.repo.is_asset_referenced(candidate.record_id).await? {
                    return Ok(RetentionDeleteDecision::Blocked {
                        reason: "asset_still_referenced",
                    });
                }
            }
            RetentionTargetKind::DeliveryOutbox => {
                if self.repo.is_outbox_undelivered(candidate.record_id).await? {
                    return Ok(RetentionDeleteDecision::Blocked {
                        reason: "outbox_undelivered",
                    });
                }
            }
            // 日志/事件类：无资产引用守卫；仍禁止删未投递事实（outbox 已覆盖）
            RetentionTargetKind::ApiCallLog
            | RetentionTargetKind::AuditLog
            | RetentionTargetKind::ChangeEvent => {}
        }
        Ok(RetentionDeleteDecision::Allowed)
    }

    /// 执行一轮清理（可按 kind 过滤；None = 全部启用策略）。
    pub async fn run_once(
        &self,
        only_kind: Option<RetentionTargetKind>,
        batch_limit: usize,
    ) -> Result<RetentionRunSummary, String> {
        let now = system_time_to_utc(self.clock.now());
        let policies = self.repo.list_policies().await?;
        let mut summary = RetentionRunSummary::default();

        for policy in policies {
            if let Some(k) = only_kind
                && policy.target_kind != k
            {
                continue;
            }
            if !policy.enabled {
                summary.skipped_disabled += 1;
                continue;
            }
            let cutoff = now - Duration::days(policy.retention_days as i64);
            let candidates = self
                .repo
                .list_expired_candidates(policy.target_kind, cutoff, batch_limit)
                .await?;
            for c in candidates {
                summary.scanned += 1;
                match self.decide_delete(&c).await? {
                    RetentionDeleteDecision::Allowed => {
                        if self.repo.hard_delete(c.target_kind, c.record_id).await? {
                            summary.deleted += 1;
                        }
                    }
                    RetentionDeleteDecision::Blocked { .. } => {
                        summary.skipped_referenced += 1;
                    }
                }
            }
        }

        if let Some(ref audit) = self.audit {
            let _ = audit
                .success(
                    ACTOR_WORKER,
                    Some("retention_cleanup"),
                    "retention.cleanup_run",
                    Some("retention_policies"),
                    None,
                    json!({
                        "scanned": summary.scanned,
                        "deleted": summary.deleted,
                        "skipped_referenced": summary.skipped_referenced,
                    }),
                    now,
                )
                .await;
        }

        Ok(summary)
    }
}

fn system_time_to_utc(t: std::time::SystemTime) -> chrono::DateTime<Utc> {
    let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    DateTime::<Utc>::from_timestamp(dur.as_secs() as i64, dur.subsec_nanos())
        .unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::retention::RetentionCandidate;
    use crate::repository::retention_memory::MemoryRetentionStore;
    use server_core::FrozenClock;
    use uuid::Uuid;

    fn frozen_now() -> Arc<dyn Clock> {
        // 固定毫秒时间戳，便于 Clock 注入测试
        Arc::new(FrozenClock::at_unix_millis(1_783_603_200_000)) as Arc<dyn Clock>
    }

    #[tokio::test]
    async fn skips_referenced_asset() {
        let store = Arc::new(MemoryRetentionStore::new());
        let now = system_time_to_utc(frozen_now().now());
        store.seed_defaults(now).await;
        let asset_id = Uuid::new_v4();
        store
            .seed_candidate(RetentionCandidate {
                target_kind: RetentionTargetKind::RawHtml,
                record_id: asset_id,
                article_id: Some(Uuid::new_v4()),
                created_at: now - Duration::days(60),
            })
            .await;
        store.mark_asset_referenced(asset_id, true).await;

        let svc = RetentionCleanupService::new(store.clone(), frozen_now());
        let summary = svc
            .run_once(Some(RetentionTargetKind::RawHtml), 100)
            .await
            .unwrap();
        assert_eq!(summary.scanned, 1);
        assert_eq!(summary.deleted, 0);
        assert_eq!(summary.skipped_referenced, 1);
        assert!(store.deleted().await.is_empty());
    }

    #[tokio::test]
    async fn deletes_unreferenced_expired_asset() {
        let store = Arc::new(MemoryRetentionStore::new());
        let now = system_time_to_utc(frozen_now().now());
        store.seed_defaults(now).await;
        let asset_id = Uuid::new_v4();
        store
            .seed_candidate(RetentionCandidate {
                target_kind: RetentionTargetKind::Screenshot,
                record_id: asset_id,
                article_id: Some(Uuid::new_v4()),
                created_at: now - Duration::days(60),
            })
            .await;
        store.mark_asset_referenced(asset_id, false).await;

        let svc = RetentionCleanupService::new(store.clone(), frozen_now());
        let summary = svc
            .run_once(Some(RetentionTargetKind::Screenshot), 100)
            .await
            .unwrap();
        assert_eq!(summary.deleted, 1);
        assert_eq!(store.deleted().await.len(), 1);
    }

    #[tokio::test]
    async fn blocks_undelivered_outbox() {
        let store = Arc::new(MemoryRetentionStore::new());
        let now = system_time_to_utc(frozen_now().now());
        store
            .seed_policy(RetentionPolicy {
                id: Uuid::new_v4(),
                target_kind: RetentionTargetKind::DeliveryOutbox,
                retention_days: 7,
                hard_delete_after_days: Some(30),
                enabled: true,
                updated_at: now,
            })
            .await;
        let oid = Uuid::new_v4();
        store
            .seed_candidate(RetentionCandidate {
                target_kind: RetentionTargetKind::DeliveryOutbox,
                record_id: oid,
                article_id: Some(Uuid::new_v4()),
                created_at: now - Duration::days(40),
            })
            .await;
        store.mark_outbox_undelivered(oid, true).await;

        let svc = RetentionCleanupService::new(store.clone(), frozen_now());
        let summary = svc
            .run_once(Some(RetentionTargetKind::DeliveryOutbox), 100)
            .await
            .unwrap();
        assert_eq!(summary.skipped_referenced, 1);
        assert_eq!(summary.deleted, 0);
    }
}
