//! 应用状态容器（可在各产品中扩展具体字段）。

use std::sync::Arc;

use crate::clock::Clock;
use crate::config::ServiceConfig;

/// 最小应用状态：配置 + 可注入时钟。
#[derive(Clone)]
pub struct AppState<C: Clock = crate::clock::SystemClock> {
    pub config: Arc<ServiceConfig>,
    pub clock: Arc<C>,
}

impl<C: Clock> AppState<C> {
    pub fn new(config: ServiceConfig, clock: C) -> Self {
        Self {
            config: Arc::new(config),
            clock: Arc::new(clock),
        }
    }
}
