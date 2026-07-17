//! 本地请求令牌桶：按 collector、egress、source、host 与 endpoint 共同约束。

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestPurpose {
    List,
    Detail,
    Media,
    Search,
}

impl RequestPurpose {
    fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Detail => "detail",
            Self::Media => "media",
            Self::Search => "search",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestLimitContext {
    pub collector_id: Option<Uuid>,
    pub egress_id: Option<Uuid>,
    pub source_id: Uuid,
    pub host: String,
    pub purpose: RequestPurpose,
}

#[derive(Debug, Clone, Copy)]
pub struct RequestRateConfig {
    pub capacity: u32,
    pub refill_per_second: f64,
}

impl Default for RequestRateConfig {
    fn default() -> Self {
        Self {
            // 正常一篇文章的正文图片可在同一 burst 内完成；批量任务会被约束。
            capacity: 20,
            refill_per_second: 2.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestLimitError {
    pub code: &'static str,
    pub retry_after_ms: u64,
}

#[derive(Debug, Clone)]
struct Bucket {
    tokens: f64,
    updated_at: DateTime<Utc>,
}

pub struct RequestRateLimiter {
    config: RequestRateConfig,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RequestRateLimiter {
    pub fn new(config: RequestRateConfig) -> Self {
        Self {
            config: RequestRateConfig {
                capacity: config.capacity.max(1),
                refill_per_second: config.refill_per_second.max(0.001),
            },
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn acquire(
        &self,
        context: &RequestLimitContext,
        now: DateTime<Utc>,
    ) -> Result<(), RequestLimitError> {
        let purpose = context.purpose.as_str();
        let mut keys = vec![
            format!("source:{}:{purpose}", context.source_id),
            format!("host:{}:{purpose}", context.host.to_ascii_lowercase()),
        ];
        if let Some(collector_id) = context.collector_id {
            keys.push(format!("collector:{collector_id}:{purpose}"));
        }
        if let Some(egress_id) = context.egress_id {
            keys.push(format!("egress:{egress_id}:{purpose}"));
        }

        let capacity = f64::from(self.config.capacity);
        let mut guard = self.buckets.lock().expect("request rate buckets");
        let mut retry_after_ms = 0_u64;
        for key in &keys {
            let bucket = guard.entry(key.clone()).or_insert(Bucket {
                tokens: capacity,
                updated_at: now,
            });
            let elapsed_ms = (now - bucket.updated_at).num_milliseconds().max(0) as f64;
            bucket.tokens =
                (bucket.tokens + elapsed_ms / 1000.0 * self.config.refill_per_second).min(capacity);
            bucket.updated_at = now;
            if bucket.tokens < 1.0 {
                let missing = 1.0 - bucket.tokens;
                retry_after_ms = retry_after_ms
                    .max((missing / self.config.refill_per_second * 1000.0).ceil() as u64);
            }
        }
        if retry_after_ms > 0 {
            return Err(RequestLimitError {
                code: "crawler.local_rate_limited",
                retry_after_ms,
            });
        }
        for key in keys {
            if let Some(bucket) = guard.get_mut(&key) {
                bucket.tokens -= 1.0;
            }
        }
        Ok(())
    }
}

impl Default for RequestRateLimiter {
    fn default() -> Self {
        Self::new(RequestRateConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone};

    use super::*;

    fn context() -> RequestLimitContext {
        RequestLimitContext {
            collector_id: Some(Uuid::nil()),
            egress_id: Some(Uuid::from_u128(1)),
            source_id: Uuid::from_u128(2),
            host: "mp.weixin.qq.com".into(),
            purpose: RequestPurpose::Detail,
        }
    }

    #[test]
    fn shared_dimensions_exhaust_and_refill_without_sleeping() {
        let limiter = RequestRateLimiter::new(RequestRateConfig {
            capacity: 2,
            refill_per_second: 1.0,
        });
        let now = Utc.with_ymd_and_hms(2026, 7, 17, 8, 0, 0).unwrap();
        assert!(limiter.acquire(&context(), now).is_ok());
        assert!(limiter.acquire(&context(), now).is_ok());
        let limited = limiter.acquire(&context(), now).unwrap_err();
        assert_eq!(limited.code, "crawler.local_rate_limited");
        assert_eq!(limited.retry_after_ms, 1000);
        assert!(
            limiter
                .acquire(&context(), now + Duration::seconds(1))
                .is_ok()
        );
    }

    #[test]
    fn endpoint_purposes_have_independent_budgets() {
        let limiter = RequestRateLimiter::new(RequestRateConfig {
            capacity: 1,
            refill_per_second: 1.0,
        });
        let now = Utc.with_ymd_and_hms(2026, 7, 17, 8, 0, 0).unwrap();
        assert!(limiter.acquire(&context(), now).is_ok());
        let mut media = context();
        media.purpose = RequestPurpose::Media;
        assert!(limiter.acquire(&media, now).is_ok());
    }
}
