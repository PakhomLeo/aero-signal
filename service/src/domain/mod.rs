//! 领域模型：唯一管理员、会话、调度设置、出口、采集登录、文章、监测公众号、健康快照、日计划、投递。

pub mod administrator;
pub mod article;
pub mod collector;
pub mod daily_plan;
pub mod delivery;
pub mod egress;
pub mod health;
pub mod password;
pub mod retention;
pub mod session;
pub mod settings;
pub mod source;

pub use administrator::{Administrator, BootstrapCredentials};
pub use article::{
    Article, ArticleAsset, ArticleChangeEvent, ArticleImageDecision, ArticleLink, ArticleOcrBlock,
    ArticleRevision, ArticleStatus, CHANGE_SCHEMA_VERSION, ChangeType, DeliveryOutbox, EditorKind,
    OcrBlockStatus, OutboxStatus,
};
pub use collector::{
    CollectorAccount, CollectorAccountPublic, CollectorAccountStatus, CollectorEgressBinding,
    CollectorLoginSession, QrLoginStatus,
};
pub use daily_plan::{DailyFetchPlan, DailyPlanStatus};
pub use delivery::{
    ApiCallLog, ApiScope, ApiToken, ApiTokenPublic, DeliveryGroup, DeliveryGroupPublic,
    DeliveryGroupSource, DeliveryGroupStatus, IssuedApiToken, PlainApiToken, generate_api_token,
    hash_api_token, token_prefix,
};
pub use egress::{
    EgressKind, EgressProfile, EgressProfilePublic, EgressPurpose, EgressStatus,
    LOCAL_DIRECT_PROFILE_ID, ProxyProvider, ProxyProviderMode, ProxyProviderPublic,
};
pub use health::{
    HealthCheckRun, HealthComponentStatus, HealthComponentType, OverallHealthStatus,
    ServiceHealthComponent, ServiceHealthSnapshot, aggregate_overall, sanitize_safe_details,
};
pub use password::{PasswordHasher, PasswordPolicy, PasswordPolicyError};
pub use retention::{
    RetentionCandidate, RetentionDeleteDecision, RetentionPolicy, RetentionTargetKind,
};
pub use session::{AdminSession, SessionToken, generate_session_token, hash_session_token};
pub use settings::{ScheduleSettings, WorkingWindow};
pub use source::{
    AccountFetchRules, HeartbeatOutcome, SourceAccount, SourceAccountId, SourceAccountPublic,
    SourceHeartbeatEvent, SourceLevel, SourceStatus, SourceValidationRun, ValidationRunStatus,
};
