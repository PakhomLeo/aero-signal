//! 内存健康快照仓储。

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::domain::health::{HealthCheckRun, ServiceHealthComponent, ServiceHealthSnapshot};
use crate::repository::health::HealthSnapshotRepository;

#[derive(Default)]
struct Inner {
    snapshots: Vec<ServiceHealthSnapshot>,
    components: Vec<ServiceHealthComponent>,
    check_runs: Vec<HealthCheckRun>,
}

#[derive(Clone, Default)]
pub struct MemoryHealthStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemoryHealthStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HealthSnapshotRepository for MemoryHealthStore {
    async fn get_by_date(&self, date: NaiveDate) -> Result<Option<ServiceHealthSnapshot>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .snapshots
            .iter()
            .find(|s| s.snapshot_date == date)
            .cloned())
    }

    async fn get_latest(&self) -> Result<Option<ServiceHealthSnapshot>, String> {
        let g = self.inner.lock().await;
        Ok(g.snapshots.iter().max_by_key(|s| s.snapshot_date).cloned())
    }

    async fn list_components(
        &self,
        snapshot_id: Uuid,
    ) -> Result<Vec<ServiceHealthComponent>, String> {
        Ok(self
            .inner
            .lock()
            .await
            .components
            .iter()
            .filter(|c| c.snapshot_id == snapshot_id)
            .cloned()
            .collect())
    }

    async fn insert_snapshot(
        &self,
        snapshot: &ServiceHealthSnapshot,
        components: &[ServiceHealthComponent],
    ) -> Result<(), String> {
        let mut g = self.inner.lock().await;
        if g.snapshots
            .iter()
            .any(|s| s.snapshot_date == snapshot.snapshot_date)
        {
            return Err(format!(
                "health.snapshot_exists: 日期 {} 已有快照，禁止覆盖",
                snapshot.snapshot_date
            ));
        }
        if g.snapshots.iter().any(|s| s.id == snapshot.id) {
            return Err("health.snapshot_id_exists: 快照 id 已存在".into());
        }
        g.snapshots.push(snapshot.clone());
        g.components.extend(components.iter().cloned());
        Ok(())
    }

    async fn insert_check_run(&self, run: &HealthCheckRun) -> Result<(), String> {
        self.inner.lock().await.check_runs.push(run.clone());
        Ok(())
    }

    async fn list_check_runs(&self, limit: usize) -> Result<Vec<HealthCheckRun>, String> {
        let g = self.inner.lock().await;
        let mut runs = g.check_runs.clone();
        runs.sort_by_key(|run| std::cmp::Reverse(run.checked_at));
        runs.truncate(limit);
        Ok(runs)
    }
}
