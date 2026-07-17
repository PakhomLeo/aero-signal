//! 从环境变量加载服务配置。

use std::env;
use std::fmt;
use std::net::SocketAddr;

use crate::error::AppError;

/// 运行环境。只有 test/development 允许显式使用 Fake 能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalEnvironment {
    Test,
    Development,
    Production,
}

impl SignalEnvironment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Development => "development",
            Self::Production => "production",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AppError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "test" => Ok(Self::Test),
            "development" | "dev" => Ok(Self::Development),
            "production" | "prod" => Ok(Self::Production),
            other => Err(AppError::new(
                "config.invalid_signal_env",
                format!("SIGNAL_ENV 无效: {other}（期望 test|development|production）"),
            )),
        }
    }

    pub fn allows_fake(self) -> bool {
        !matches!(self, Self::Production)
    }
}

/// 仓储后端：内存联调或 PostgreSQL 部署。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreMode {
    Memory,
    Postgres,
}

impl StoreMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Postgres => "postgres",
        }
    }

    pub fn parse(s: &str) -> Result<Self, AppError> {
        match s.trim().to_ascii_lowercase().as_str() {
            "memory" | "mem" => Ok(Self::Memory),
            "postgres" | "pg" | "postgresql" => Ok(Self::Postgres),
            other => Err(AppError::new(
                "config.invalid_store_mode",
                format!("SIGNAL_STORE 无效: {other}（期望 memory|postgres）"),
            )),
        }
    }
}

/// 进程级服务配置（不含业务密钥明文日志）。
#[derive(Clone)]
pub struct ServiceConfig {
    /// 运行环境；production 启用严格依赖与 Fake 禁止门禁。
    pub environment: SignalEnvironment,
    /// 监听地址，默认 `127.0.0.1:8080`。
    pub listen_addr: SocketAddr,
    /// 产品名，用于审计与日志。
    pub product: String,
    /// 是否处于调试模式（可影响错误细节暴露策略）。
    pub debug: bool,
    /// 数据库连接串；`store_mode=postgres` 时必填。
    pub database_url: Option<String>,
    /// Redis URL（QR ticket 等短期状态；postgres 部署推荐）。
    pub redis_url: Option<String>,
    /// Redis key 前缀。
    pub redis_key_prefix: String,
    /// 仓储模式：`SIGNAL_STORE` 优先；未设时有 `DATABASE_URL` → postgres，否则 memory。
    pub store_mode: StoreMode,
    /// 仅 memory 模式：HTTP 进程内嵌 Worker（`SIGNAL_EMBED_WORKER=1`）。生产禁止依赖。
    pub embed_worker: bool,
    /// Worker 进程内日调度（07:30 preflight / 08:00 plan）；`SIGNAL_SCHEDULER=0` 关闭。
    pub scheduler_enabled: bool,
    /// 对象存储 endpoint（MinIO / S3 兼容）。
    pub s3_endpoint: Option<String>,
    pub s3_access_key: Option<String>,
    pub s3_secret_key: Option<String>,
    pub s3_bucket: String,
    /// 密钥文件目录（postgres 默认 `./data/secrets`）。
    pub secret_store_path: Option<String>,
    /// CloakBrowser CDP（后续浏览器抓取接入）。
    pub cloakbrowser_cdp_url: Option<String>,
    /// OCR provider 名称。R05 接入真实 provider 前，production 会拒绝启动。
    pub ocr_provider: Option<String>,
    /// mp_admin 真实协议开关。
    pub mp_admin_protocol: bool,
}

impl fmt::Debug for ServiceConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceConfig")
            .field("environment", &self.environment)
            .field("listen_addr", &self.listen_addr)
            .field("product", &self.product)
            .field("debug", &self.debug)
            .field(
                "database_url",
                &self.database_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("redis_url", &self.redis_url.as_ref().map(|_| "[REDACTED]"))
            .field("redis_key_prefix", &self.redis_key_prefix)
            .field("store_mode", &self.store_mode)
            .field("embed_worker", &self.embed_worker)
            .field("scheduler_enabled", &self.scheduler_enabled)
            .field(
                "s3_endpoint",
                &self.s3_endpoint.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "s3_access_key",
                &self.s3_access_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "s3_secret_key",
                &self.s3_secret_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field("s3_bucket", &self.s3_bucket)
            .field("secret_store_path", &self.secret_store_path)
            .field(
                "cloakbrowser_cdp_url",
                &self.cloakbrowser_cdp_url.as_ref().map(|_| "[REDACTED]"),
            )
            .field("ocr_provider", &self.ocr_provider)
            .field("mp_admin_protocol", &self.mp_admin_protocol)
            .finish()
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn env_bool(key: &str, default: bool) -> bool {
    match env::var(key) {
        Ok(v) => matches!(v.as_str(), "1" | "true" | "TRUE" | "yes"),
        Err(_) => default,
    }
}

impl ServiceConfig {
    /// 从环境变量加载；缺失项使用安全默认值。
    pub fn from_env() -> Result<Self, AppError> {
        let environment = SignalEnvironment::parse(
            &env::var("SIGNAL_ENV").unwrap_or_else(|_| "development".into()),
        )?;
        let listen = env::var("SIGNAL_LISTEN_ADDR")
            .or_else(|_| env::var("LISTEN_ADDR"))
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string());
        let listen_addr: SocketAddr = listen.parse().map_err(|_| {
            AppError::new(
                "config.invalid_listen_addr",
                format!("无法解析监听地址: {listen}"),
            )
        })?;

        let product = env::var("SIGNAL_PRODUCT").unwrap_or_else(|_| "signal".to_string());
        let debug = env_bool("SIGNAL_DEBUG", false);

        let database_url = env_nonempty("DATABASE_URL");
        let store_mode = match env::var("SIGNAL_STORE") {
            Ok(v) if !v.trim().is_empty() => StoreMode::parse(&v)?,
            _ => {
                if database_url.is_some() {
                    StoreMode::Postgres
                } else {
                    StoreMode::Memory
                }
            }
        };
        if store_mode == StoreMode::Postgres && database_url.is_none() {
            return Err(AppError::new(
                "config.database_url_required",
                "store_mode=postgres 时必须设置 DATABASE_URL",
            ));
        }

        let embed_worker = env_bool("SIGNAL_EMBED_WORKER", false);
        let scheduler_enabled = env_bool("SIGNAL_SCHEDULER", true);

        let config = Self {
            environment,
            listen_addr,
            product,
            debug,
            database_url,
            redis_url: env_nonempty("REDIS_URL"),
            redis_key_prefix: env::var("REDIS_KEY_PREFIX")
                .unwrap_or_else(|_| "signal:".to_string()),
            store_mode,
            embed_worker,
            scheduler_enabled,
            s3_endpoint: env_nonempty("S3_ENDPOINT"),
            s3_access_key: env_nonempty("S3_ACCESS_KEY"),
            s3_secret_key: env_nonempty("S3_SECRET_KEY"),
            s3_bucket: env::var("S3_BUCKET").unwrap_or_else(|_| "aero-signal-dev".into()),
            secret_store_path: env_nonempty("SIGNAL_SECRET_STORE_PATH"),
            cloakbrowser_cdp_url: env_nonempty("CLOAKBROWSER_CDP_URL"),
            ocr_provider: env_nonempty("SIGNAL_OCR_PROVIDER"),
            mp_admin_protocol: env_bool("SIGNAL_MP_ADMIN_PROTOCOL", false),
        };
        config.validate_runtime()?;
        Ok(config)
    }

    /// 在任何外部连接之前验证生产部署的硬依赖。
    pub fn validate_runtime(&self) -> Result<(), AppError> {
        if self.environment != SignalEnvironment::Production {
            return Ok(());
        }
        if self.store_mode != StoreMode::Postgres {
            return Err(AppError::new(
                "config.production_postgres_required",
                "production 必须使用 SIGNAL_STORE=postgres",
            ));
        }
        require(
            &self.database_url,
            "config.database_url_required",
            "DATABASE_URL",
        )?;
        require(&self.redis_url, "config.redis_url_required", "REDIS_URL")?;
        require(
            &self.secret_store_path,
            "config.secret_store_path_required",
            "SIGNAL_SECRET_STORE_PATH",
        )?;
        require(
            &self.s3_endpoint,
            "config.s3_endpoint_required",
            "S3_ENDPOINT",
        )?;
        require(
            &self.s3_access_key,
            "config.s3_access_key_required",
            "S3_ACCESS_KEY",
        )?;
        require(
            &self.s3_secret_key,
            "config.s3_secret_key_required",
            "S3_SECRET_KEY",
        )?;
        if self.s3_bucket.trim().is_empty() {
            return Err(AppError::new(
                "config.s3_bucket_required",
                "production 必须设置 S3_BUCKET",
            ));
        }
        require(
            &self.cloakbrowser_cdp_url,
            "config.cloakbrowser_required",
            "CLOAKBROWSER_CDP_URL",
        )?;
        if !self.mp_admin_protocol {
            return Err(AppError::new(
                "config.connector_required",
                "production 必须启用至少一个已验收真实 connector（当前要求 SIGNAL_MP_ADMIN_PROTOCOL=1）",
            ));
        }
        let provider = require(
            &self.ocr_provider,
            "config.ocr_provider_required",
            "SIGNAL_OCR_PROVIDER",
        )?;
        // R05 前没有生产 OCR adapter。明确拒绝，绝不回退 Fake。
        Err(AppError::new(
            "config.ocr_provider_not_implemented",
            format!("OCR provider `{provider}` 尚未接入生产 adapter"),
        ))
    }

    /// 单元测试用默认配置（端口 0，由系统分配）。
    pub fn for_test() -> Self {
        Self {
            environment: SignalEnvironment::Test,
            listen_addr: "127.0.0.1:0".parse().expect("valid addr"),
            product: "signal".to_string(),
            debug: true,
            database_url: None,
            redis_url: None,
            redis_key_prefix: "signal:".to_string(),
            store_mode: StoreMode::Memory,
            embed_worker: false,
            scheduler_enabled: false,
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_bucket: "aero-signal-dev".into(),
            secret_store_path: None,
            cloakbrowser_cdp_url: None,
            ocr_provider: Some("fake".into()),
            mp_admin_protocol: false,
        }
    }

    /// 本地脚手架默认监听 `127.0.0.1:8080`。
    pub fn for_test_listen() -> Self {
        Self {
            environment: SignalEnvironment::Development,
            listen_addr: "127.0.0.1:8080".parse().expect("valid addr"),
            product: "signal".to_string(),
            debug: true,
            database_url: None,
            redis_url: None,
            redis_key_prefix: "signal:".to_string(),
            store_mode: StoreMode::Memory,
            embed_worker: true,
            scheduler_enabled: false,
            s3_endpoint: None,
            s3_access_key: None,
            s3_secret_key: None,
            s3_bucket: "aero-signal-dev".into(),
            secret_store_path: None,
            cloakbrowser_cdp_url: None,
            ocr_provider: Some("fake".into()),
            mp_admin_protocol: false,
        }
    }
}

fn require<'a>(
    value: &'a Option<String>,
    code: &'static str,
    variable: &'static str,
) -> Result<&'a str, AppError> {
    value
        .as_deref()
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| AppError::new(code, format!("production 必须设置 {variable}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn for_test_config_is_valid() {
        let cfg = ServiceConfig::for_test();
        assert_eq!(cfg.product, "signal");
        assert!(cfg.debug);
        assert!(!cfg.scheduler_enabled);
        assert_eq!(cfg.environment, SignalEnvironment::Test);
        assert!(cfg.environment.allows_fake());
    }

    fn production_config() -> ServiceConfig {
        let mut cfg = ServiceConfig::for_test();
        cfg.environment = SignalEnvironment::Production;
        cfg.store_mode = StoreMode::Postgres;
        cfg.database_url = Some("postgres://redacted/db".into());
        cfg.redis_url = Some("redis://redacted/0".into());
        cfg.secret_store_path = Some("./data/secrets".into());
        cfg.s3_endpoint = Some("https://s3.invalid".into());
        cfg.s3_access_key = Some("access".into());
        cfg.s3_secret_key = Some("secret".into());
        cfg.s3_bucket = "signal".into();
        cfg.cloakbrowser_cdp_url = Some("http://browser.invalid".into());
        cfg.mp_admin_protocol = true;
        cfg.ocr_provider = Some("external".into());
        cfg
    }

    #[test]
    fn production_rejects_memory_store() {
        let mut cfg = production_config();
        cfg.store_mode = StoreMode::Memory;
        assert_eq!(
            cfg.validate_runtime().unwrap_err().code,
            "config.production_postgres_required"
        );
    }

    #[test]
    fn production_requires_each_runtime_dependency() {
        type MissingDependencyCase = (&'static str, fn(&mut ServiceConfig));
        let cases: &[MissingDependencyCase] = &[
            ("config.database_url_required", |c| c.database_url = None),
            ("config.redis_url_required", |c| c.redis_url = None),
            ("config.secret_store_path_required", |c| {
                c.secret_store_path = None
            }),
            ("config.s3_endpoint_required", |c| c.s3_endpoint = None),
            ("config.s3_access_key_required", |c| c.s3_access_key = None),
            ("config.s3_secret_key_required", |c| c.s3_secret_key = None),
            ("config.cloakbrowser_required", |c| {
                c.cloakbrowser_cdp_url = None
            }),
            ("config.connector_required", |c| c.mp_admin_protocol = false),
            ("config.ocr_provider_required", |c| c.ocr_provider = None),
        ];
        for (expected, mutate) in cases {
            let mut cfg = production_config();
            mutate(&mut cfg);
            assert_eq!(cfg.validate_runtime().unwrap_err().code, *expected);
        }
    }

    #[test]
    fn production_never_accepts_unimplemented_ocr_provider() {
        let cfg = production_config();
        assert_eq!(
            cfg.validate_runtime().unwrap_err().code,
            "config.ocr_provider_not_implemented"
        );
    }

    #[test]
    fn debug_redacts_connection_secrets() {
        let mut cfg = ServiceConfig::for_test();
        cfg.database_url = Some("postgres://user:password@db.invalid/signal".into());
        cfg.redis_url = Some("redis://:secret@redis.invalid/0".into());
        cfg.s3_secret_key = Some("super-secret".into());
        let debug = format!("{cfg:?}");
        assert!(!debug.contains("password@"));
        assert!(!debug.contains("super-secret"));
        assert!(debug.contains("[REDACTED]"));
    }
}
