//! 可注入时钟：生产用系统时间，测试可冻结。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 时钟抽象。
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;
}

/// 系统时钟。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// 冻结时钟：`now()` 始终返回构造时或 `set` 后的时间。
#[derive(Debug)]
pub struct FrozenClock {
    /// 自 UNIX epoch 起的毫秒数。
    millis: AtomicU64,
}

impl FrozenClock {
    pub fn new(now: SystemTime) -> Self {
        Self {
            millis: AtomicU64::new(system_time_to_millis(now)),
        }
    }

    pub fn at_unix_millis(millis: u64) -> Self {
        Self {
            millis: AtomicU64::new(millis),
        }
    }

    pub fn set(&self, now: SystemTime) {
        self.millis
            .store(system_time_to_millis(now), Ordering::SeqCst);
    }

    pub fn advance(&self, duration: Duration) {
        let add = duration.as_millis() as u64;
        self.millis.fetch_add(add, Ordering::SeqCst);
    }
}

impl Clock for FrozenClock {
    fn now(&self) -> SystemTime {
        let millis = self.millis.load(Ordering::SeqCst);
        UNIX_EPOCH + Duration::from_millis(millis)
    }
}

fn system_time_to_millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_clock_returns_same_instant_twice() {
        let clock = FrozenClock::at_unix_millis(1_700_000_000_000);
        let a = clock.now();
        let b = clock.now();
        assert_eq!(a, b);
    }

    #[test]
    fn frozen_clock_advance_changes_now() {
        let clock = FrozenClock::at_unix_millis(1000);
        clock.advance(Duration::from_secs(2));
        assert_eq!(clock.now(), UNIX_EPOCH + Duration::from_millis(3000));
    }
}
