//! 预检结果过滤与业务时间窗口决策。

use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use uuid::Uuid;

use crate::domain::settings::ScheduleSettings;

use crate::service::daily_preflight::ComponentCheckResult;

pub(crate) fn filter_immediate(
    results: Vec<ComponentCheckResult>,
    target_type: Option<&str>,
    target_id: Option<Uuid>,
) -> Vec<ComponentCheckResult> {
    let Some(tt) = target_type else {
        return results;
    };
    if tt == "all" {
        return results;
    }
    results
        .into_iter()
        .filter(|r| {
            r.component_type.as_str() == tt && (target_id.is_none() || r.component_id == target_id)
        })
        .collect()
}

/// 无 chrono-tz：Asia/Shanghai 等东八区用 +8h；其余用 UTC 日历日。
pub(crate) fn business_date_for(now: DateTime<Utc>, timezone: &str) -> NaiveDate {
    let offset_hours = match timezone {
        "Asia/Shanghai" | "Asia/Chongqing" | "Asia/Hong_Kong" | "Asia/Taipei" => 8,
        "UTC" | "Etc/UTC" => 0,
        _ => 8,
    };
    let local = now + chrono::Duration::hours(offset_hours);
    NaiveDate::from_ymd_opt(local.year(), local.month(), local.day()).expect("valid date")
}

pub(crate) fn in_preflight_window(now: DateTime<Utc>, schedule: &ScheduleSettings) -> bool {
    let date = business_date_for(now, &schedule.timezone);
    let (ph, pm) = parse_hhmm(&schedule.preflight_time).unwrap_or((7, 30));
    let offset_hours = match schedule.timezone.as_str() {
        "UTC" | "Etc/UTC" => 0,
        _ => 8,
    };
    let local = now + chrono::Duration::hours(offset_hours);
    if local.date_naive() != date {
        return false;
    }
    let mins = local.hour() as i32 * 60 + local.minute() as i32;
    let start = ph as i32 * 60 + pm as i32;
    mins >= start && mins < start + 15
}

pub(crate) fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(':');
    let h = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    Some((h, m))
}
