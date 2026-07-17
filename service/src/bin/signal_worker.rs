//! Aero Signal 后台 Worker：抢占并执行 fetch_jobs。
//!
//! 与 `signal-service` 共用 `build_app`；生产必须独立进程，勿依赖 HTTP 内嵌 Worker。

use std::sync::Arc;

use signal_service::{ServiceConfig, app::build_app};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() {
    match dotenvy::dotenv() {
        Ok(path) => eprintln!("已加载环境文件: {}", path.display()),
        Err(dotenvy::Error::Io(_)) => {}
        Err(err) => eprintln!("读取 .env 失败（忽略）: {err}"),
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config = ServiceConfig::from_env().unwrap_or_else(|err| {
        eprintln!("配置加载失败: {err}");
        std::process::exit(1);
    });

    tracing::info!(store = config.store_mode.as_str(), "装配 signal-worker");

    let handles = build_app(&config).await.unwrap_or_else(|err| {
        eprintln!("装配失败: {err}");
        std::process::exit(1);
    });

    let handles = Arc::new(handles);
    signal_service::scheduler::spawn_daily_scheduler(handles.clone());

    let worker_id = std::env::var("SIGNAL_WORKER_ID")
        .unwrap_or_else(|_| format!("signal-worker-{}", std::process::id()));
    let worker = handles.build_worker(&worker_id);
    tracing::info!(%worker_id, "signal-worker 开始轮询");
    println!(
        "signal-worker running id={worker_id} store={}",
        config.store_mode.as_str()
    );

    worker.run_forever().await;
}
