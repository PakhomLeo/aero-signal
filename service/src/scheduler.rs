//! Worker 日调度：07:30 preflight、08:00 build_daily_plan（幂等入队）。

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::app::AppHandles;

/// 在独立 task 中运行调度循环（仅 signal-worker 调用）。
pub fn spawn_daily_scheduler(handles: Arc<AppHandles>) {
    if !handles.config.scheduler_enabled {
        tracing::info!("SIGNAL_SCHEDULER=0，日调度未启动");
        return;
    }
    tokio::spawn(async move {
        info!("日调度已启动（60s tick；07:30 preflight / 08:00 plan）");
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(e) = scheduler_tick(&handles).await {
                warn!(error = %e, "scheduler tick 失败");
            }
        }
    });
}

async fn scheduler_tick(handles: &AppHandles) -> Result<(), String> {
    if let Some(job) = handles.preflight.tick_schedule_enqueue().await? {
        info!(job_id = %job.id, kind = %job.kind, "调度：preflight 已入队");
    }
    if let Some(job) = handles.daily_plan.tick_schedule_enqueue().await? {
        info!(job_id = %job.id, kind = %job.kind, "调度：daily_plan 已入队");
    } else {
        debug!("调度：当前无计划窗口任务");
    }
    Ok(())
}
