//! HeartbeatService：唯一写 next_heartbeat_at / 等级 / 空抓计数的入口。

use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use server_core::Clock;
use uuid::Uuid;

use crate::domain::source::{
    AccountFetchRules, HeartbeatOutcome, SourceHeartbeatEvent, SourceLevel, SourceStatus,
};
use crate::repository::source::{ApplyHeartbeatRules, SourceRepository};

/// 技术失败最大退避次数（之后 needs_action）。
pub const MAX_TECHNICAL_FAILURES: i32 = 3;
/// L2/L3 发现新文章后临时提升为 L1 的默认天数。
pub const DEFAULT_PROMOTION_DAYS: i64 = 14;

#[derive(Debug, Clone)]
pub struct HeartbeatPolicy {
    /// 是否允许 L2/L3 在 NewArticle 时限期提升为 L1。
    pub auto_promote_l2_l3: bool,
    pub promotion_days: i64,
    pub max_technical_failures: i32,
}

impl Default for HeartbeatPolicy {
    fn default() -> Self {
        Self {
            auto_promote_l2_l3: true,
            promotion_days: DEFAULT_PROMOTION_DAYS,
            max_technical_failures: MAX_TECHNICAL_FAILURES,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplyOutcomeCommand {
    pub source_account_id: Uuid,
    /// Stable command key supplied by the fetch job. Replays of the same key
    /// return the original event without changing counters or source status.
    pub event_key: String,
    pub outcome: HeartbeatOutcome,
    /// Manual 时：是否重算 next_heartbeat_at。
    pub recalculate_next: bool,
    /// Manual 且发现新文章时，可重置空抓（默认 true）。
    pub reset_empty_on_manual_new: bool,
    /// Manual 子结果：是否发现新文章（仅 Manual 有意义）。
    pub manual_found_new: bool,
}

#[derive(Debug, Clone)]
pub struct ApplyOutcomeResult {
    pub event: SourceHeartbeatEvent,
    pub rules: AccountFetchRules,
    /// AccessBlocked / 技术失败耗尽时置为 NeedsAction。
    pub source_status: Option<SourceStatus>,
    pub effective_level: SourceLevel,
}

pub struct HeartbeatService {
    sources: Arc<dyn SourceRepository>,
    clock: Arc<dyn Clock>,
    policy: HeartbeatPolicy,
}

impl HeartbeatService {
    pub fn new(sources: Arc<dyn SourceRepository>, clock: Arc<dyn Clock>) -> Self {
        Self {
            sources,
            clock,
            policy: HeartbeatPolicy::default(),
        }
    }

    pub fn with_policy(mut self, policy: HeartbeatPolicy) -> Self {
        self.policy = policy;
        self
    }

    fn now(&self) -> DateTime<Utc> {
        DateTime::<Utc>::from(self.clock.now())
    }

    /// 抓取完成后的唯一入口：写规则 + 心跳事件（可选改 source 状态）。
    pub async fn apply_outcome(
        &self,
        cmd: ApplyOutcomeCommand,
    ) -> Result<ApplyOutcomeResult, String> {
        let source = self
            .sources
            .get_source(cmd.source_account_id)
            .await?
            .ok_or_else(|| "source.not_found: source account not found".to_string())?;
        let rules = self
            .sources
            .get_fetch_rules(cmd.source_account_id)
            .await?
            .ok_or_else(|| "source.fetch_rules_not_found: 抓取规则不存在".to_string())?;

        let now = self.now();
        let decision = decide_outcome(&rules, &cmd, &self.policy, now);

        // 永不改写 first_heartbeat_at
        let event = SourceHeartbeatEvent {
            id: Uuid::new_v4(),
            source_account_id: cmd.source_account_id,
            event_key: cmd.event_key,
            outcome: cmd.outcome,
            level_before: decision.level_before,
            level_after: decision.level_after,
            empty_count_after: decision.consecutive_empty_fetches,
            previous_next_at: Some(rules.next_heartbeat_at),
            next_heartbeat_at: Some(decision.next_heartbeat_at),
            reason_code: decision.reason_code.clone(),
            created_at: now,
        };
        let applied = self
            .sources
            .apply_heartbeat_outcome(crate::repository::source::ApplyHeartbeatOutcome {
                source_account_id: cmd.source_account_id,
                expected_version: source.version,
                rules: ApplyHeartbeatRules {
                    source_account_id: cmd.source_account_id,
                    base_level: decision.base_level,
                    next_heartbeat_at: decision.next_heartbeat_at,
                    consecutive_empty_fetches: decision.consecutive_empty_fetches,
                    consecutive_technical_failures: decision.consecutive_technical_failures,
                    low_activity: decision.low_activity,
                    promotion_until: decision.promotion_until,
                    manual_next_heartbeat_at: decision.manual_next_heartbeat_at,
                    now,
                },
                event,
                source_status: decision.source_status,
            })
            .await?;

        let effective_level = applied.event.level_after;
        Ok(ApplyOutcomeResult {
            event: applied.event,
            rules: applied.rules,
            source_status: decision.source_status,
            effective_level,
        })
    }
}

#[derive(Debug, Clone)]
struct Decision {
    level_before: SourceLevel,
    level_after: SourceLevel,
    base_level: SourceLevel,
    consecutive_empty_fetches: i32,
    consecutive_technical_failures: i32,
    low_activity: bool,
    promotion_until: Option<DateTime<Utc>>,
    manual_next_heartbeat_at: Option<DateTime<Utc>>,
    next_heartbeat_at: DateTime<Utc>,
    reason_code: String,
    source_status: Option<SourceStatus>,
}

/// 纯规则：便于单测覆盖等级表各分支。
fn decide_outcome(
    rules: &AccountFetchRules,
    cmd: &ApplyOutcomeCommand,
    policy: &HeartbeatPolicy,
    now: DateTime<Utc>,
) -> Decision {
    let level_before = effective_level(rules, now);
    let previous_next = rules.next_heartbeat_at;

    match cmd.outcome {
        HeartbeatOutcome::NewArticle => decide_new_article(rules, level_before, policy, now),
        HeartbeatOutcome::EmptySuccess => decide_empty_success(rules, level_before, now),
        HeartbeatOutcome::TechnicalFailure => {
            decide_technical_failure(rules, level_before, policy, now)
        }
        HeartbeatOutcome::AccessBlocked => {
            decide_access_blocked(rules, level_before, previous_next)
        }
        HeartbeatOutcome::Manual => decide_manual(rules, level_before, cmd, policy, now),
    }
}

fn effective_level(rules: &AccountFetchRules, now: DateTime<Utc>) -> SourceLevel {
    if let Some(until) = rules.promotion_until
        && until > now
        && matches!(rules.base_level, SourceLevel::L2 | SourceLevel::L3)
    {
        return SourceLevel::L1;
    }
    rules.base_level
}

fn interval_days_for(
    level: SourceLevel,
    rules: &AccountFetchRules,
    empty_count: i32,
) -> Option<i64> {
    if let Some(override_hours) = rules.interval_override_hours {
        let hours = (override_hours as i64).max(1);
        return Some(((hours + 23) / 24).max(1));
    }
    match level {
        SourceLevel::L4 => None,
        SourceLevel::LO0 | SourceLevel::LO1 => Some(1),
        SourceLevel::L1 => {
            if empty_count >= 2 {
                Some(3)
            } else {
                Some(2)
            }
        }
        SourceLevel::L2 => {
            if empty_count >= 2 {
                Some(10)
            } else {
                Some(7)
            }
        }
        SourceLevel::L3 => Some(30),
    }
}

fn next_from(now: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    now + Duration::days(days)
}

fn decide_new_article(
    rules: &AccountFetchRules,
    level_before: SourceLevel,
    policy: &HeartbeatPolicy,
    now: DateTime<Utc>,
) -> Decision {
    let base_level = rules.base_level;
    let mut promotion_until = rules.promotion_until.filter(|u| *u > now);
    let mut reason = "heartbeat.new_article".to_string();

    if policy.auto_promote_l2_l3
        && rules.auto_promotion_enabled
        && matches!(base_level, SourceLevel::L2 | SourceLevel::L3)
    {
        promotion_until = Some(now + Duration::days(policy.promotion_days));
        reason = "heartbeat.new_article_promoted".into();
    }

    let level_after = if promotion_until.map(|u| u > now).unwrap_or(false)
        && matches!(base_level, SourceLevel::L2 | SourceLevel::L3)
    {
        SourceLevel::L1
    } else {
        base_level
    };

    // L4 仍不参与自动计划：保留 next，但标记 reason
    let next = if base_level == SourceLevel::L4 {
        reason = "heartbeat.new_article_l4_no_plan".into();
        rules.next_heartbeat_at
    } else {
        let days = interval_days_for(level_after, rules, 0).unwrap_or(1);
        next_from(now, days)
    };

    Decision {
        level_before,
        level_after,
        base_level,
        consecutive_empty_fetches: 0,
        consecutive_technical_failures: 0,
        low_activity: false,
        promotion_until,
        manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
        next_heartbeat_at: next,
        reason_code: reason,
        source_status: None,
    }
}

fn decide_empty_success(
    rules: &AccountFetchRules,
    level_before: SourceLevel,
    now: DateTime<Utc>,
) -> Decision {
    let empty = rules.consecutive_empty_fetches + 1;
    let mut base_level = rules.base_level;
    let mut low_activity = rules.low_activity;
    let mut reason = "heartbeat.empty_deferred".to_string();

    // 空抓时若 promotion 仍有效，按有效等级算间隔；到期则回落 base
    let mut working = level_before;
    if let Some(until) = rules.promotion_until
        && until <= now
    {
        working = base_level;
    }

    if !base_level.never_auto_demotes() {
        match base_level {
            SourceLevel::L1 if empty >= 4 => {
                base_level = SourceLevel::L2;
                working = SourceLevel::L2;
                reason = "heartbeat.empty_demote_l2".into();
            }
            SourceLevel::L2 if empty >= 4 => {
                base_level = SourceLevel::L3;
                working = SourceLevel::L3;
                reason = "heartbeat.empty_demote_l3".into();
            }
            SourceLevel::L3 if empty >= 4 => {
                low_activity = true;
                reason = "heartbeat.empty_low_activity".into();
            }
            _ => {
                if empty >= 2 {
                    reason = "heartbeat.empty_extended".into();
                }
            }
        }
    } else {
        reason = "heartbeat.empty_no_demote".into();
    }

    let promotion_until = if matches!(base_level, SourceLevel::L2 | SourceLevel::L3) {
        // 降级后清除提升
        if reason.starts_with("heartbeat.empty_demote") {
            None
        } else {
            rules.promotion_until.filter(|u| *u > now)
        }
    } else {
        rules.promotion_until.filter(|u| *u > now)
    };

    let level_after = if promotion_until.map(|u| u > now).unwrap_or(false)
        && matches!(base_level, SourceLevel::L2 | SourceLevel::L3)
    {
        SourceLevel::L1
    } else {
        working
    };

    let next = if base_level == SourceLevel::L4 {
        reason = "heartbeat.empty_l4_no_plan".into();
        rules.next_heartbeat_at
    } else {
        let days = interval_days_for(level_after, rules, empty).unwrap_or(1);
        next_from(now, days)
    };

    Decision {
        level_before,
        level_after,
        base_level,
        consecutive_empty_fetches: empty,
        consecutive_technical_failures: rules.consecutive_technical_failures,
        low_activity,
        promotion_until,
        manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
        next_heartbeat_at: next,
        reason_code: reason,
        source_status: None,
    }
}

fn decide_technical_failure(
    rules: &AccountFetchRules,
    level_before: SourceLevel,
    policy: &HeartbeatPolicy,
    now: DateTime<Utc>,
) -> Decision {
    let failures = rules.consecutive_technical_failures + 1;
    let exhausted = failures >= policy.max_technical_failures;

    let (next, reason, status) = if exhausted {
        (
            rules.next_heartbeat_at,
            "heartbeat.technical_exhausted".to_string(),
            Some(SourceStatus::NeedsAction),
        )
    } else {
        // 指数退避：2^n 小时，不计空抓
        let hours = 1i64 << failures.min(6);
        (
            now + Duration::hours(hours),
            format!("heartbeat.technical_backoff_{hours}h"),
            None,
        )
    };

    Decision {
        level_before,
        level_after: level_before,
        base_level: rules.base_level,
        consecutive_empty_fetches: rules.consecutive_empty_fetches,
        consecutive_technical_failures: failures,
        low_activity: rules.low_activity,
        promotion_until: rules.promotion_until,
        manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
        next_heartbeat_at: next,
        reason_code: reason,
        source_status: status,
    }
}

fn decide_access_blocked(
    rules: &AccountFetchRules,
    level_before: SourceLevel,
    previous_next: DateTime<Utc>,
) -> Decision {
    Decision {
        level_before,
        level_after: level_before,
        base_level: rules.base_level,
        consecutive_empty_fetches: rules.consecutive_empty_fetches,
        consecutive_technical_failures: rules.consecutive_technical_failures,
        low_activity: rules.low_activity,
        promotion_until: rules.promotion_until,
        manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
        next_heartbeat_at: previous_next,
        reason_code: "heartbeat.access_blocked".into(),
        source_status: Some(SourceStatus::NeedsAction),
    }
}

fn decide_manual(
    rules: &AccountFetchRules,
    level_before: SourceLevel,
    cmd: &ApplyOutcomeCommand,
    policy: &HeartbeatPolicy,
    now: DateTime<Utc>,
) -> Decision {
    // 手动抓取永不改 first_heartbeat_at（由 apply 层保证）
    if cmd.manual_found_new {
        let mut d = decide_new_article(rules, level_before, policy, now);
        if !cmd.reset_empty_on_manual_new {
            d.consecutive_empty_fetches = rules.consecutive_empty_fetches;
        }
        if !cmd.recalculate_next {
            d.next_heartbeat_at = rules.next_heartbeat_at;
            d.reason_code = "heartbeat.manual_new_keep_next".into();
        } else {
            d.reason_code = "heartbeat.manual_new_recalc".into();
        }
        d
    } else {
        // 手动空结果：不计自动空抓路径，除非显式要求重算
        let mut next = rules.next_heartbeat_at;
        let mut reason = "heartbeat.manual_keep_next".to_string();
        if cmd.recalculate_next {
            let days = interval_days_for(level_before, rules, rules.consecutive_empty_fetches)
                .unwrap_or(1);
            next = next_from(now, days);
            reason = "heartbeat.manual_recalc".into();
        }
        Decision {
            level_before,
            level_after: level_before,
            base_level: rules.base_level,
            consecutive_empty_fetches: rules.consecutive_empty_fetches,
            consecutive_technical_failures: rules.consecutive_technical_failures,
            low_activity: rules.low_activity,
            promotion_until: rules.promotion_until,
            manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
            next_heartbeat_at: next,
            reason_code: reason,
            source_status: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::WorkingWindow;
    use crate::domain::source::SourceStatus;
    use crate::repository::source::{
        ApplyHeartbeatOutcome, ApplyHeartbeatRules, CreateSourceAccount, UpsertFetchRules,
    };
    use crate::repository::source_memory::MemorySourceStore;
    use server_core::FrozenClock;
    use std::time::Duration as StdDuration;

    fn frozen_now() -> (Arc<FrozenClock>, DateTime<Utc>) {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let now = DateTime::<Utc>::from(clock.now());
        (clock, now)
    }

    fn base_rules(level: SourceLevel, now: DateTime<Utc>, empty: i32) -> AccountFetchRules {
        AccountFetchRules {
            auto_promotion_enabled: true,
            source_account_id: Uuid::nil(),
            base_level: level,
            connector_override: None,
            enabled: true,
            first_heartbeat_at: now,
            next_heartbeat_at: now,
            working_windows: vec![WorkingWindow {
                start: "09:00".into(),
                end: "18:00".into(),
            }],
            interval_override_hours: None,
            consecutive_empty_fetches: empty,
            consecutive_technical_failures: 0,
            low_activity: false,
            promotion_until: None,
            manual_next_heartbeat_at: None,
            updated_at: now,
        }
    }

    fn cmd(outcome: HeartbeatOutcome) -> ApplyOutcomeCommand {
        ApplyOutcomeCommand {
            source_account_id: Uuid::nil(),
            event_key: format!("test-{}", Uuid::new_v4()),
            outcome,
            recalculate_next: true,
            reset_empty_on_manual_new: true,
            manual_found_new: false,
        }
    }

    #[test]
    fn empty_success_defers_l1_then_demotes() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();

        // 第 1 次空抓：间隔仍 2 天
        let r = base_rules(SourceLevel::L1, now, 0);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert_eq!(d.consecutive_empty_fetches, 1);
        assert_eq!(d.base_level, SourceLevel::L1);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(2));

        // 第 2 次：延至 3 天
        let r = base_rules(SourceLevel::L1, now, 1);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert_eq!(d.consecutive_empty_fetches, 2);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(3));
        assert_eq!(d.reason_code, "heartbeat.empty_extended");

        // 第 4 次：转 L2
        let r = base_rules(SourceLevel::L1, now, 3);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert_eq!(d.base_level, SourceLevel::L2);
        assert_eq!(d.level_after, SourceLevel::L2);
        assert_eq!(d.reason_code, "heartbeat.empty_demote_l2");
    }

    #[test]
    fn lo0_lo1_never_auto_demote() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        for level in [SourceLevel::LO0, SourceLevel::LO1] {
            let r = base_rules(level, now, 10);
            let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
            assert_eq!(d.base_level, level);
            assert_eq!(d.level_after, level);
            assert_eq!(d.reason_code, "heartbeat.empty_no_demote");
            assert_eq!(d.next_heartbeat_at, now + Duration::days(1));
        }
    }

    #[test]
    fn l2_empty_extends_then_demotes_to_l3() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();

        let r = base_rules(SourceLevel::L2, now, 1);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(10));

        let r = base_rules(SourceLevel::L2, now, 3);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert_eq!(d.base_level, SourceLevel::L3);
        assert_eq!(d.reason_code, "heartbeat.empty_demote_l3");
    }

    #[test]
    fn l3_empty_marks_low_activity() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let r = base_rules(SourceLevel::L3, now, 3);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::EmptySuccess), &policy, now);
        assert!(d.low_activity);
        assert_eq!(d.base_level, SourceLevel::L3);
        assert_eq!(d.reason_code, "heartbeat.empty_low_activity");
    }

    #[test]
    fn new_article_clears_empty_and_promotes_l2() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let mut r = base_rules(SourceLevel::L2, now, 5);
        r.consecutive_technical_failures = 2;
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::NewArticle), &policy, now);
        assert_eq!(d.consecutive_empty_fetches, 0);
        assert_eq!(d.consecutive_technical_failures, 0);
        assert_eq!(d.level_after, SourceLevel::L1);
        assert_eq!(d.base_level, SourceLevel::L2);
        assert!(d.promotion_until.unwrap() > now);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(2)); // L1 基础间隔
    }

    #[test]
    fn new_article_promotion_can_be_disabled() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy {
            auto_promote_l2_l3: false,
            ..Default::default()
        };
        let r = base_rules(SourceLevel::L3, now, 1);
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::NewArticle), &policy, now);
        assert_eq!(d.level_after, SourceLevel::L3);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(30));
    }

    #[test]
    fn source_rule_can_disable_new_article_promotion() {
        let (_, now) = frozen_now();
        let mut rules = base_rules(SourceLevel::L3, now, 1);
        rules.auto_promotion_enabled = false;
        let decision = decide_outcome(
            &rules,
            &cmd(HeartbeatOutcome::NewArticle),
            &HeartbeatPolicy::default(),
            now,
        );
        assert_eq!(decision.level_after, SourceLevel::L3);
        assert_eq!(decision.next_heartbeat_at, now + Duration::days(30));
    }

    #[test]
    fn technical_failure_backoff_does_not_count_empty() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let mut r = base_rules(SourceLevel::L1, now, 2);
        r.consecutive_technical_failures = 0;
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::TechnicalFailure), &policy, now);
        assert_eq!(d.consecutive_empty_fetches, 2);
        assert_eq!(d.consecutive_technical_failures, 1);
        assert_eq!(d.next_heartbeat_at, now + Duration::hours(2));
        assert!(d.source_status.is_none());
    }

    #[test]
    fn technical_failure_exhausted_needs_action() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy {
            max_technical_failures: 3,
            ..Default::default()
        };
        let mut r = base_rules(SourceLevel::L1, now, 1);
        r.consecutive_technical_failures = 2;
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::TechnicalFailure), &policy, now);
        assert_eq!(d.consecutive_technical_failures, 3);
        assert_eq!(d.consecutive_empty_fetches, 1);
        assert_eq!(d.source_status, Some(SourceStatus::NeedsAction));
        assert_eq!(d.reason_code, "heartbeat.technical_exhausted");
    }

    #[test]
    fn access_blocked_needs_action_keeps_counts() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let mut r = base_rules(SourceLevel::L1, now, 3);
        r.consecutive_technical_failures = 1;
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::AccessBlocked), &policy, now);
        assert_eq!(d.source_status, Some(SourceStatus::NeedsAction));
        assert_eq!(d.consecutive_empty_fetches, 3);
        assert_eq!(d.base_level, SourceLevel::L1);
    }

    #[test]
    fn l4_does_not_auto_plan_interval() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let r = base_rules(SourceLevel::L4, now, 0);
        assert!(!SourceLevel::L4.participates_in_auto_plan());
        let d = decide_outcome(&r, &cmd(HeartbeatOutcome::NewArticle), &policy, now);
        assert_eq!(d.next_heartbeat_at, now);
        assert_eq!(d.reason_code, "heartbeat.new_article_l4_no_plan");
    }

    #[test]
    fn manual_keep_next_does_not_change_first_anchor() {
        let (_, now) = frozen_now();
        let policy = HeartbeatPolicy::default();
        let first = now - Duration::days(10);
        let mut r = base_rules(SourceLevel::L1, now, 2);
        r.first_heartbeat_at = first;
        r.next_heartbeat_at = now + Duration::days(5);
        let mut c = cmd(HeartbeatOutcome::Manual);
        c.recalculate_next = false;
        c.manual_found_new = true;
        let d = decide_outcome(&r, &c, &policy, now);
        assert_eq!(d.next_heartbeat_at, now + Duration::days(5));
        assert_eq!(d.consecutive_empty_fetches, 0);
        assert_eq!(d.reason_code, "heartbeat.manual_new_keep_next");
    }

    #[tokio::test]
    async fn apply_outcome_persists_event_and_never_touches_first() {
        let (clock, now) = frozen_now();
        let store = Arc::new(MemorySourceStore::new());
        let sid = Uuid::new_v4();
        store
            .insert_source(CreateSourceAccount {
                id: sid,
                display_name: "t".into(),
                status: SourceStatus::Active,
                preferred_connector: None,
                now,
            })
            .await
            .unwrap();
        let first = now - Duration::days(3);
        store
            .upsert_fetch_rules(UpsertFetchRules {
                source_account_id: sid,
                base_level: SourceLevel::L1,
                working_windows: vec![WorkingWindow {
                    start: "09:00".into(),
                    end: "18:00".into(),
                }],
                first_heartbeat_at: first,
                next_heartbeat_at: now,
                connector_override: None,
                now,
            })
            .await
            .unwrap();

        let svc = HeartbeatService::new(store.clone() as Arc<dyn SourceRepository>, clock);
        let result = svc
            .apply_outcome(ApplyOutcomeCommand {
                source_account_id: sid,
                event_key: "apply-outcome-test".into(),
                outcome: HeartbeatOutcome::EmptySuccess,
                recalculate_next: true,
                reset_empty_on_manual_new: true,
                manual_found_new: false,
            })
            .await
            .unwrap();

        assert_eq!(result.rules.first_heartbeat_at, first);
        assert_eq!(result.rules.consecutive_empty_fetches, 1);
        assert_eq!(result.event.level_before, SourceLevel::L1);
        assert_eq!(result.event.reason_code, "heartbeat.empty_deferred");

        let events = store.list_heartbeat_events(sid, 10).await.unwrap();
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn apply_outcome_replay_is_idempotent_and_stale_version_is_rejected() {
        let (clock, now) = frozen_now();
        let store = Arc::new(MemorySourceStore::new());
        let sid = Uuid::new_v4();
        store
            .insert_source(CreateSourceAccount {
                id: sid,
                display_name: "t".into(),
                status: SourceStatus::Active,
                preferred_connector: None,
                now,
            })
            .await
            .unwrap();
        store
            .upsert_fetch_rules(UpsertFetchRules {
                source_account_id: sid,
                base_level: SourceLevel::L1,
                working_windows: vec![],
                first_heartbeat_at: now,
                next_heartbeat_at: now,
                connector_override: None,
                now,
            })
            .await
            .unwrap();

        let svc = HeartbeatService::new(store.clone() as Arc<dyn SourceRepository>, clock);
        let request = ApplyOutcomeCommand {
            source_account_id: sid,
            event_key: "fetch-job-1".into(),
            outcome: HeartbeatOutcome::EmptySuccess,
            recalculate_next: true,
            reset_empty_on_manual_new: true,
            manual_found_new: false,
        };
        let first = svc.apply_outcome(request.clone()).await.unwrap();
        let replay = svc.apply_outcome(request).await.unwrap();
        assert_eq!(first.event.id, replay.event.id);
        assert_eq!(replay.rules.consecutive_empty_fetches, 1);
        assert_eq!(store.list_heartbeat_events(sid, 10).await.unwrap().len(), 1);
        assert_eq!(store.get_source(sid).await.unwrap().unwrap().version, 2);

        let rules = store.get_fetch_rules(sid).await.unwrap().unwrap();
        let stale_event = SourceHeartbeatEvent {
            id: Uuid::new_v4(),
            source_account_id: sid,
            event_key: "fetch-job-2".into(),
            outcome: HeartbeatOutcome::EmptySuccess,
            level_before: SourceLevel::L1,
            level_after: SourceLevel::L1,
            empty_count_after: 2,
            previous_next_at: Some(rules.next_heartbeat_at),
            next_heartbeat_at: Some(rules.next_heartbeat_at),
            reason_code: "test.stale".into(),
            created_at: now,
        };
        let err = store
            .apply_heartbeat_outcome(ApplyHeartbeatOutcome {
                source_account_id: sid,
                expected_version: 1,
                rules: ApplyHeartbeatRules {
                    source_account_id: sid,
                    base_level: rules.base_level,
                    next_heartbeat_at: rules.next_heartbeat_at,
                    consecutive_empty_fetches: rules.consecutive_empty_fetches,
                    consecutive_technical_failures: rules.consecutive_technical_failures,
                    low_activity: rules.low_activity,
                    promotion_until: rules.promotion_until,
                    manual_next_heartbeat_at: rules.manual_next_heartbeat_at,
                    now,
                },
                event: stale_event,
                source_status: None,
            })
            .await
            .unwrap_err();
        assert!(err.contains("source.version_conflict"));
        assert_eq!(store.list_heartbeat_events(sid, 10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn frozen_clock_advance_affects_next() {
        let clock = Arc::new(FrozenClock::at_unix_millis(1_700_000_000_000));
        let store = Arc::new(MemorySourceStore::new());
        let sid = Uuid::new_v4();
        let t0 = DateTime::<Utc>::from(clock.now());
        store
            .insert_source(CreateSourceAccount {
                id: sid,
                display_name: "t".into(),
                status: SourceStatus::Active,
                preferred_connector: None,
                now: t0,
            })
            .await
            .unwrap();
        store
            .upsert_fetch_rules(UpsertFetchRules {
                source_account_id: sid,
                base_level: SourceLevel::LO0,
                working_windows: vec![WorkingWindow {
                    start: "09:00".into(),
                    end: "18:00".into(),
                }],
                first_heartbeat_at: t0,
                next_heartbeat_at: t0,
                connector_override: None,
                now: t0,
            })
            .await
            .unwrap();

        let svc = HeartbeatService::new(
            store.clone() as Arc<dyn SourceRepository>,
            clock.clone() as Arc<dyn Clock>,
        );
        let r1 = svc
            .apply_outcome(ApplyOutcomeCommand {
                source_account_id: sid,
                event_key: "frozen-new".into(),
                outcome: HeartbeatOutcome::NewArticle,
                recalculate_next: true,
                reset_empty_on_manual_new: true,
                manual_found_new: false,
            })
            .await
            .unwrap();
        assert_eq!(r1.rules.next_heartbeat_at, t0 + Duration::days(1));

        clock.advance(StdDuration::from_secs(86_400));
        let t1 = DateTime::<Utc>::from(clock.now());
        let r2 = svc
            .apply_outcome(ApplyOutcomeCommand {
                source_account_id: sid,
                event_key: "frozen-empty".into(),
                outcome: HeartbeatOutcome::EmptySuccess,
                recalculate_next: true,
                reset_empty_on_manual_new: true,
                manual_found_new: false,
            })
            .await
            .unwrap();
        assert_eq!(r2.rules.next_heartbeat_at, t1 + Duration::days(1));
        assert_eq!(r2.rules.first_heartbeat_at, t0);
    }
}
