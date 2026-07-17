//! SlotAllocator：工作窗口内加权随机抽样，禁止等间隔排程。

use chrono::{DateTime, Duration, NaiveDate, Utc};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use uuid::Uuid;

use crate::domain::settings::WorkingWindow;
use crate::service::daily_preflight::business_date_for;

/// 分配约束。
#[derive(Debug, Clone)]
pub struct SlotAllocatorConfig {
    /// 同一采集账号两次任务最小间隔（秒）。
    pub min_collector_gap_secs: i64,
    /// 全局最大并发（分桶容量近似）。
    pub global_concurrency: usize,
    /// 单任务预估占用分钟（用于容量桶）。
    pub slot_minutes: i64,
}

impl Default for SlotAllocatorConfig {
    fn default() -> Self {
        Self {
            min_collector_gap_secs: 120,
            global_concurrency: 4,
            slot_minutes: 5,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlotRequest {
    pub source_account_id: Uuid,
    pub collector_account_id: Uuid,
    /// 权重：越大越优先占桶（默认 1）。
    pub weight: f64,
}

#[derive(Debug, Clone)]
pub struct SlotAssignment {
    pub source_account_id: Uuid,
    pub collector_account_id: Uuid,
    pub planned_run_at: DateTime<Utc>,
}

/// 在工作窗口内按容量分桶后加权随机抽样时间点。
pub struct SlotAllocator {
    config: SlotAllocatorConfig,
    rng: StdRng,
}

impl SlotAllocator {
    pub fn new(config: SlotAllocatorConfig) -> Self {
        Self {
            config,
            rng: StdRng::from_entropy(),
        }
    }

    pub fn with_seed(config: SlotAllocatorConfig, seed: u64) -> Self {
        Self {
            config,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// 将请求分配到 `plan_date` 的工作窗口内。
    pub fn allocate(
        &mut self,
        plan_date: NaiveDate,
        timezone: &str,
        windows: &[WorkingWindow],
        requests: &[SlotRequest],
    ) -> Result<Vec<SlotAssignment>, String> {
        if requests.is_empty() {
            return Ok(vec![]);
        }
        let ranges = expand_window_ranges(plan_date, timezone, windows)?;
        if ranges.is_empty() {
            return Err("daily_plan.no_working_window: 无有效工作窗口".into());
        }

        let buckets = build_capacity_buckets(&ranges, self.config.slot_minutes);
        if buckets.is_empty() {
            return Err("daily_plan.no_capacity: 工作窗口容量为空".into());
        }

        // 按权重降序，再在同权内打乱，避免固定序号等距。
        let mut order: Vec<usize> = (0..requests.len()).collect();
        // Fisher–Yates
        for i in (1..order.len()).rev() {
            let j = self.rng.gen_range(0..=i);
            order.swap(i, j);
        }
        order.sort_by(|&a, &b| {
            requests[b]
                .weight
                .partial_cmp(&requests[a].weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut assignments = Vec::with_capacity(requests.len());
        let mut collector_last: std::collections::HashMap<Uuid, DateTime<Utc>> =
            std::collections::HashMap::new();
        // 每桶已用并发
        let mut bucket_load: Vec<usize> = vec![0; buckets.len()];

        for idx in order {
            let req = &requests[idx];
            let mut candidates: Vec<(usize, DateTime<Utc>)> = Vec::new();

            for (bi, bucket) in buckets.iter().enumerate() {
                if bucket_load[bi] >= self.config.global_concurrency {
                    continue;
                }
                // 桶内随机秒偏移（非等间隔）
                let span = (bucket.end - bucket.start).num_seconds().max(1);
                let offset = self.rng.gen_range(0..span);
                let candidate = bucket.start + Duration::seconds(offset);

                if let Some(last) = collector_last.get(&req.collector_account_id)
                    && candidate < *last + Duration::seconds(self.config.min_collector_gap_secs)
                {
                    // 尝试推到 gap 之后仍在桶内
                    let pushed = *last + Duration::seconds(self.config.min_collector_gap_secs);
                    if pushed >= bucket.start && pushed < bucket.end {
                        candidates.push((bi, pushed));
                    }
                    continue;
                }
                candidates.push((bi, candidate));
            }

            if candidates.is_empty() {
                // 回退：在全部窗口内随机，仅尊重 collector gap
                let range = &ranges[self.rng.gen_range(0..ranges.len())];
                let span = (range.end - range.start).num_seconds().max(1);
                let mut t = range.start + Duration::seconds(self.rng.gen_range(0..span));
                if let Some(last) = collector_last.get(&req.collector_account_id) {
                    let min_t = *last + Duration::seconds(self.config.min_collector_gap_secs);
                    if t < min_t && min_t < range.end {
                        t = min_t;
                    }
                }
                collector_last.insert(req.collector_account_id, t);
                assignments.push(SlotAssignment {
                    source_account_id: req.source_account_id,
                    collector_account_id: req.collector_account_id,
                    planned_run_at: t,
                });
                continue;
            }

            let pick = candidates[self.rng.gen_range(0..candidates.len())];
            bucket_load[pick.0] += 1;
            collector_last.insert(req.collector_account_id, pick.1);
            assignments.push(SlotAssignment {
                source_account_id: req.source_account_id,
                collector_account_id: req.collector_account_id,
                planned_run_at: pick.1,
            });
        }

        assignments.sort_by_key(|a| a.planned_run_at);
        Ok(assignments)
    }
}

#[derive(Debug, Clone)]
struct TimeRange {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct CapacityBucket {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

fn expand_window_ranges(
    plan_date: NaiveDate,
    timezone: &str,
    windows: &[WorkingWindow],
) -> Result<Vec<TimeRange>, String> {
    let offset_hours = match timezone {
        "UTC" | "Etc/UTC" => 0,
        _ => 8,
    };
    let mut ranges = Vec::new();
    for w in windows {
        let (sh, sm) = parse_hhmm(&w.start)
            .ok_or_else(|| format!("daily_plan.bad_window_start: {}", w.start))?;
        let (eh, em) =
            parse_hhmm(&w.end).ok_or_else(|| format!("daily_plan.bad_window_end: {}", w.end))?;
        let start_local = plan_date
            .and_hms_opt(sh, sm, 0)
            .ok_or_else(|| "daily_plan.bad_start".to_string())?;
        let end_local = plan_date
            .and_hms_opt(eh, em, 0)
            .ok_or_else(|| "daily_plan.bad_end".to_string())?;
        if end_local <= start_local {
            return Err("daily_plan.window_inverted: 窗口结束须晚于开始".into());
        }
        let start = (start_local - chrono::Duration::hours(offset_hours)).and_utc();
        let end = (end_local - chrono::Duration::hours(offset_hours)).and_utc();
        // 校验日历日一致（业务日）
        let _ = business_date_for(start, timezone);
        ranges.push(TimeRange { start, end });
    }
    Ok(ranges)
}

fn build_capacity_buckets(ranges: &[TimeRange], slot_minutes: i64) -> Vec<CapacityBucket> {
    let step = Duration::minutes(slot_minutes.max(1));
    let mut buckets = Vec::new();
    for r in ranges {
        let mut cursor = r.start;
        while cursor < r.end {
            let next = (cursor + step).min(r.end);
            if next > cursor {
                buckets.push(CapacityBucket {
                    start: cursor,
                    end: next,
                });
            }
            cursor = next;
        }
    }
    buckets
}

fn parse_hhmm(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.split(':');
    let h = parts.next()?.parse().ok()?;
    let m = parts.next()?.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some((h, m))
}

/// 断言一组时间戳不是等间隔（允许极小浮点误差）。
pub fn assert_not_equal_intervals(times: &[DateTime<Utc>]) -> bool {
    if times.len() < 3 {
        return true; // 样本不足时不判定为等间隔
    }
    let mut sorted = times.to_vec();
    sorted.sort();
    let gaps: Vec<i64> = sorted
        .windows(2)
        .map(|w| (w[1] - w[0]).num_seconds())
        .collect();
    let first = gaps[0];
    // 全部 gap 相同（容差 1 秒）→ 等间隔
    !gaps.iter().all(|g| (*g - first).abs() <= 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::WorkingWindow;
    use chrono::Timelike;

    #[test]
    fn allocate_not_equal_interval() {
        let windows = vec![WorkingWindow {
            start: "09:00".into(),
            end: "18:00".into(),
        }];
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let mut alloc = SlotAllocator::with_seed(SlotAllocatorConfig::default(), 42);
        let collector = Uuid::new_v4();
        let requests: Vec<_> = (0..12)
            .map(|_| SlotRequest {
                source_account_id: Uuid::new_v4(),
                collector_account_id: collector,
                weight: 1.0,
            })
            .collect();
        // 放宽 gap 以便能排下
        alloc.config.min_collector_gap_secs = 30;
        let out = alloc
            .allocate(date, "Asia/Shanghai", &windows, &requests)
            .unwrap();
        assert_eq!(out.len(), 12);
        let times: Vec<_> = out.iter().map(|a| a.planned_run_at).collect();
        assert!(
            assert_not_equal_intervals(&times),
            "分配结果不得为等间隔: {:?}",
            times
                .windows(2)
                .map(|w| (w[1] - w[0]).num_seconds())
                .collect::<Vec<_>>()
        );
        // 同一 collector 满足最小间隔
        for w in out.windows(2) {
            if w[0].collector_account_id == w[1].collector_account_id {
                assert!(
                    (w[1].planned_run_at - w[0].planned_run_at).num_seconds()
                        >= alloc.config.min_collector_gap_secs - 1
                );
            }
        }
    }

    #[test]
    fn equal_interval_detector() {
        let t0 = Utc::now();
        let equal = vec![
            t0,
            t0 + Duration::seconds(60),
            t0 + Duration::seconds(120),
            t0 + Duration::seconds(180),
        ];
        assert!(!assert_not_equal_intervals(&equal));
        let uneven = vec![
            t0,
            t0 + Duration::seconds(10),
            t0 + Duration::seconds(100),
            t0 + Duration::seconds(130),
        ];
        assert!(assert_not_equal_intervals(&uneven));
    }

    #[test]
    fn times_fall_in_window() {
        let windows = vec![WorkingWindow {
            start: "09:00".into(),
            end: "12:00".into(),
        }];
        let date = NaiveDate::from_ymd_opt(2023, 11, 18).unwrap();
        let mut alloc = SlotAllocator::with_seed(SlotAllocatorConfig::default(), 7);
        let requests: Vec<_> = (0..5)
            .map(|_| SlotRequest {
                source_account_id: Uuid::new_v4(),
                collector_account_id: Uuid::new_v4(),
                weight: 1.0,
            })
            .collect();
        let out = alloc
            .allocate(date, "Asia/Shanghai", &windows, &requests)
            .unwrap();
        for a in &out {
            let local = a.planned_run_at + Duration::hours(8);
            assert_eq!(local.date_naive(), date);
            let mins = local.hour() * 60 + local.minute();
            assert!((9 * 60..12 * 60).contains(&mins));
        }
    }
}
