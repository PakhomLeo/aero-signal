//! PlanEligibility：根据同日健康快照与 source 状态判断今日可执行性。

use chrono::{DateTime, NaiveDate, Utc};
use uuid::Uuid;

use crate::domain::collector::CollectorAccountStatus;
use crate::domain::health::{
    HealthComponentStatus, HealthComponentType, OverallHealthStatus, ServiceHealthComponent,
    ServiceHealthSnapshot,
};
use crate::domain::source::{SourceAccount, SourceLevel, SourceStatus};

/// 可执行候选（已通过资格过滤）。
#[derive(Debug, Clone)]
pub struct EligibleSource {
    pub source_account_id: Uuid,
    pub source_identity: String,
    pub connector: String,
    pub collector_account_id: Uuid,
    pub weight: f64,
    pub reason_code: String,
}

/// 被排除原因。
#[derive(Debug, Clone)]
pub struct IneligibleSource {
    pub source_account_id: Uuid,
    pub reason_code: String,
}

#[derive(Debug, Clone)]
pub struct EligibilityInput<'a> {
    pub plan_date: NaiveDate,
    pub day_end: DateTime<Utc>,
    pub snapshot: Option<&'a ServiceHealthSnapshot>,
    pub components: &'a [ServiceHealthComponent],
    pub sources: &'a [SourceCandidate],
}

/// 参与资格判定的源侧输入。
#[derive(Debug, Clone)]
pub struct SourceCandidate {
    pub account: SourceAccount,
    pub base_level: SourceLevel,
    pub enabled: bool,
    pub next_heartbeat_at: DateTime<Utc>,
    pub source_identity: String,
    pub connector: String,
    pub collector_account_id: Option<Uuid>,
    pub collector_status: Option<CollectorAccountStatus>,
}

#[derive(Debug, Clone)]
pub struct EligibilityResult {
    pub eligible: Vec<EligibleSource>,
    pub ineligible: Vec<IneligibleSource>,
    /// 无快照 / unhealthy 时为 false：不创建可执行计划。
    pub can_build_executable: bool,
    pub block_reason: Option<String>,
}

/// 纯函数资格判定（便于单测）。
pub fn evaluate_eligibility(input: EligibilityInput<'_>) -> EligibilityResult {
    let Some(snap) = input.snapshot else {
        return EligibilityResult {
            eligible: vec![],
            ineligible: input
                .sources
                .iter()
                .map(|s| IneligibleSource {
                    source_account_id: s.account.id,
                    reason_code: "health.snapshot_unavailable".into(),
                })
                .collect(),
            can_build_executable: false,
            block_reason: Some("health.snapshot_unavailable".into()),
        };
    };

    if snap.snapshot_date != input.plan_date {
        return EligibilityResult {
            eligible: vec![],
            ineligible: input
                .sources
                .iter()
                .map(|s| IneligibleSource {
                    source_account_id: s.account.id,
                    reason_code: "health.snapshot_date_mismatch".into(),
                })
                .collect(),
            can_build_executable: false,
            block_reason: Some("health.snapshot_date_mismatch".into()),
        };
    }

    if snap.overall_status == OverallHealthStatus::Unhealthy {
        return EligibilityResult {
            eligible: vec![],
            ineligible: input
                .sources
                .iter()
                .map(|s| IneligibleSource {
                    source_account_id: s.account.id,
                    reason_code: "health.snapshot_unhealthy".into(),
                })
                .collect(),
            can_build_executable: false,
            block_reason: Some("health.snapshot_unhealthy".into()),
        };
    }

    let degraded = snap.overall_status == OverallHealthStatus::Degraded;
    let healthy_connectors = healthy_connector_names(input.components);
    let healthy_collector_ids = healthy_collector_ids(input.components);

    let mut eligible = Vec::new();
    let mut ineligible = Vec::new();

    for s in input.sources {
        if let Some(reason) = reject_source(
            s,
            input.day_end,
            degraded,
            &healthy_connectors,
            &healthy_collector_ids,
        ) {
            ineligible.push(IneligibleSource {
                source_account_id: s.account.id,
                reason_code: reason,
            });
            continue;
        }
        let collector_id = s.collector_account_id.expect("checked");
        eligible.push(EligibleSource {
            source_account_id: s.account.id,
            source_identity: s.source_identity.clone(),
            connector: s.connector.clone(),
            collector_account_id: collector_id,
            weight: level_weight(s.base_level),
            reason_code: "plan.eligible".into(),
        });
    }

    EligibilityResult {
        eligible,
        ineligible,
        can_build_executable: true,
        block_reason: None,
    }
}

fn reject_source(
    s: &SourceCandidate,
    day_end: DateTime<Utc>,
    degraded: bool,
    healthy_connectors: &std::collections::HashSet<String>,
    healthy_collectors: &std::collections::HashSet<Uuid>,
) -> Option<String> {
    if s.account.status == SourceStatus::Paused {
        return Some("plan.excluded_paused".into());
    }
    if s.account.status != SourceStatus::Active {
        return Some("plan.excluded_not_active".into());
    }
    if !s.enabled {
        return Some("plan.excluded_disabled".into());
    }
    if !s.base_level.participates_in_auto_plan() {
        return Some("plan.excluded_l4".into());
    }
    if s.next_heartbeat_at > day_end {
        return Some("plan.excluded_not_due".into());
    }
    // 未验证：无强标识 / 无 identity
    if s.source_identity.trim().is_empty() {
        return Some("plan.excluded_unverified".into());
    }
    let Some(collector_id) = s.collector_account_id else {
        return Some("plan.excluded_no_collector".into());
    };
    if s.collector_status != Some(CollectorAccountStatus::Active) {
        return Some("plan.excluded_collector_unhealthy".into());
    }
    if !healthy_collectors.is_empty() && !healthy_collectors.contains(&collector_id) {
        return Some("plan.excluded_collector_unhealthy".into());
    }
    // degraded：仅健康 connector 侧可执行
    if degraded {
        if healthy_connectors.is_empty() {
            return Some("plan.excluded_connector_unhealthy".into());
        }
        // 组件里 connector 可能按名称记在 safe_details；若集合非空则要求匹配
        let conn_ok = healthy_connectors.contains(&s.connector)
            || healthy_connectors.iter().any(|c| c == "healthy");
        if !conn_ok && !connector_component_healthy_for(&s.connector, healthy_connectors) {
            // 若快照只按 component_id 记录，退化为：有任意 healthy connector 组件且本源 connector 非空
            if !healthy_connectors
                .iter()
                .any(|n| n.is_empty() || n == &s.connector)
            {
                return Some("plan.excluded_connector_unhealthy".into());
            }
        }
    }
    None
}

fn connector_component_healthy_for(
    connector: &str,
    healthy: &std::collections::HashSet<String>,
) -> bool {
    healthy.contains(connector)
}

fn healthy_connector_names(
    components: &[ServiceHealthComponent],
) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    for c in components {
        if c.component_type != HealthComponentType::Connector {
            continue;
        }
        if c.status != HealthComponentStatus::Healthy {
            continue;
        }
        // safe_details 可含 connector 名
        if let Some(name) = c
            .safe_details_json
            .get("connector")
            .and_then(|v| v.as_str())
        {
            set.insert(name.to_string());
        } else {
            // 无名称时插入占位，表示「存在健康 connector 组件」
            set.insert(String::new());
        }
    }
    set
}

fn healthy_collector_ids(components: &[ServiceHealthComponent]) -> std::collections::HashSet<Uuid> {
    components
        .iter()
        .filter(|c| {
            c.component_type == HealthComponentType::Collector
                && c.status == HealthComponentStatus::Healthy
        })
        .filter_map(|c| c.component_id)
        .collect()
}

fn level_weight(level: SourceLevel) -> f64 {
    match level {
        SourceLevel::LO0 => 3.0,
        SourceLevel::LO1 => 2.5,
        SourceLevel::L1 => 2.0,
        SourceLevel::L2 => 1.5,
        SourceLevel::L3 => 1.0,
        SourceLevel::L4 => 0.0,
    }
}

/// Job 开始前二次读取：暂停或 L4 → 应 skipped。
pub fn should_skip_at_runtime(status: SourceStatus, level: SourceLevel) -> Option<&'static str> {
    if status == SourceStatus::Paused {
        return Some("plan.skipped_paused");
    }
    if !level.participates_in_auto_plan() {
        return Some("plan.skipped_l4");
    }
    if status != SourceStatus::Active {
        return Some("plan.skipped_not_active");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;

    fn snap(date: NaiveDate, overall: OverallHealthStatus) -> ServiceHealthSnapshot {
        ServiceHealthSnapshot {
            id: Uuid::new_v4(),
            snapshot_date: date,
            generated_at: Utc::now(),
            overall_status: overall,
            yesterday_plan_success_count: 0,
            yesterday_plan_failure_count: 0,
            yesterday_article_failure_count: 0,
            today_eligible_source_count: 0,
            today_blocked_source_count: 0,
            pending_action_count: 0,
            summary_json: json!({}),
        }
    }

    fn active_source(id: Uuid) -> SourceAccount {
        let now = Utc::now();
        SourceAccount {
            version: 1,
            id,
            display_name: "t".into(),
            status: SourceStatus::Active,
            preferred_connector: Some("fake".into()),
            added_at: now,
            last_fetch_at: None,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn no_snapshot_blocks_plan() {
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let day_end = Utc.with_ymd_and_hms(2023, 11, 18, 15, 59, 59).unwrap();
        let sid = Uuid::new_v4();
        let r = evaluate_eligibility(EligibilityInput {
            plan_date: date,
            day_end,
            snapshot: None,
            components: &[],
            sources: &[SourceCandidate {
                account: active_source(sid),
                base_level: SourceLevel::L1,
                enabled: true,
                next_heartbeat_at: day_end - chrono::Duration::hours(1),
                source_identity: "biz1".into(),
                connector: "fake".into(),
                collector_account_id: Some(Uuid::new_v4()),
                collector_status: Some(CollectorAccountStatus::Active),
            }],
        });
        assert!(!r.can_build_executable);
        assert_eq!(
            r.block_reason.as_deref(),
            Some("health.snapshot_unavailable")
        );
        assert!(r.eligible.is_empty());
    }

    #[test]
    fn unhealthy_blocks_plan() {
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let day_end = Utc.with_ymd_and_hms(2023, 11, 18, 15, 59, 59).unwrap();
        let s = snap(date, OverallHealthStatus::Unhealthy);
        let r = evaluate_eligibility(EligibilityInput {
            plan_date: date,
            day_end,
            snapshot: Some(&s),
            components: &[],
            sources: &[],
        });
        assert!(!r.can_build_executable);
        assert_eq!(r.block_reason.as_deref(), Some("health.snapshot_unhealthy"));
    }

    #[test]
    fn degraded_filters_unhealthy_connector() {
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let day_end = Utc.with_ymd_and_hms(2023, 11, 18, 15, 59, 59).unwrap();
        let s = snap(date, OverallHealthStatus::Degraded);
        let collector = Uuid::new_v4();
        let comps = vec![
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: s.id,
                component_type: HealthComponentType::Connector,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({ "connector": "fake" }),
            },
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: s.id,
                component_type: HealthComponentType::Collector,
                component_id: Some(collector),
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({}),
            },
        ];
        let ok_id = Uuid::new_v4();
        let bad_id = Uuid::new_v4();
        let r = evaluate_eligibility(EligibilityInput {
            plan_date: date,
            day_end,
            snapshot: Some(&s),
            components: &comps,
            sources: &[
                SourceCandidate {
                    account: active_source(ok_id),
                    base_level: SourceLevel::L1,
                    enabled: true,
                    next_heartbeat_at: day_end - chrono::Duration::hours(1),
                    source_identity: "a".into(),
                    connector: "fake".into(),
                    collector_account_id: Some(collector),
                    collector_status: Some(CollectorAccountStatus::Active),
                },
                SourceCandidate {
                    account: active_source(bad_id),
                    base_level: SourceLevel::L1,
                    enabled: true,
                    next_heartbeat_at: day_end - chrono::Duration::hours(1),
                    source_identity: "b".into(),
                    connector: "weread".into(),
                    collector_account_id: Some(collector),
                    collector_status: Some(CollectorAccountStatus::Active),
                },
            ],
        });
        assert!(r.can_build_executable);
        assert_eq!(r.eligible.len(), 1);
        assert_eq!(r.eligible[0].source_account_id, ok_id);
        assert!(
            r.ineligible
                .iter()
                .any(|i| i.reason_code == "plan.excluded_connector_unhealthy")
        );
    }

    #[test]
    fn excludes_paused_and_l4() {
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let day_end = Utc.with_ymd_and_hms(2023, 11, 18, 15, 59, 59).unwrap();
        let s = snap(date, OverallHealthStatus::Healthy);
        let collector = Uuid::new_v4();
        let mut paused = active_source(Uuid::new_v4());
        paused.status = SourceStatus::Paused;
        let r = evaluate_eligibility(EligibilityInput {
            plan_date: date,
            day_end,
            snapshot: Some(&s),
            components: &[],
            sources: &[
                SourceCandidate {
                    account: paused,
                    base_level: SourceLevel::L1,
                    enabled: true,
                    next_heartbeat_at: day_end,
                    source_identity: "p".into(),
                    connector: "fake".into(),
                    collector_account_id: Some(collector),
                    collector_status: Some(CollectorAccountStatus::Active),
                },
                SourceCandidate {
                    account: active_source(Uuid::new_v4()),
                    base_level: SourceLevel::L4,
                    enabled: true,
                    next_heartbeat_at: day_end,
                    source_identity: "l4".into(),
                    connector: "fake".into(),
                    collector_account_id: Some(collector),
                    collector_status: Some(CollectorAccountStatus::Active),
                },
            ],
        });
        assert!(r.eligible.is_empty());
        assert!(
            r.ineligible
                .iter()
                .any(|i| i.reason_code == "plan.excluded_paused")
        );
        assert!(
            r.ineligible
                .iter()
                .any(|i| i.reason_code == "plan.excluded_l4")
        );
    }

    #[test]
    fn runtime_skip_paused_or_l4() {
        assert_eq!(
            should_skip_at_runtime(SourceStatus::Paused, SourceLevel::L1),
            Some("plan.skipped_paused")
        );
        assert_eq!(
            should_skip_at_runtime(SourceStatus::Active, SourceLevel::L4),
            Some("plan.skipped_l4")
        );
        assert_eq!(
            should_skip_at_runtime(SourceStatus::Active, SourceLevel::L1),
            None
        );
    }
}
