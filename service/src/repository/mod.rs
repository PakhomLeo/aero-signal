//! 仓储层。

pub mod administrator;
pub mod article;
pub mod article_memory;
pub mod article_postgres;
mod article_postgres_codec;
pub mod audit;
pub mod collector;
pub mod collector_memory;
pub mod collector_postgres;
pub mod daily_plan;
pub mod daily_plan_memory;
pub mod daily_plan_postgres;
pub mod delivery;
pub mod delivery_memory;
pub mod delivery_postgres;
pub mod egress;
pub mod egress_memory;
pub mod egress_postgres;
pub mod health;
pub mod health_memory;
pub mod health_postgres;
pub mod memory;
pub mod postgres;
pub mod retention;
pub mod retention_memory;
pub mod retention_postgres;
pub mod settings;
pub mod source;
pub mod source_memory;
pub mod source_postgres;

pub use administrator::{AdminSessionRecord, AdministratorRepository};
pub use article::{
    ArticleRepository, CapturedArticleCommit, CapturedArticleCommitResult, CommitRevisionCommand,
    CommitRevisionResult, UpsertArticleLink, UpsertArticleLinkResult,
};
pub use article_memory::MemoryArticleStore;
pub use article_postgres::PgArticleRepository;
pub use audit::{AuditLogRecord, AuditRepository};
pub use collector::CollectorRepository;
pub use collector_memory::MemoryCollectorStore;
pub use collector_postgres::PgCollectorRepository;
pub use daily_plan::DailyPlanRepository;
pub use daily_plan_memory::MemoryDailyPlanStore;
pub use daily_plan_postgres::PgDailyPlanRepository;
pub use delivery::DeliveryRepository;
pub use delivery_memory::MemoryDeliveryStore;
pub use delivery_postgres::PgDeliveryRepository;
pub use egress::EgressRepository;
pub use egress_memory::MemoryEgressStore;
pub use egress_postgres::PgEgressRepository;
pub use health::HealthSnapshotRepository;
pub use health_memory::MemoryHealthStore;
pub use health_postgres::PgHealthSnapshotRepository;
pub use memory::MemoryAdministratorStore;
pub use postgres::PgAdministratorRepository;
pub use retention::RetentionRepository;
pub use retention_memory::{MemoryRetentionStore, default_policies};
pub use retention_postgres::PgRetentionRepository;
pub use settings::SettingsRepository;
pub use source::SourceRepository;
pub use source::{AppliedHeartbeatOutcome, ApplyHeartbeatOutcome, ApplyHeartbeatRules};
pub use source_memory::MemorySourceStore;
pub use source_postgres::PgSourceRepository;
