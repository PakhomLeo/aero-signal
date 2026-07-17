//! Daily plan 的纯映射与时间决策函数。
//!
//! 这些函数不访问仓储或外部连接器，单独放置后可在不触碰服务编排的情况下回归。

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};

use crate::domain::daily_plan::DailyFetchPlan;
use crate::domain::settings::ScheduleSettings;
use crate::domain::source::SourceLevel;
use crate::service::daily_preflight::business_date_for;

use super::DailyPlanRowDto;

pub(crate) fn plan_to_dto(p: DailyFetchPlan) -> DailyPlanRowDto {
    DailyPlanRowDto {
        id: p.id,
        plan_date: p.plan_date.format("%Y-%m-%d").to_string(),
        source_account_id: p.source_account_id,
        planned_run_at: p.planned_run_at,
        status: p.status.as_str().to_string(),
        reason_code: p.reason_code,
        fetch_job_id: p.fetch_job_id,
        collector_account_id: p.collector_account_id,
    }
}

pub(crate) fn effective_level(
    base: &SourceLevel,
    promotion_until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> SourceLevel {
    if let Some(until) = promotion_until
        && until > now
        && matches!(base, SourceLevel::L2 | SourceLevel::L3)
    {
        return SourceLevel::L1;
    }
    *base
}

/// 业务日结束（本地 23:59:59 → UTC）。
pub(crate) fn day_end_utc(date: NaiveDate, timezone: &str) -> DateTime<Utc> {
    let offset_hours = match timezone {
        "UTC" | "Etc/UTC" => 0,
        _ => 8,
    };
    let local = date.and_hms_opt(23, 59, 59).expect("valid end of day");
    (local - Duration::hours(offset_hours)).and_utc()
}

/// 08:00 起 15 分钟计划入队窗口。
pub(crate) fn in_plan_start_window(now: DateTime<Utc>, schedule: &ScheduleSettings) -> bool {
    let date = business_date_for(now, &schedule.timezone);
    let (ph, pm) = parse_plan_hhmm(&schedule.plan_start_time).unwrap_or((8, 0));
    let offset_hours = match schedule.timezone.as_str() {
        "UTC" | "Etc/UTC" => 0,
        _ => 8,
    };
    let local = now + Duration::hours(offset_hours);
    if local.date_naive() != date {
        return false;
    }
    let mins = local.hour() as i32 * 60 + local.minute() as i32;
    let start = ph as i32 * 60 + pm as i32;
    mins >= start && mins < start + 15
}

pub(crate) fn parse_plan_hhmm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(':');
    let h = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    Some((h, m))
}
