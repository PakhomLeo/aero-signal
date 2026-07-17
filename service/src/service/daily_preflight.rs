//! DailyPreflightService：07:30 一次性预检并固化不可变健康快照。

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use crawler_capture::ObjectStore;
use job_pipeline::kinds::idempotency;
use job_pipeline::{CreateJob, Job, JobRepository, KIND_COLLECTOR_PROBE, KIND_DAILY_PREFLIGHT};
use platform_connectors::{CollectorAccountView, ConnectorRegistry};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use server_core::Clock;
use uuid::Uuid;

use crate::adapters::secret_store::SecretStore;
use crate::domain::collector::CollectorAccountStatus;
use crate::domain::egress::{EgressKind, EgressStatus};
use crate::domain::health::{
    HealthCheckRun, HealthComponentStatus, HealthComponentType, OverallHealthStatus,
    ServiceHealthComponent, ServiceHealthSnapshot, aggregate_overall, sanitize_safe_details,
};
use crate::domain::settings::ScheduleSettings;
use crate::repository::collector::CollectorRepository;
use crate::repository::egress::EgressRepository;
use crate::repository::health::HealthSnapshotRepository;
use crate::repository::settings::SettingsRepository;
use crate::service::daily_preflight_mapping;

/// 单项子检查默认超时。
pub const DEFAULT_COMPONENT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyPreflightPayload {
    /// YYYY-MM-DD（业务时区日历日）。
    pub snapshot_date: String,
    /// 若为 true：仅写独立 check run，不写/不覆盖快照。
    #[serde(default)]
    pub immediate_check: bool,
    /// 立即检查目标类型（collector / fixed_egress / … / all）。
    #[serde(default)]
    pub target_type: Option<String>,
    pub target_id: Option<Uuid>,
}

#[derive(Debug, Clone)]
pub struct ComponentCheckResult {
    pub component_type: HealthComponentType,
    pub component_id: Option<Uuid>,
    pub status: HealthComponentStatus,
    pub reason_code: Option<String>,
    pub safe_details_json: Value,
    pub checked_at: DateTime<Utc>,
}

/// 子检查接口：可注入 fake/桩。
#[async_trait::async_trait]
pub trait HealthComponentChecker: Send + Sync {
    async fn check_all(&self, now: DateTime<Utc>) -> Vec<ComponentCheckResult>;
}

/// 默认子检查：collector / connector / fixed&dynamic egress / worker / object_store。
pub struct DefaultHealthComponentChecker {
    collectors: Arc<dyn CollectorRepository>,
    egress: Arc<dyn EgressRepository>,
    connectors: Arc<ConnectorRegistry>,
    object_store: Arc<dyn ObjectStore>,
    secrets: Arc<dyn SecretStore>,
    /// worker 可用性桩：true=healthy。
    worker_healthy: Arc<std::sync::atomic::AtomicBool>,
    timeout: Duration,
}

impl DefaultHealthComponentChecker {
    pub fn new(
        collectors: Arc<dyn CollectorRepository>,
        egress: Arc<dyn EgressRepository>,
        connectors: Arc<ConnectorRegistry>,
        object_store: Arc<dyn ObjectStore>,
        secrets: Arc<dyn SecretStore>,
    ) -> Self {
        Self {
            collectors,
            egress,
            connectors,
            object_store,
            secrets,
            worker_healthy: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            timeout: DEFAULT_COMPONENT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn set_worker_healthy(&self, healthy: bool) {
        self.worker_healthy
            .store(healthy, std::sync::atomic::Ordering::SeqCst);
    }

    async fn with_timeout_result<F, Fut>(&self, fut: F) -> Result<ComponentCheckResult, ()>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ComponentCheckResult>,
    {
        match tokio::time::timeout(self.timeout, fut()).await {
            Ok(r) => Ok(r),
            Err(_) => Err(()),
        }
    }
}

#[async_trait::async_trait]
impl HealthComponentChecker for DefaultHealthComponentChecker {
    async fn check_all(&self, now: DateTime<Utc>) -> Vec<ComponentCheckResult> {
        let mut out = Vec::new();

        // worker（核心）
        let worker_ok = self
            .worker_healthy
            .load(std::sync::atomic::Ordering::SeqCst);
        out.push(ComponentCheckResult {
            component_type: HealthComponentType::Worker,
            component_id: None,
            status: if worker_ok {
                HealthComponentStatus::Healthy
            } else {
                HealthComponentStatus::Unhealthy
            },
            reason_code: if worker_ok {
                None
            } else {
                Some("worker.unavailable".into())
            },
            safe_details_json: json!({ "status": if worker_ok { "ok" } else { "down" } }),
            checked_at: now,
        });

        // object_store（核心）
        let store = self.object_store.clone();
        let store_check = self
            .with_timeout_result(|| async move {
                match store.exists_by_hash("preflight-probe-missing").await {
                    Ok(_) => ComponentCheckResult {
                        component_type: HealthComponentType::ObjectStore,
                        component_id: None,
                        status: HealthComponentStatus::Healthy,
                        reason_code: None,
                        safe_details_json: json!({ "status": "ok" }),
                        checked_at: now,
                    },
                    Err(e) => ComponentCheckResult {
                        component_type: HealthComponentType::ObjectStore,
                        component_id: None,
                        status: HealthComponentStatus::Unhealthy,
                        reason_code: Some("object_store.unavailable".into()),
                        safe_details_json: json!({
                            "error_code": e.code,
                        }),
                        checked_at: now,
                    },
                }
            })
            .await;
        out.push(match store_check {
            Ok(r) => r,
            Err(()) => ComponentCheckResult {
                component_type: HealthComponentType::ObjectStore,
                component_id: None,
                status: HealthComponentStatus::Unhealthy,
                reason_code: Some("object_store.timeout".into()),
                safe_details_json: json!({ "error_code": "object_store.timeout" }),
                checked_at: now,
            },
        });

        // collectors + connector capability（只读 probe，不 discover）
        let accounts = self.collectors.list_accounts().await.unwrap_or_default();
        if accounts.is_empty() {
            out.push(ComponentCheckResult {
                component_type: HealthComponentType::Collector,
                component_id: None,
                status: HealthComponentStatus::NotConfigured,
                reason_code: Some("collector.not_configured".into()),
                safe_details_json: json!({ "counts": { "total": 0 } }),
                checked_at: now,
            });
        } else {
            for account in accounts {
                if account.status == CollectorAccountStatus::Disabled {
                    continue;
                }
                let collectors = self.collectors.clone();
                let connectors = self.connectors.clone();
                let secrets = self.secrets.clone();
                let account_id = account.id;
                let connector_name = account.connector.clone();
                let status = account.status;
                let display = account.display_name.clone();
                let check = self
                    .with_timeout_result(|| async move {
                        let binding = collectors
                            .get_egress_binding(account_id)
                            .await
                            .ok()
                            .flatten();
                        let session = collectors
                            .get_active_login_session(account_id)
                            .await
                            .ok()
                            .flatten();
                        let (credentials, authorized_mp_ids) = match session {
                            Some(session) => (
                                secrets
                                    .get(&session.credential_secret_ref)
                                    .await
                                    .ok()
                                    .flatten(),
                                session.authorized_mp_ids,
                            ),
                            None => (None, Vec::new()),
                        };
                        let backend = match connectors.get(&connector_name) {
                            Some(b) => b,
                            None => {
                                return ComponentCheckResult {
                                    component_type: HealthComponentType::Collector,
                                    component_id: Some(account_id),
                                    status: HealthComponentStatus::Unhealthy,
                                    reason_code: Some("connector.unsupported".into()),
                                    safe_details_json: json!({
                                        "error_code": "connector.unsupported",
                                        "connector": connector_name,
                                    }),
                                    checked_at: now,
                                };
                            }
                        };
                        let view = CollectorAccountView {
                            id: account_id,
                            connector: connector_name.clone(),
                            display_name: Some(display),
                            active: status == CollectorAccountStatus::Active,
                            needs_relogin: status == CollectorAccountStatus::NeedsRelogin
                                || binding
                                    .as_ref()
                                    .map(|b| b.requires_relogin)
                                    .unwrap_or(false),
                            fixed_egress_healthy: true,
                            fixed_egress_id: binding.as_ref().and_then(|b| b.fixed_egress_id),
                            cooling: false,
                            budget_exhausted: false,
                            enabled_capabilities: backend.capabilities(),
                            authorized_mp_ids,
                        };
                        match backend.probe(&view, credentials.as_deref(), now).await {
                            Ok(probe) => {
                                let (st, reason) = match probe.status {
                                    platform_connectors::ProbeStatus::Healthy => {
                                        (HealthComponentStatus::Healthy, None)
                                    }
                                    platform_connectors::ProbeStatus::Degraded => {
                                        (HealthComponentStatus::Degraded, probe.reason_code.clone())
                                    }
                                    platform_connectors::ProbeStatus::NeedsRelogin => (
                                        HealthComponentStatus::Unhealthy,
                                        Some("collector.needs_relogin".into()),
                                    ),
                                    platform_connectors::ProbeStatus::Failed
                                    | platform_connectors::ProbeStatus::Unsupported => (
                                        HealthComponentStatus::Unhealthy,
                                        probe
                                            .reason_code
                                            .clone()
                                            .or_else(|| Some("collector.probe_failed".into())),
                                    ),
                                };
                                ComponentCheckResult {
                                    component_type: HealthComponentType::Collector,
                                    component_id: Some(account_id),
                                    status: st,
                                    reason_code: reason,
                                    safe_details_json: json!({
                                        "connector": connector_name,
                                        "status": st.as_str(),
                                    }),
                                    checked_at: now,
                                }
                            }
                            Err(e) => ComponentCheckResult {
                                component_type: HealthComponentType::Collector,
                                component_id: Some(account_id),
                                status: HealthComponentStatus::Unhealthy,
                                reason_code: Some(e.reason_code.clone()),
                                safe_details_json: json!({
                                    "error_code": e.reason_code,
                                    "connector": connector_name,
                                }),
                                checked_at: now,
                            },
                        }
                    })
                    .await;
                out.push(match check {
                    Ok(r) => r,
                    Err(()) => ComponentCheckResult {
                        component_type: HealthComponentType::Collector,
                        component_id: Some(account_id),
                        status: HealthComponentStatus::Unhealthy,
                        reason_code: Some("collector.timeout".into()),
                        safe_details_json: json!({ "error_code": "collector.timeout" }),
                        checked_at: now,
                    },
                });

                // connector capability 行（按账号 connector；此处每账号一条）
                if let Some(backend) = self.connectors.get(&account.connector) {
                    let caps = backend.capabilities();
                    let ok = !caps.is_empty();
                    out.push(ComponentCheckResult {
                        component_type: HealthComponentType::Connector,
                        component_id: Some(account.id),
                        status: if ok {
                            HealthComponentStatus::Healthy
                        } else {
                            HealthComponentStatus::Unhealthy
                        },
                        reason_code: if ok {
                            None
                        } else {
                            Some("connector.no_capability".into())
                        },
                        safe_details_json: json!({
                            "connector": account.connector,
                            "capability_count": caps.items.len(),
                        }),
                        checked_at: now,
                    });
                }
            }
        }

        // fixed / dynamic egress（只读状态，不测真实代理连通时可桩）
        let profiles = self.egress.list_egress_profiles().await.unwrap_or_default();
        let mut any_fixed = false;
        let mut any_dynamic = false;
        for p in profiles {
            match p.kind {
                EgressKind::Fixed | EgressKind::LocalDirect => {
                    any_fixed = true;
                    let st = match p.status {
                        EgressStatus::Active => HealthComponentStatus::Healthy,
                        EgressStatus::Degraded => HealthComponentStatus::Degraded,
                        EgressStatus::Disabled => HealthComponentStatus::NotConfigured,
                    };
                    out.push(ComponentCheckResult {
                        component_type: HealthComponentType::FixedEgress,
                        component_id: Some(p.id),
                        status: st,
                        reason_code: if st == HealthComponentStatus::Degraded {
                            Some("egress.degraded".into())
                        } else {
                            None
                        },
                        safe_details_json: json!({
                            "kind": p.kind.as_str(),
                            "status": p.status.as_str(),
                            "priority": p.priority,
                        }),
                        checked_at: now,
                    });
                }
                EgressKind::Dynamic => {
                    any_dynamic = true;
                    let st = match p.status {
                        EgressStatus::Active => HealthComponentStatus::Healthy,
                        EgressStatus::Degraded => HealthComponentStatus::Degraded,
                        EgressStatus::Disabled => HealthComponentStatus::NotConfigured,
                    };
                    out.push(ComponentCheckResult {
                        component_type: HealthComponentType::DynamicEgress,
                        component_id: Some(p.id),
                        status: st,
                        reason_code: if st == HealthComponentStatus::Degraded {
                            Some("egress.degraded".into())
                        } else {
                            None
                        },
                        safe_details_json: json!({
                            "kind": p.kind.as_str(),
                            "status": p.status.as_str(),
                            "priority": p.priority,
                        }),
                        checked_at: now,
                    });
                }
            }
        }
        if !any_fixed {
            out.push(ComponentCheckResult {
                component_type: HealthComponentType::FixedEgress,
                component_id: None,
                status: HealthComponentStatus::NotConfigured,
                reason_code: Some("egress.fixed_not_configured".into()),
                safe_details_json: json!({ "counts": { "total": 0 } }),
                checked_at: now,
            });
        }
        if !any_dynamic {
            out.push(ComponentCheckResult {
                component_type: HealthComponentType::DynamicEgress,
                component_id: None,
                status: HealthComponentStatus::NotConfigured,
                reason_code: Some("egress.dynamic_not_configured".into()),
                safe_details_json: json!({ "counts": { "total": 0 } }),
                checked_at: now,
            });
        }

        out
    }
}

/// 可控 Fake 检查器（测试用）。
pub struct FakeHealthComponentChecker {
    pub results: Arc<tokio::sync::Mutex<Vec<ComponentCheckResult>>>,
}

impl FakeHealthComponentChecker {
    pub fn new(results: Vec<ComponentCheckResult>) -> Self {
        Self {
            results: Arc::new(tokio::sync::Mutex::new(results)),
        }
    }
}

#[async_trait::async_trait]
impl HealthComponentChecker for FakeHealthComponentChecker {
    async fn check_all(&self, _now: DateTime<Utc>) -> Vec<ComponentCheckResult> {
        self.results.lock().await.clone()
    }
}

pub struct DailyPreflightService {
    health: Arc<dyn HealthSnapshotRepository>,
    jobs: Arc<dyn JobRepository>,
    settings: Arc<dyn SettingsRepository>,
    checker: Arc<dyn HealthComponentChecker>,
    clock: Arc<dyn Clock>,
}

impl DailyPreflightService {
    pub fn new(
        health: Arc<dyn HealthSnapshotRepository>,
        jobs: Arc<dyn JobRepository>,
        settings: Arc<dyn SettingsRepository>,
        checker: Arc<dyn HealthComponentChecker>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            health,
            jobs,
            settings,
            checker,
            clock,
        }
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    /// 业务日历日：用设置时区偏移近似（无 chrono-tz 时按固定偏移或 UTC 日期）。
    pub async fn business_date(&self) -> Result<NaiveDate, String> {
        let schedule = self
            .settings
            .get_schedule()
            .await
            .unwrap_or_else(|_| ScheduleSettings::default());
        Ok(business_date_for(self.now(), &schedule.timezone))
    }

    /// 是否处于 preflight 触发窗口（默认 07:30 起 15 分钟内，可测）。
    pub async fn in_preflight_window(&self) -> Result<bool, String> {
        let schedule = self.settings.get_schedule().await?;
        Ok(in_preflight_window(self.now(), &schedule))
    }

    /// 调度 tick：处于 preflight 窗口时幂等入队日预检。
    pub async fn tick_schedule_enqueue(&self) -> Result<Option<Job>, String> {
        if !self.in_preflight_window().await? {
            return Ok(None);
        }
        let date = self.business_date().await?;
        self.enqueue_daily(date).await.map(Some)
    }

    /// 创建或返回已有 `daily_preflight` job（幂等键 `preflight:<date>`）。
    pub async fn enqueue_daily(&self, date: NaiveDate) -> Result<Job, String> {
        let key = idempotency::preflight(&date.to_string());
        let payload = DailyPreflightPayload {
            snapshot_date: date.to_string(),
            immediate_check: false,
            target_type: None,
            target_id: None,
        };
        self.jobs
            .create_job(CreateJob::new(
                KIND_DAILY_PREFLIGHT,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())
    }

    /// 管理员「立即检查」：独立 job（无日幂等键），不覆盖快照。
    pub async fn enqueue_immediate_check(
        &self,
        target_type: &str,
        target_id: Option<Uuid>,
    ) -> Result<Job, String> {
        let now = self.now();
        let key = format!(
            "immediate-check:{}:{}:{}",
            target_type,
            target_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| "all".into()),
            now.timestamp_millis()
        );
        let payload = DailyPreflightPayload {
            snapshot_date: self.business_date().await?.to_string(),
            immediate_check: true,
            target_type: Some(target_type.into()),
            target_id,
        };
        // 使用 collector_probe kind 承载单对象立即检查，避免占用日幂等键。
        self.jobs
            .create_job(CreateJob::new(
                KIND_COLLECTOR_PROBE,
                key,
                serde_json::to_value(&payload).map_err(|e| e.to_string())?,
            ))
            .await
            .map_err(|e| e.to_string())
    }

    /// 执行预检：写快照（非 immediate）或写独立 check run（immediate）。
    pub async fn run(&self, payload: &DailyPreflightPayload) -> Result<Value, String> {
        let now = self.now();
        let date = NaiveDate::parse_from_str(&payload.snapshot_date, "%Y-%m-%d")
            .map_err(|e| format!("invalid snapshot_date: {e}"))?;

        let raw = self.checker.check_all(now).await;
        let results: Vec<ComponentCheckResult> = if payload.immediate_check {
            filter_immediate(raw, payload.target_type.as_deref(), payload.target_id)
        } else {
            raw
        };

        if payload.immediate_check {
            return self.persist_immediate_runs(&results, now).await;
        }

        // 已有当日快照：幂等返回，禁止 update
        if let Some(existing) = self.health.get_by_date(date).await? {
            let comps = self.health.list_components(existing.id).await?;
            return Ok(json!({
                "ok": true,
                "idempotent": true,
                "snapshot_id": existing.id,
                "snapshot_date": existing.snapshot_date.to_string(),
                "overall_status": existing.overall_status.as_str(),
                "component_count": comps.len(),
            }));
        }

        let snapshot_id = Uuid::new_v4();
        let mut components: Vec<ServiceHealthComponent> = results
            .into_iter()
            .map(|r| ServiceHealthComponent {
                id: Uuid::new_v4(),
                snapshot_id,
                component_type: r.component_type,
                component_id: r.component_id,
                status: r.status,
                reason_code: r.reason_code,
                checked_at: r.checked_at,
                safe_details_json: sanitize_safe_details(&r.safe_details_json),
            })
            .collect();

        // 单项失败仍写快照：即使 components 含 unhealthy 也继续
        let overall = aggregate_overall(&components);
        let pending = components
            .iter()
            .filter(|c| {
                matches!(
                    c.status,
                    HealthComponentStatus::Unhealthy | HealthComponentStatus::Degraded
                )
            })
            .count() as i32;

        let snapshot = ServiceHealthSnapshot {
            id: snapshot_id,
            snapshot_date: date,
            generated_at: now,
            overall_status: overall,
            yesterday_plan_success_count: 0,
            yesterday_plan_failure_count: 0,
            yesterday_article_failure_count: 0,
            today_eligible_source_count: 0,
            today_blocked_source_count: 0,
            pending_action_count: pending,
            summary_json: json!({
                "schema_version": 1,
                "component_count": components.len(),
                "overall_status": overall.as_str(),
            }),
        };

        // 确保组件 snapshot_id 一致
        for c in &mut components {
            c.snapshot_id = snapshot_id;
        }

        self.health.insert_snapshot(&snapshot, &components).await?;

        Ok(json!({
            "ok": true,
            "idempotent": false,
            "snapshot_id": snapshot_id,
            "snapshot_date": date.to_string(),
            "overall_status": overall.as_str(),
            "component_count": components.len(),
            "pending_action_count": pending,
        }))
    }

    async fn persist_immediate_runs(
        &self,
        results: &[ComponentCheckResult],
        now: DateTime<Utc>,
    ) -> Result<Value, String> {
        let mut run_ids = Vec::new();
        for r in results {
            let run = HealthCheckRun {
                id: Uuid::new_v4(),
                job_id: None,
                target_type: r.component_type.as_str().into(),
                target_id: r.component_id,
                status: r.status,
                reason_code: r.reason_code.clone(),
                safe_details_json: sanitize_safe_details(&r.safe_details_json),
                checked_at: r.checked_at,
                created_at: now,
            };
            run_ids.push(run.id);
            self.health.insert_check_run(&run).await?;
        }
        Ok(json!({
            "ok": true,
            "immediate_check": true,
            "run_ids": run_ids,
            "run_count": run_ids.len(),
            "snapshot_written": false,
        }))
    }

    /// 读取最新快照（含组件），供 Admin API。
    pub async fn latest_snapshot_view(&self) -> Result<Option<HealthSnapshotAdminDto>, String> {
        let Some(snap) = self.health.get_latest().await? else {
            return Ok(None);
        };
        let comps = self.health.list_components(snap.id).await?;
        Ok(Some(HealthSnapshotAdminDto::from_parts(snap, comps)))
    }
}

fn filter_immediate(
    results: Vec<ComponentCheckResult>,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
) -> Vec<ComponentCheckResult> {
    daily_preflight_mapping::filter_immediate(results, target_type, target_id)
}

/// 无 chrono-tz：Asia/Shanghai 等东八区用 +8h；其余用 UTC 日历日。
pub fn business_date_for(now: DateTime<Utc>, timezone: &str) -> NaiveDate {
    daily_preflight_mapping::business_date_for(now, timezone)
}

pub fn in_preflight_window(now: DateTime<Utc>, schedule: &ScheduleSettings) -> bool {
    daily_preflight_mapping::in_preflight_window(now, schedule)
}

/// Admin 脱敏 DTO（无 cookie/token/代理密码）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshotAdminDto {
    pub schema_version: u32,
    pub id: Uuid,
    pub snapshot_date: String,
    pub generated_at: DateTime<Utc>,
    pub overall_status: OverallHealthStatus,
    pub yesterday_plan_success_count: i32,
    pub yesterday_plan_failure_count: i32,
    pub yesterday_article_failure_count: i32,
    pub today_eligible_source_count: i32,
    pub today_blocked_source_count: i32,
    pub pending_action_count: i32,
    pub summary: Value,
    pub components: Vec<HealthComponentAdminDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthComponentAdminDto {
    pub component_type: HealthComponentType,
    /// 后台完整视图可含 id；仍不暴露密钥。
    pub component_id: Option<Uuid>,
    pub status: HealthComponentStatus,
    pub reason_code: Option<String>,
    pub checked_at: DateTime<Utc>,
    pub safe_details: Value,
}

impl HealthSnapshotAdminDto {
    pub fn from_parts(snap: ServiceHealthSnapshot, comps: Vec<ServiceHealthComponent>) -> Self {
        Self {
            schema_version: 1,
            id: snap.id,
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
                .map(|c| HealthComponentAdminDto {
                    component_type: c.component_type,
                    component_id: c.component_id,
                    status: c.status,
                    reason_code: c.reason_code,
                    checked_at: c.checked_at,
                    safe_details: sanitize_safe_details(&c.safe_details_json),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::secret_store::MemorySecretStore;
    use crate::domain::collector::{CollectorAccount, CollectorAccountStatus};
    use crate::domain::egress::{
        EgressKind, EgressProfile, EgressPurpose, EgressStatus, LOCAL_DIRECT_PROFILE_ID,
    };
    use crate::repository::{
        MemoryAdministratorStore, MemoryCollectorStore, MemoryEgressStore, MemoryHealthStore,
        SettingsRepository,
    };
    use crawler_capture::FakeObjectStore;
    use job_pipeline::MemoryRepo;
    use platform_connectors::{FakeConnector, FakeProbeBehavior, NAME_FAKE};
    use server_core::FrozenClock;
    use std::sync::Arc;

    fn frozen_shanghai_0730() -> Arc<FrozenClock> {
        // 2023-11-14 07:30 Asia/Shanghai = 2023-11-13 23:30 UTC
        // 使用已知毫秒：2023-11-14T00:00:00Z + 相关偏移
        // 更直接：设 UTC 使得 +8 = 07:30 → UTC 23:30 前日
        // 2023-11-13 23:30:00 UTC
        let dt = DateTime::parse_from_rfc3339("2023-11-13T23:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        Arc::new(FrozenClock::at_unix_millis(dt.timestamp_millis() as u64))
    }

    async fn build_svc(
        checker: Arc<dyn HealthComponentChecker>,
        clock: Arc<dyn Clock>,
    ) -> (
        DailyPreflightService,
        Arc<MemoryHealthStore>,
        Arc<MemoryRepo>,
    ) {
        let health = Arc::new(MemoryHealthStore::new());
        let jobs = Arc::new(MemoryRepo::new());
        let settings = Arc::new(MemoryAdministratorStore::new()) as Arc<dyn SettingsRepository>;
        let svc = DailyPreflightService::new(
            health.clone() as Arc<dyn HealthSnapshotRepository>,
            jobs.clone() as Arc<dyn JobRepository>,
            settings,
            checker,
            clock,
        );
        (svc, health, jobs)
    }

    fn core_healthy_results(now: DateTime<Utc>) -> Vec<ComponentCheckResult> {
        vec![
            ComponentCheckResult {
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                safe_details_json: json!({ "status": "ok" }),
                checked_at: now,
            },
            ComponentCheckResult {
                component_type: HealthComponentType::ObjectStore,
                component_id: None,
                status: HealthComponentStatus::Healthy,
                reason_code: None,
                safe_details_json: json!({ "status": "ok" }),
                checked_at: now,
            },
        ]
    }

    #[tokio::test]
    async fn preflight_idempotent_same_date() {
        let now = Utc::now();
        let checker = Arc::new(FakeHealthComponentChecker::new(core_healthy_results(now)));
        let (svc, health, jobs) = build_svc(
            checker,
            Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000)),
        )
        .await;
        let date = NaiveDate::from_ymd_opt(2023, 11, 14).unwrap();

        let j1 = svc.enqueue_daily(date).await.unwrap();
        let j2 = svc.enqueue_daily(date).await.unwrap();
        assert_eq!(j1.id, j2.id);
        assert_eq!(j1.idempotency_key.as_deref(), Some("preflight:2023-11-14"));
        let _ = jobs;

        let payload = DailyPreflightPayload {
            snapshot_date: date.to_string(),
            immediate_check: false,
            target_type: None,
            target_id: None,
        };
        let r1 = svc.run(&payload).await.unwrap();
        assert_eq!(r1["idempotent"], false);
        let r2 = svc.run(&payload).await.unwrap();
        assert_eq!(r2["idempotent"], true);
        assert_eq!(r1["snapshot_id"], r2["snapshot_id"]);

        // 禁止 update
        let snap = health.get_by_date(date).await.unwrap().unwrap();
        let err = health.update_snapshot(&snap).await.unwrap_err();
        assert!(err.contains("immutable") || err.contains("禁止"));
    }

    #[tokio::test]
    async fn component_failure_still_writes_snapshot() {
        let now = Utc::now();
        let mut results = core_healthy_results(now);
        results.push(ComponentCheckResult {
            component_type: HealthComponentType::Collector,
            component_id: Some(Uuid::new_v4()),
            status: HealthComponentStatus::Unhealthy,
            reason_code: Some("collector.needs_relogin".into()),
            safe_details_json: json!({
                "error_code": "collector.needs_relogin",
                "cookie": "SHOULD_NOT_PERSIST",
                "proxy_url": "http://user:pass@1.2.3.4:8080",
            }),
            checked_at: now,
        });
        let checker = Arc::new(FakeHealthComponentChecker::new(results));
        let (svc, health, _) = build_svc(
            checker,
            Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000)),
        )
        .await;
        let date = NaiveDate::from_ymd_opt(2023, 11, 15).unwrap();
        let r = svc
            .run(&DailyPreflightPayload {
                snapshot_date: date.to_string(),
                immediate_check: false,
                target_type: None,
                target_id: None,
            })
            .await
            .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["overall_status"], "degraded");
        let snap = health.get_by_date(date).await.unwrap().unwrap();
        let comps = health.list_components(snap.id).await.unwrap();
        assert!(!comps.is_empty());
        let collector = comps
            .iter()
            .find(|c| c.component_type == HealthComponentType::Collector)
            .unwrap();
        assert!(collector.safe_details_json.get("cookie").is_none());
        assert!(collector.safe_details_json.get("proxy_url").is_none());
    }

    #[tokio::test]
    async fn immediate_check_does_not_overwrite_snapshot() {
        let now = Utc::now();
        let checker = Arc::new(FakeHealthComponentChecker::new(core_healthy_results(now)));
        let (svc, health, _) = build_svc(
            checker.clone(),
            Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000)),
        )
        .await;
        let date = NaiveDate::from_ymd_opt(2023, 11, 16).unwrap();
        svc.run(&DailyPreflightPayload {
            snapshot_date: date.to_string(),
            immediate_check: false,
            target_type: None,
            target_id: None,
        })
        .await
        .unwrap();
        let before = health.get_by_date(date).await.unwrap().unwrap();

        // 改 checker 结果为 unhealthy，立即检查不应改快照
        {
            let mut g = checker.results.lock().await;
            *g = vec![ComponentCheckResult {
                component_type: HealthComponentType::Worker,
                component_id: None,
                status: HealthComponentStatus::Unhealthy,
                reason_code: Some("worker.unavailable".into()),
                safe_details_json: json!({}),
                checked_at: now,
            }];
        }

        let imm = svc
            .run(&DailyPreflightPayload {
                snapshot_date: date.to_string(),
                immediate_check: true,
                target_type: Some("worker".into()),
                target_id: None,
            })
            .await
            .unwrap();
        assert_eq!(imm["snapshot_written"], false);
        assert_eq!(imm["immediate_check"], true);

        let after = health.get_by_date(date).await.unwrap().unwrap();
        assert_eq!(before.id, after.id);
        assert_eq!(before.overall_status, after.overall_status);
        assert_eq!(after.overall_status, OverallHealthStatus::Healthy);

        let runs = health.list_check_runs(10).await.unwrap();
        assert!(!runs.is_empty());
    }

    #[tokio::test]
    async fn admin_dto_redacts_secrets() {
        let now = Utc::now();
        let snap = ServiceHealthSnapshot {
            id: Uuid::new_v4(),
            snapshot_date: NaiveDate::from_ymd_opt(2023, 11, 17).unwrap(),
            generated_at: now,
            overall_status: OverallHealthStatus::Healthy,
            yesterday_plan_success_count: 0,
            yesterday_plan_failure_count: 0,
            yesterday_article_failure_count: 0,
            today_eligible_source_count: 0,
            today_blocked_source_count: 0,
            pending_action_count: 0,
            summary_json: json!({ "token": "secret", "ok": true }),
        };
        let comps = vec![ServiceHealthComponent {
            id: Uuid::new_v4(),
            snapshot_id: snap.id,
            component_type: HealthComponentType::FixedEgress,
            component_id: Some(Uuid::new_v4()),
            status: HealthComponentStatus::Healthy,
            reason_code: None,
            checked_at: now,
            safe_details_json: json!({
                "cookie": "x",
                "proxy_password": "p",
                "status": "ok"
            }),
        }];
        let dto = HealthSnapshotAdminDto::from_parts(snap, comps);
        assert!(dto.summary.get("token").is_none());
        assert_eq!(dto.summary["ok"], true);
        assert!(dto.components[0].safe_details.get("cookie").is_none());
        assert!(
            dto.components[0]
                .safe_details
                .get("proxy_password")
                .is_none()
        );
        assert_eq!(dto.components[0].safe_details["status"], "ok");
    }

    #[tokio::test]
    async fn clock_preflight_window() {
        let clock = frozen_shanghai_0730();
        let schedule = ScheduleSettings::default();
        let now = DateTime::<Utc>::from(clock.now());
        assert!(in_preflight_window(now, &schedule));
        assert_eq!(
            business_date_for(now, "Asia/Shanghai"),
            NaiveDate::from_ymd_opt(2023, 11, 14).unwrap()
        );
    }

    #[tokio::test]
    async fn default_checker_with_local_egress() {
        let collectors = Arc::new(MemoryCollectorStore::new());
        let egress = Arc::new(MemoryEgressStore::with_local_direct());
        let mut reg = ConnectorRegistry::new();
        let fake = FakeConnector::new();
        fake.set_probe_behavior(FakeProbeBehavior::Healthy);
        reg.register(Arc::new(fake));
        let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
        let checker = DefaultHealthComponentChecker::new(
            collectors.clone() as Arc<dyn CollectorRepository>,
            egress as Arc<dyn EgressRepository>,
            Arc::new(reg),
            store,
            Arc::new(MemorySecretStore::new()),
        );
        let now = Utc::now();
        let results = checker.check_all(now).await;
        assert!(
            results
                .iter()
                .any(|r| r.component_type == HealthComponentType::Worker
                    && r.status == HealthComponentStatus::Healthy)
        );
        assert!(
            results
                .iter()
                .any(|r| r.component_type == HealthComponentType::ObjectStore)
        );
        let _ = MemorySecretStore::new();
        let _ = LOCAL_DIRECT_PROFILE_ID;
        let _ = EgressPurpose::Login;
        let _ = CollectorAccount {
            id: Uuid::new_v4(),
            connector: NAME_FAKE.into(),
            display_name: "t".into(),
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
        };
        let _ = EgressProfile {
            id: Uuid::new_v4(),
            name: "x".into(),
            kind: EgressKind::Fixed,
            provider_id: None,
            endpoint_secret_ref: None,
            priority: 1,
            allowed_purposes: vec![],
            status: EgressStatus::Active,
            last_preflight_status: None,
            last_preflight_at: None,
            created_at: now,
            updated_at: now,
        };
    }
}
