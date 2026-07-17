//! Composition root 适配器工厂：按配置选择 QR / 密钥 / 对象存储实现。

use std::sync::Arc;

use crawler_capture::{FakeObjectStore, MinioObjectStore, ObjectStore};
use server_core::{AppError, ServiceConfig, SignalEnvironment, StoreMode};

use super::qr_ticket_store::{MemoryQrTicketStore, QrTicketStore};
use super::redis_qr_ticket_store::RedisQrTicketStore;
use super::secret_store::{FileSecretStore, MemorySecretStore, SecretStore};

/// 运行时外部适配器（HTTP 与 Worker 共用）。
pub struct RuntimeAdapters {
    pub qr_tickets: Arc<dyn QrTicketStore>,
    pub secrets: Arc<dyn SecretStore>,
    pub object_store: Arc<dyn ObjectStore>,
}

/// 内存联调：全部进程内实现。
pub fn build_memory_adapters() -> RuntimeAdapters {
    RuntimeAdapters {
        qr_tickets: Arc::new(MemoryQrTicketStore::new()),
        secrets: Arc::new(MemorySecretStore::new()),
        object_store: Arc::new(FakeObjectStore::new()),
    }
}

/// Postgres / 生产路径：优先 Redis + 文件密钥 + MinIO（可回退并打日志）。
pub async fn build_production_adapters(
    config: &ServiceConfig,
) -> Result<RuntimeAdapters, AppError> {
    let secrets = build_secret_store(config)?;
    let object_store = build_object_store(config)?;
    let qr_tickets = build_qr_ticket_store(config).await?;
    Ok(RuntimeAdapters {
        qr_tickets,
        secrets,
        object_store,
    })
}

/// 按 store_mode 选择适配器。
pub async fn build_runtime_adapters(config: &ServiceConfig) -> Result<RuntimeAdapters, AppError> {
    config.validate_runtime()?;
    match config.store_mode {
        StoreMode::Memory => {
            if config.redis_url.is_some() || config.s3_endpoint.is_some() {
                // memory 模式也允许显式配置 Redis/S3（双进程联调）
                build_production_adapters(config).await
            } else {
                Ok(build_memory_adapters())
            }
        }
        StoreMode::Postgres => build_production_adapters(config).await,
    }
}

fn build_secret_store(config: &ServiceConfig) -> Result<Arc<dyn SecretStore>, AppError> {
    if config.store_mode == StoreMode::Memory && config.secret_store_path.is_none() {
        return Ok(Arc::new(MemorySecretStore::new()));
    }
    let path = config.secret_store_path.clone().ok_or_else(|| {
        AppError::new(
            "config.secret_store_path_required",
            "PostgreSQL/production 运行必须显式设置 SIGNAL_SECRET_STORE_PATH",
        )
    })?;
    let store = FileSecretStore::new(&path).map_err(|e| {
        AppError::new(
            "config.secret_store_init_failed",
            format!("初始化密钥目录 {path} 失败: {e}"),
        )
    })?;
    tracing::info!(path = %path, "使用 FileSecretStore");
    Ok(Arc::new(store))
}

fn build_object_store(config: &ServiceConfig) -> Result<Arc<dyn ObjectStore>, AppError> {
    if let Some(endpoint) = config.s3_endpoint.as_deref() {
        let access = config.s3_access_key.as_deref().ok_or_else(|| {
            AppError::new("config.s3_access_key_required", "必须设置 S3_ACCESS_KEY")
        })?;
        let secret = config.s3_secret_key.as_deref().ok_or_else(|| {
            AppError::new("config.s3_secret_key_required", "必须设置 S3_SECRET_KEY")
        })?;
        let store = MinioObjectStore::from_env_config(endpoint, &config.s3_bucket, access, secret)
            .map_err(|e| AppError::new("config.s3_init_failed", e.message))?;
        tracing::info!(
            endpoint = %endpoint,
            bucket = %config.s3_bucket,
            "使用 MinioObjectStore（真 S3 PUT）"
        );
        return Ok(Arc::new(store));
    }
    if config.store_mode == StoreMode::Postgres
        || config.environment == SignalEnvironment::Production
    {
        return Err(AppError::new(
            "config.s3_endpoint_required",
            "PostgreSQL/production 运行必须配置 S3_ENDPOINT，禁止回退 FakeObjectStore",
        ));
    }
    Ok(Arc::new(FakeObjectStore::new()))
}

async fn build_qr_ticket_store(config: &ServiceConfig) -> Result<Arc<dyn QrTicketStore>, AppError> {
    if let Some(url) = config.redis_url.as_deref() {
        let store = RedisQrTicketStore::connect(url, &config.redis_key_prefix)
            .await
            .map_err(|e| AppError::new("config.redis_connect_failed", e))?;
        tracing::info!("使用 RedisQrTicketStore");
        return Ok(Arc::new(store));
    }
    if config.store_mode == StoreMode::Postgres
        || config.environment == SignalEnvironment::Production
    {
        return Err(AppError::new(
            "config.redis_url_required",
            "PostgreSQL/production 运行必须配置 REDIS_URL，禁止回退 MemoryQrTicketStore",
        ));
    }
    Ok(Arc::new(MemoryQrTicketStore::new()))
}
