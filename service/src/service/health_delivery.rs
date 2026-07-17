//! HealthDeliveryService：消费端脱敏健康快照（无真实组件 ID / 凭据）。

use std::sync::Arc;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use server_core::AppError;

use crate::domain::health::{
    HealthComponentStatus, HealthComponentType, OverallHealthStatus, ServiceHealthComponent,
    ServiceHealthSnapshot, sanitize_safe_details,
};
use crate::repository::health::HealthSnapshotRepository;

pub struct HealthDeliveryService {
    health: Arc<dyn HealthSnapshotRepository>,
}

/// 消费 API 健康 DTO（版本化；无 proxy/cookie/token/真实敏感 ID）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshotDeliveryDto {
    pub schema_version: u32,
    pub snapshot_date: String,
    pub generated_at: chrono::DateTime<chrono::Utc>,
    pub overall_status: OverallHealthStatus,
    pub yesterday_plan_success_count: i32,
    pub yesterday_plan_failure_count: i32,
    pub yesterday_article_failure_count: i32,
    pub today_eligible_source_count: i32,
    pub today_blocked_source_count: i32,
    pub pending_action_count: i32,
    pub summary: Value,
    pub components: Vec<HealthComponentDeliveryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthComponentDeliveryDto {
    pub component_type: HealthComponentType,
    /// 故意不暴露真实 UUID；仅类型级状态。
    pub status: HealthComponentStatus,
    pub reason_code: Option<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
    pub safe_details: Value,
}

impl HealthDeliveryService {
    pub fn new(health: Arc<dyn HealthSnapshotRepository>) -> Self {
        Self { health }
    }

    pub async fn latest(&self) -> Result<HealthSnapshotDeliveryDto, AppError> {
        let snap = self
            .health
            .get_latest()
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| AppError::not_found("health.snapshot_unavailable", "尚无健康快照"))?;
        self.to_dto(snap).await
    }

    pub async fn by_date(&self, date: NaiveDate) -> Result<HealthSnapshotDeliveryDto, AppError> {
        let snap = self
            .health
            .get_by_date(date)
            .await
            .map_err(AppError::internal)?
            .ok_or_else(|| {
                AppError::not_found("health.snapshot_unavailable", "指定日期无健康快照")
            })?;
        self.to_dto(snap).await
    }

    async fn to_dto(
        &self,
        snap: ServiceHealthSnapshot,
    ) -> Result<HealthSnapshotDeliveryDto, AppError> {
        let comps = self
            .health
            .list_components(snap.id)
            .await
            .map_err(AppError::internal)?;
        Ok(HealthSnapshotDeliveryDto::from_parts(snap, comps))
    }
}

impl HealthSnapshotDeliveryDto {
    pub fn from_parts(snap: ServiceHealthSnapshot, comps: Vec<ServiceHealthComponent>) -> Self {
        Self {
            schema_version: 1,
            snapshot_date: snap.snapshot_date.to_string(),
            generated_at: snap.generated_at,
            overall_status: snap.overall_status,
            yesterday_plan_success_count: snap.yesterday_plan_success_count,
            yesterday_plan_failure_count: snap.yesterday_plan_failure_count,
            yesterday_article_failure_count: snap.yesterday_article_failure_count,
            today_eligible_source_count: snap.today_eligible_source_count,
            today_blocked_source_count: snap.today_blocked_source_count,
            pending_action_count: snap.pending_action_count,
            summary: sanitize_safe_details(&snap.summary_json),
            components: comps
                .into_iter()
                .map(|c| HealthComponentDeliveryDto {
                    component_type: c.component_type,
                    status: c.status,
                    reason_code: c.reason_code,
                    checked_at: c.checked_at,
                    safe_details: sanitize_safe_details(&c.safe_details_json),
                })
                .collect(),
        }
    }

    /// 统计脱敏剔除的敏感键数量（用于调用日志）。
    pub fn redaction_count_estimate(raw_summary: &Value, raw_components: &[Value]) -> i32 {
        fn count_sensitive(v: &Value) -> i32 {
            match v {
                Value::Object(map) => {
                    let mut n = 0;
                    for (k, child) in map {
                        let lower = k.to_ascii_lowercase();
                        if [
                            "cookie",
                            "token",
                            "password",
                            "secret",
                            "proxy",
                            "endpoint",
                            "credential",
                            "authorization",
                            "raw_error",
                        ]
                        .iter()
                        .any(|d| lower.contains(d))
                        {
                            n += 1;
                        }
                        n += count_sensitive(child);
                    }
                    n
                }
                Value::Array(arr) => arr.iter().map(count_sensitive).sum(),
                _ => 0,
            }
        }
        count_sensitive(raw_summary) + raw_components.iter().map(count_sensitive).sum::<i32>()
    }
}
