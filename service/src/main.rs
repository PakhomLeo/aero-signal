//! Aero Signal HTTP 服务入口。
//!
//! 仓储模式由 `SIGNAL_STORE` / `DATABASE_URL` 决定。
//! Job 执行请另起 `signal-worker`；仅 memory 且 `SIGNAL_EMBED_WORKER=1` 时可内嵌。

use signal_service::{
    ServiceConfig,
    app::build_app,
    runtime::{build_http_router, maybe_embed_worker},
};
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
        tracing::warn!("配置加载失败，使用 memory 联调默认值: {err}");
        ServiceConfig::for_test_listen()
    });

    tracing::info!(
        store = config.store_mode.as_str(),
        embed_worker = config.embed_worker,
        "装配 signal-service"
    );

    let handles = build_app(&config).await.unwrap_or_else(|err| {
        eprintln!("装配失败: {err}");
        std::process::exit(1);
    });
    if let Err(err) = handles.admin.try_bootstrap_from_env().await {
        eprintln!("管理员自动 bootstrap 失败: {} ({})", err.message, err.code);
        std::process::exit(1);
    }
    maybe_embed_worker(&handles);
    let app = build_http_router(&handles);

    let addr = config.listen_addr;
    tracing::info!(%addr, product = %config.product, "signal-service 启动中");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("绑定监听地址失败");
    let local = listener.local_addr().expect("读取本地地址失败");
    tracing::info!(%local, "signal-service 已监听；GET /healthz 可用");
    println!("signal-service listening on {local}");
    println!("store_mode={}", config.store_mode.as_str());
    println!(
        "bootstrap: POST http://{local}/api/admin/auth/bootstrap (requires SIGNAL_BOOTSTRAP_TOKEN)"
    );

    axum::serve(listener, app).await.expect("HTTP 服务异常退出");
}
