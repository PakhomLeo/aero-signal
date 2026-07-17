//! DailyPlanService：08:00 非均匀日计划 + 创建 fetch_source jobs。

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use job_pipeline::kinds::idempotency;
use job_pipeline::{
    CreateJob, Job, JobId, JobRepository, KIND_BUILD_DAILY_PLAN, KIND_FETCH_SOURCE,
};
use platform_connectors::ConnectorCapability;
use serde::{Deserialize, Serialize};
use serde_json::json;
use server_core::Clock;
use uuid::Uuid;

use crate::audit::{ACTOR_SYSTEM, ACTOR_WORKER, AuditWriter};
use crate::domain::daily_plan::{DailyFetchPlan, DailyPlanStatus};
use crate::domain::health::OverallHealthStatus;
use crate::repository::collector::CollectorRepository;
use crate::repository::daily_plan::DailyPlanRepository;
use crate::repository::health::HealthSnapshotRepository;
use crate::repository::settings::SettingsRepository;
use crate::repository::source::SourceRepository;
use crate::service::connector_context::CollectorConnectorResolver;
use crate::service::daily_plan_mapping::{
    day_end_utc, effective_level, in_plan_start_window, plan_to_dto,
};
use crate::service::daily_preflight::business_date_for;
use crate::service::plan_eligibility::{
    EligibilityInput, SourceCandidate, evaluate_eligibility, should_skip_at_runtime,
};
use crate::service::slot_allocator::{SlotAllocator, SlotAllocatorConfig, SlotRequest};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildDailyPlanPayload {
    /// YYYY-MM-DD
    pub plan_date: String,
    /// 强制重建 planned 行。
    #[serde(default)]
    pub rebuild: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyPlanRowDto {
    pub id: Uuid,
    pub plan_date: String,
    pub source_account_id: Uuid,
    pub planned_run_at: DateTime<Utc>,
    pub status: String,
    pub reason_code: String,
    pub fetch_job_id: Option<Uuid>,
    pub collector_account_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyPlanListDto {
    pub plan_date: String,
    pub snapshot_id: Option<Uuid>,
    pub overall_status: Option<String>,
    pub can_build_executable: bool,
    pub block_reason: Option<String>,
    pub rows: Vec<DailyPlanRowDto>,
    pub eligible_count: usize,
    pub planned_count: usize,
    pub started_count: usize,
}

pub struct DailyPlanService {
    plans: Arc<dyn DailyPlanRepository>,
    health: Arc<dyn HealthSnapshotRepository>,
    sources: Arc<dyn SourceRepository>,
    collectors: Arc<dyn CollectorRepository>,
    settings: Arc<dyn SettingsRepository>,
    jobs: Arc<dyn JobRepository>,
    clock: Arc<dyn Clock>,
    slot_config: SlotAllocatorConfig,
    /// 可注入种子以便测试可复现。
    rng_seed: Option<u64>,
    audit: Option<AuditWriter>,
    connector_resolver: Option<Arc<CollectorConnectorResolver>>,
}

impl DailyPlanService {
    pub fn new(
        plans: Arc<dyn DailyPlanRepository>,
        health: Arc<dyn HealthSnapshotRepository>,
        sources: Arc<dyn SourceRepository>,
        collectors: Arc<dyn CollectorRepository>,
        settings: Arc<dyn SettingsRepository>,
        jobs: Arc<dyn JobRepository>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            plans,
            health,
            sources,
            collectors,
            settings,
            jobs,
            clock,
            slot_config: SlotAllocatorConfig::default(),
            rng_seed: None,
            audit: None,
            connector_resolver: None,
        }
    }

    pub fn with_slot_config(mut self, cfg: SlotAllocatorConfig) -> Self {
        self.slot_config = cfg;
        self
    }

    pub fn with_rng_seed(mut self, seed: u64) -> Self {
        self.rng_seed = Some(seed);
        self
    }

    pub fn with_audit(mut self, audit: AuditWriter) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_connector_resolver(mut self, resolver: Arc<CollectorConnectorResolver>) -> Self {
        self.connector_resolver = Some(resolver);
        self
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    pub async fn business_date(&self) -> Result<NaiveDate, String> {
        let schedule = self.settings.get_schedule().await?;
        Ok(business_date_for(self.now(), &schedule.timezone))
    }

    /// 调度 tick：处于计划窗口时幂等入队 build_daily_plan。
    pub async fn tick_schedule_enqueue(&self) -> Result<Option<Job>, String> {
        let schedule = self.settings.get_schedule().await?;
        if !in_plan_start_window(self.now(), &schedule) {
            return Ok(None);
        }
        let date = self.business_date().await?;
        self.enqueue_build(date, false).await.map(Some)
    }

    /// 幂等入队 `build_daily_plan`。
    pub async fn enqueue_build(&self, date: NaiveDate, rebuild: bool) -> Result<Job, String> {
        let key = idempotency::daily_plan(&date.format("%Y-%m-%d").to_string());
        // rebuild 使用独立幂等后缀，避免与日任务冲突
        let key = if rebuild {
            format!("{key}:rebuild:{}", Uuid::new_v4())
        } else {
            key
        };
        let payload = BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild,
        };
        self.jobs
            .create_job(CreateJob::new(
                KIND_BUILD_DAILY_PLAN,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn list_for_date(&self, date: NaiveDate) -> Result<DailyPlanListDto, String> {
        let snap = self.health.get_by_date(date).await?;
        let rows = self.plans.list_by_date(date).await?;
        let planned_count = rows
            .iter()
            .filter(|r| r.status == DailyPlanStatus::Planned)
            .count();
        let started_count = rows.iter().filter(|r| r.status.is_started()).count();
        Ok(DailyPlanListDto {
            plan_date: date.format("%Y-%m-%d").to_string(),
            snapshot_id: snap.as_ref().map(|s| s.id),
            overall_status: snap.as_ref().map(|s| s.overall_status.as_str().to_string()),
            can_build_executable: snap
                .as_ref()
                .map(|s| s.overall_status != OverallHealthStatus::Unhealthy)
                .unwrap_or(false),
            block_reason: if snap.is_none() {
                Some("health.snapshot_unavailable".into())
            } else if snap
                .as_ref()
                .map(|s| s.overall_status == OverallHealthStatus::Unhealthy)
                .unwrap_or(false)
            {
                Some("health.snapshot_unhealthy".into())
            } else {
                None
            },
            eligible_count: rows
                .iter()
                .filter(|r| r.reason_code == "plan.eligible")
                .count(),
            planned_count,
            started_count,
            rows: rows.into_iter().map(plan_to_dto).collect(),
        })
    }

    /// 执行建计划（job handler / 同步测试入口）。
    pub async fn build(
        &self,
        payload: &BuildDailyPlanPayload,
    ) -> Result<serde_json::Value, String> {
        let date = NaiveDate::parse_from_str(&payload.plan_date, "%Y-%m-%d")
            .map_err(|e| format!("daily_plan.bad_date: {e}"))?;
        let schedule = self.settings.get_schedule().await?;
        let now = self.now();
        let day_end = day_end_utc(date, &schedule.timezone);

        let snap = self.health.get_by_date(date).await?;
        let components = if let Some(ref s) = snap {
            self.health.list_components(s.id).await?
        } else {
            vec![]
        };

        let candidates = self.collect_candidates(day_end).await?;
        let eligibility = evaluate_eligibility(EligibilityInput {
            plan_date: date,
            day_end,
            snapshot: snap.as_ref(),
            components: &components,
            sources: &candidates,
        });

        if payload.rebuild {
            let existing = self.plans.list_by_date(date).await?;
            for plan in existing
                .iter()
                .filter(|plan| plan.status == DailyPlanStatus::Planned)
            {
                if let Some(job_id) = plan.fetch_job_id {
                    self.jobs
                        .request_cancel(JobId::from_raw(job_id))
                        .await
                        .map_err(|error| {
                            format!("daily_plan.cancel_planned_job_failed: {error}")
                        })?;
                }
            }
            let deleted = self.plans.delete_planned_for_date(date).await?;
            tracing::info!(deleted, %date, "daily_plan.rebuild_cleared_planned");
        }

        if !eligibility.can_build_executable {
            let result = json!({
                "ok": true,
                "plan_date": payload.plan_date,
                "can_build_executable": false,
                "block_reason": eligibility.block_reason,
                "created": 0,
                "skipped_existing": 0,
                "jobs_created": 0,
            });
            if payload.rebuild {
                self.audit_rebuilt(&payload.plan_date, false, 0, 0, 0, now)
                    .await;
            }
            return Ok(result);
        }

        let snap = snap.expect("can_build implies snapshot");
        let mut allocator = match self.rng_seed {
            Some(seed) => SlotAllocator::with_seed(self.slot_config.clone(), seed),
            None => SlotAllocator::new(self.slot_config.clone()),
        };

        let requests: Vec<SlotRequest> = eligibility
            .eligible
            .iter()
            .map(|e| SlotRequest {
                source_account_id: e.source_account_id,
                collector_account_id: e.collector_account_id,
                weight: e.weight,
            })
            .collect();

        let assignments = if requests.is_empty() {
            vec![]
        } else {
            allocator.allocate(
                date,
                &schedule.timezone,
                &schedule.working_windows,
                &requests,
            )?
        };

        let mut created = 0u32;
        let mut skipped_existing = 0u32;
        let mut jobs_created = 0u32;

        for a in &assignments {
            let elig = eligibility
                .eligible
                .iter()
                .find(|e| e.source_account_id == a.source_account_id)
                .expect("assignment from eligible");

            // 心跳事件占位 id（与 fetch 幂等键关联；真实事件在抓取完成后由 HeartbeatService 写入）
            let heartbeat_event_id = Uuid::new_v4();
            let plan = DailyFetchPlan {
                id: Uuid::new_v4(),
                plan_date: date,
                source_account_id: a.source_account_id,
                health_snapshot_id: snap.id,
                planned_run_at: a.planned_run_at,
                status: DailyPlanStatus::Planned,
                reason_code: elig.reason_code.clone(),
                heartbeat_event_id: Some(heartbeat_event_id),
                fetch_job_id: None,
                collector_account_id: Some(a.collector_account_id),
                created_at: now,
                updated_at: now,
            };

            let (stored, wrote) = self.plans.upsert_rebuildable(&plan).await?;
            if !wrote {
                skipped_existing += 1;
                continue;
            }
            created += 1;

            let job_id = self
                .enqueue_fetch_for_plan(
                    &stored,
                    elig.source_identity.as_str(),
                    elig.connector.as_str(),
                )
                .await?;
            let _ = self
                .plans
                .update_status(stored.id, DailyPlanStatus::Planned, None, Some(job_id), now)
                .await;
            jobs_created += 1;
        }

        let result = json!({
            "ok": true,
            "plan_date": payload.plan_date,
            "can_build_executable": true,
            "block_reason": null,
            "eligible": eligibility.eligible.len(),
            "ineligible": eligibility.ineligible.len(),
            "created": created,
            "skipped_existing": skipped_existing,
            "jobs_created": jobs_created,
            "rebuild": payload.rebuild,
        });
        if payload.rebuild {
            self.audit_rebuilt(
                &payload.plan_date,
                true,
                created,
                skipped_existing,
                jobs_created,
                now,
            )
            .await;
        }
        Ok(result)
    }

    async fn audit_rebuilt(
        &self,
        plan_date: &str,
        can_build_executable: bool,
        created: u32,
        skipped_existing: u32,
        jobs_created: u32,
        now: DateTime<Utc>,
    ) {
        if let Some(ref audit) = self.audit {
            let actor = if can_build_executable {
                ACTOR_WORKER
            } else {
                ACTOR_SYSTEM
            };
            let _ = audit
                .success(
                    actor,
                    Some("daily_plan"),
                    "daily_plan.rebuilt",
                    Some("daily_plan"),
                    Some(plan_date),
                    json!({
                        "plan_date": plan_date,
                        "can_build_executable": can_build_executable,
                        "created": created,
                        "skipped_existing": skipped_existing,
                        "jobs_created": jobs_created,
                    }),
                    now,
                )
                .await;
        }
    }

    async fn collect_candidates(
        &self,
        day_end: DateTime<Utc>,
    ) -> Result<Vec<SourceCandidate>, String> {
        let sources = self.sources.list_sources().await?;
        let collectors = self.collectors.list_accounts().await.unwrap_or_default();
        let mut out = Vec::new();

        for account in sources {
            let rules = match self.sources.get_fetch_rules(account.id).await? {
                Some(r) => r,
                None => continue,
            };
            // 有效等级：promotion 期内按 L1
            let level = effective_level(&rules.base_level, rules.promotion_until, self.now());
            let ids = self.sources.list_source_ids(account.id).await?;
            let identity = ids.first().map(|i| i.id_value.clone()).unwrap_or_default();
            let preferred_connector = account
                .preferred_connector
                .clone()
                .or(rules.connector_override.clone());
            let (connector, collector_id, collector_status) =
                if let Some(resolver) = &self.connector_resolver {
                    match resolver
                        .resolve(
                            ConnectorCapability::DiscoverRecent,
                            None,
                            preferred_connector.clone(),
                        )
                        .await
                    {
                        Ok(selected) => (
                            selected.account.connector,
                            Some(selected.account.id),
                            Some(crate::domain::collector::CollectorAccountStatus::Active),
                        ),
                        Err(_) => (preferred_connector.unwrap_or_default(), None, None),
                    }
                } else {
                    let connector = preferred_connector.unwrap_or_default();
                    let collector = collectors
                        .iter()
                        .find(|c| c.connector == connector && c.status.as_str() == "active");
                    (
                        connector,
                        collector.map(|c| c.id),
                        collector.map(|c| c.status),
                    )
                };

            let _ = day_end; // 过滤在 eligibility
            out.push(SourceCandidate {
                account,
                base_level: level,
                enabled: rules.enabled,
                next_heartbeat_at: rules
                    .manual_next_heartbeat_at
                    .unwrap_or(rules.next_heartbeat_at),
                source_identity: identity,
                connector,
                collector_account_id: collector_id,
                collector_status,
            });
        }
        Ok(out)
    }

    async fn enqueue_fetch_for_plan(
        &self,
        plan: &DailyFetchPlan,
        identity: &str,
        connector: &str,
    ) -> Result<Uuid, String> {
        let hb = plan
            .heartbeat_event_id
            .map(|u| u.to_string())
            .unwrap_or_else(|| format!("plan-{}", plan.id));
        let key = idempotency::source_fetch(&plan.source_account_id.to_string(), &hb);
        let payload = json!({
            "source_account_id": plan.source_account_id,
            "source_identity": identity,
            "heartbeat_event_id": hb,
            "collector_account_id": plan.collector_account_id.unwrap_or(Uuid::nil()),
            "connector": connector,
            "limit": 10,
            "manual": false,
            "recalculate_next": true,
            "daily_plan_id": plan.id,
        });
        let mut cmd = CreateJob::new(KIND_FETCH_SOURCE, key, payload);
        cmd.next_run_at = Some(plan.planned_run_at);
        cmd.safe_context_json = Some(json!({
            "job_kind": KIND_FETCH_SOURCE,
            "source_id": plan.source_account_id.to_string(),
            "status": "planned",
        }));
        let job = self.jobs.create_job(cmd).await.map_err(|e| e.to_string())?;
        Ok(*job.id.as_uuid())
    }

    /// Job 开始前二次校验：暂停/L4 → skipped（更新计划行）。
    pub async fn preflight_plan_row(&self, plan_id: Uuid) -> Result<Option<&'static str>, String> {
        let plan = self
            .plans
            .get_by_id(plan_id)
            .await?
            .ok_or_else(|| "daily_plan.not_found".to_string())?;
        let source = self
            .sources
            .get_source(plan.source_account_id)
            .await?
            .ok_or_else(|| "source.not_found".to_string())?;
        let rules = self
            .sources
            .get_fetch_rules(plan.source_account_id)
            .await?
            .ok_or_else(|| "source.fetch_rules_not_found".to_string())?;
        let level = effective_level(&rules.base_level, rules.promotion_until, self.now());
        if let Some(reason) = should_skip_at_runtime(source.status, level) {
            let _ = self
                .plans
                .update_status(
                    plan_id,
                    DailyPlanStatus::Skipped,
                    Some(reason.to_string()),
                    None,
                    self.now(),
                )
                .await;
            return Ok(Some(reason));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::collector::{CollectorAccount, CollectorAccountStatus};
    use crate::domain::health::{
        HealthComponentStatus, HealthComponentType, ServiceHealthComponent, ServiceHealthSnapshot,
    };
    use crate::domain::settings::{ScheduleSettings, WorkingWindow};
    use crate::domain::source::{IdConfidence, SourceIdType};
    use crate::domain::source::{SourceLevel, SourceStatus};
    use crate::repository::daily_plan_memory::MemoryDailyPlanStore;
    use crate::repository::health_memory::MemoryHealthStore;
    use crate::repository::memory::MemoryAdministratorStore;
    use crate::repository::source::{
        CreateSourceAccount, CreateSourceAccountId, SourceRepository, UpsertFetchRules,
    };
    use crate::repository::source_memory::MemorySourceStore;
    use crate::repository::{HealthSnapshotRepository, MemoryCollectorStore, SettingsRepository};
    use chrono::Duration;
    use job_pipeline::{FakeHandler, MemoryRepo, Worker};
    use server_core::FrozenClock;
    use std::time::{Duration as StdDuration, SystemTime};

    async fn setup_service(
        overall: OverallHealthStatus,
    ) -> (
        Arc<DailyPlanService>,
        Arc<MemoryDailyPlanStore>,
        Arc<MemorySourceStore>,
        NaiveDate,
    ) {
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        // 2023-11-18 08:00 Asia/Shanghai = 2023-11-18 00:00 UTC
        let frozen = SystemTime::UNIX_EPOCH + StdDuration::from_secs(1_700_265_600);
        let clock = Arc::new(FrozenClock::new(frozen)) as Arc<dyn Clock>;

        let admin_store = Arc::new(MemoryAdministratorStore::new());
        let schedule = ScheduleSettings {
            timezone: "Asia/Shanghai".into(),
            working_windows: vec![WorkingWindow {
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            ..ScheduleSettings::default()
        };
        admin_store.update_schedule(&schedule).await.unwrap();

        let health = Arc::new(MemoryHealthStore::new());
        let snap_id = Uuid::new_v4();
        let snap = ServiceHealthSnapshot {
            id: snap_id,
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
        };
        let collector_id = Uuid::new_v4();
        let comps = vec![
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: snap_id,
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({}),
            },
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: snap_id,
                component_type: HealthComponentType::Connector,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({ "connector": "fake" }),
            },
            ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id: snap_id,
                component_type: HealthComponentType::Collector,
                component_id: Some(collector_id),
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                checked_at: Utc::now(),
                safe_details_json: json!({}),
            },
        ];
        health.insert_snapshot(&snap, &comps).await.unwrap();

        let sources = Arc::new(MemorySourceStore::new());
        let collectors = Arc::new(MemoryCollectorStore::new());
        let now = DateTime::<Utc>::from(frozen);
        collectors
            .insert_account(&CollectorAccount {
                id: collector_id,
                connector: "fake".into(),
                display_name: "c1".into(),
                external_account_ref: None,
                status: CollectorAccountStatus::Active,
                capability_json: json!({}),
                last_probe_at: None,
                last_success_at: None,
                last_failure_at: None,
                last_error_code: None,
                cooldown_until: None,
                daily_budget_remaining: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();

        let sid = Uuid::new_v4();
        sources
            .insert_source(CreateSourceAccount {
                id: sid,
                display_name: "src".into(),
                status: SourceStatus::Active,
                preferred_connector: Some("fake".into()),
                now,
            })
            .await
            .unwrap();
        sources
            .insert_source_id(CreateSourceAccountId {
                id: Uuid::new_v4(),
                source_account_id: sid,
                id_type: SourceIdType::Biz,
                id_value: "biz-test".into(),
                confidence: IdConfidence::Strong,
            })
            .await
            .unwrap();
        sources
            .upsert_fetch_rules(UpsertFetchRules {
                source_account_id: sid,
                base_level: SourceLevel::L1,
                working_windows: vec![],
                first_heartbeat_at: now,
                next_heartbeat_at: now + Duration::hours(1),
                connector_override: None,
                now,
            })
            .await
            .unwrap();

        let plans = Arc::new(MemoryDailyPlanStore::new());
        let jobs = Arc::new(MemoryRepo::new());
        let svc = Arc::new(
            DailyPlanService::new(
                plans.clone() as Arc<dyn DailyPlanRepository>,
                health as Arc<dyn HealthSnapshotRepository>,
                sources.clone() as Arc<dyn SourceRepository>,
                collectors as Arc<dyn CollectorRepository>,
                admin_store as Arc<dyn SettingsRepository>,
                jobs as Arc<dyn JobRepository>,
                clock,
            )
            .with_rng_seed(99),
        );
        (svc, plans, sources, date)
    }

    #[tokio::test]
    async fn build_with_snapshot_creates_plan_and_job() {
        let (svc, plans, _, date) = setup_service(OverallHealthStatus::Healthy).await;
        let result = svc
            .build(&BuildDailyPlanPayload {
                plan_date: date.format("%Y-%m-%d").to_string(),
                rebuild: false,
            })
            .await
            .unwrap();
        assert_eq!(result["can_build_executable"], true);
        assert!(result["created"].as_u64().unwrap() >= 1);
        assert!(result["jobs_created"].as_u64().unwrap() >= 1);
        let rows = plans.list_by_date(date).await.unwrap();
        assert!(!rows.is_empty());
        assert_eq!(rows[0].status, DailyPlanStatus::Planned);
        assert!(rows[0].fetch_job_id.is_some());
    }

    #[tokio::test]
    async fn unhealthy_creates_no_executable_plan() {
        let date = NaiveDate::from_ymd_opt(2023, 11, 19).unwrap();
        let frozen = SystemTime::UNIX_EPOCH + StdDuration::from_secs(1_700_352_000);
        let clock = Arc::new(FrozenClock::new(frozen)) as Arc<dyn Clock>;
        let admin_store = Arc::new(MemoryAdministratorStore::new());
        admin_store
            .update_schedule(&ScheduleSettings::default())
            .await
            .unwrap();
        let health = Arc::new(MemoryHealthStore::new());
        let snap_id = Uuid::new_v4();
        health
            .insert_snapshot(
                &ServiceHealthSnapshot {
                    id: snap_id,
                    snapshot_date: date,
                    generated_at: Utc::now(),
                    overall_status: OverallHealthStatus::Unhealthy,
                    yesterday_plan_success_count: 0,
                    yesterday_plan_failure_count: 0,
                    yesterday_article_failure_count: 0,
                    today_eligible_source_count: 0,
                    today_blocked_source_count: 0,
                    pending_action_count: 0,
                    summary_json: json!({}),
                },
                &[],
            )
            .await
            .unwrap();
        let plans = Arc::new(MemoryDailyPlanStore::new());
        let svc = DailyPlanService::new(
            plans.clone() as Arc<dyn DailyPlanRepository>,
            health as Arc<dyn HealthSnapshotRepository>,
            Arc::new(MemorySourceStore::new()) as Arc<dyn SourceRepository>,
            Arc::new(MemoryCollectorStore::new()) as Arc<dyn CollectorRepository>,
            admin_store as Arc<dyn SettingsRepository>,
            Arc::new(MemoryRepo::new()) as Arc<dyn JobRepository>,
            clock,
        );
        let result = svc
            .build(&BuildDailyPlanPayload {
                plan_date: date.format("%Y-%m-%d").to_string(),
                rebuild: false,
            })
            .await
            .unwrap();
        assert_eq!(result["can_build_executable"], false);
        assert_eq!(result["created"], 0);
        assert!(plans.list_by_date(date).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn rebuild_preserves_started_rows() {
        let (svc, plans, _, date) = setup_service(OverallHealthStatus::Healthy).await;
        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: false,
        })
        .await
        .unwrap();
        let rows = plans.list_by_date(date).await.unwrap();
        let started_id = rows[0].id;
        plans
            .update_status(started_id, DailyPlanStatus::Running, None, None, Utc::now())
            .await
            .unwrap();

        // 再建：不应覆盖 running
        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: true,
        })
        .await
        .unwrap();
        let after = plans.get_by_id(started_id).await.unwrap().unwrap();
        assert_eq!(after.status, DailyPlanStatus::Running);
    }

    #[tokio::test]
    async fn rebuild_cancels_old_planned_job_before_replacing_plan() {
        let (svc, plans, _, date) = setup_service(OverallHealthStatus::Healthy).await;
        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: false,
        })
        .await
        .unwrap();
        let old_plan = plans.list_by_date(date).await.unwrap().remove(0);
        let old_job_id = old_plan.fetch_job_id.unwrap();

        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: true,
        })
        .await
        .unwrap();

        let old_job = svc
            .jobs
            .get_job(JobId::from_raw(old_job_id))
            .await
            .unwrap()
            .unwrap();
        // Cancellation is intentionally asynchronous: the rebuild requests it,
        // then the worker consumes the request and records the terminal state.
        assert_eq!(old_job.status, job_pipeline::JobStatus::Cancelling);
        assert!(old_job.cancel_requested_at.is_some());
        let replacement = plans.list_by_date(date).await.unwrap().remove(0);
        assert_ne!(replacement.id, old_plan.id);
        assert_ne!(replacement.fetch_job_id, Some(old_job_id));
    }

    #[tokio::test]
    async fn rebuild_cancel_request_is_consumed_by_worker() {
        let (svc, plans, _, date) = setup_service(OverallHealthStatus::Healthy).await;
        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: false,
        })
        .await
        .unwrap();
        let old_plan = plans.list_by_date(date).await.unwrap().remove(0);
        let old_job_id = old_plan.fetch_job_id.unwrap();

        svc.build(&BuildDailyPlanPayload {
            plan_date: date.format("%Y-%m-%d").to_string(),
            rebuild: true,
        })
        .await
        .unwrap();

        // The old planned fetch is due in the test fixture, so a worker poll
        // can consume the cancellation immediately.  Registering a handler is
        // deliberate: a cancelling job must be finalized without executing it.
        let worker = Worker::new("daily-plan-cancel-test", svc.jobs.clone()).register(Arc::new(
            FakeHandler {
                kind: KIND_FETCH_SOURCE.into(),
            },
        ));
        worker.run_once().await.unwrap();

        let old_job = svc
            .jobs
            .get_job(JobId::from_raw(old_job_id))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_job.status, job_pipeline::JobStatus::Cancelled);
        assert!(old_job.cancel_requested_at.is_some());
        assert!(old_job.cancelled_at.is_some());
        assert_eq!(old_job.attempt_count, 0);
        assert!(old_job.result_json.is_none());
    }
}
