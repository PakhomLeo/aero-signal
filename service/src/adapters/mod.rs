//! 外部适配器。

pub mod factory;
pub mod proxy_provider;
pub mod qr_ticket_store;
pub mod redis_qr_ticket_store;
pub mod secret_store;

/// 微信连接器（M06）：re-export `platform-connectors`，避免业务层直接依赖路径漂移。
pub mod connectors {
    pub use platform_connectors::*;
}

pub use factory::{
    RuntimeAdapters, build_memory_adapters, build_production_adapters, build_runtime_adapters,
};
pub use proxy_provider::{
    AllocatedEndpoint, DefaultProxyProviderAdapter, EgressTestResult, ProxyProviderAdapter,
    ProxyProviderError,
};
pub use qr_ticket_store::{
    MemoryQrTicketStore, QR_TICKET_KEY_PREFIX, QrTicketRecord, QrTicketStore, qr_ticket_key,
};
pub use redis_qr_ticket_store::RedisQrTicketStore;
pub use secret_store::{FileSecretStore, MemorySecretStore, SecretStore};
