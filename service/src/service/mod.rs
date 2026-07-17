//! 应用服务层。

pub mod administrator;
pub mod api_token;
pub mod article_capture;
mod article_capture_contract;
pub mod article_cursor;
pub mod capture_failure;
pub mod connector_context;
pub mod content_normalizer;
pub mod daily_plan;
mod daily_plan_mapping;
pub mod daily_preflight;
mod daily_preflight_mapping;
pub mod delivery_group;
pub mod dynamic_lease;
pub mod egress;
pub mod egress_policy;
pub mod egress_resolver;
pub mod health_delivery;
pub mod heartbeat;
pub mod manual_edit;
pub mod markdown_builder;
pub mod ocr_planner;
pub mod plan_eligibility;
pub mod qr_login;
mod qr_login_contract;
pub mod request_limiter;
pub mod retention;
pub mod revision;
pub mod slot_allocator;
pub mod source_parser;
pub mod source_validation;
mod source_validation_contract;

pub use administrator::{
    AdministratorService, AuthSession, BootstrapResult, ChangePasswordCommand, LoginCommand,
    LoginResult, PatchMeCommand, SessionService,
};
pub use api_token::{ApiTokenService, AuthenticatedApiToken, IssueTokenCommand};
pub use article_capture::{
    ArticleCaptureService, CaptureOutcome, FetchArticleDetailPayload, FetchSourcePayload,
    HistoryBackfillPayload, RetryArticleAssetsPayload,
};
pub use article_cursor::{
    ArticleCursorService, DeliveryArticleDetail, DeliveryArticleSummary, SinceEventDto,
    SinceResponse,
};
pub use capture_failure::{AccountEffect, CaptureFailure, CaptureFailureCategory, SourceEffect};
pub use connector_context::{CollectorConnectorResolver, SelectedConnectorContext};
pub use content_normalizer::{ContentNormalizerService, NormalizeOutcome};
pub use daily_plan::{BuildDailyPlanPayload, DailyPlanListDto, DailyPlanRowDto, DailyPlanService};
pub use daily_preflight::{
    DailyPreflightPayload, DailyPreflightService, DefaultHealthComponentChecker,
    FakeHealthComponentChecker, HealthComponentChecker, HealthSnapshotAdminDto,
};
pub use delivery_group::{CreateGroupCommand, DeliveryGroupService, PatchGroupCommand};
pub use dynamic_lease::{DynamicLease, DynamicLeaseManager};
pub use egress::EgressService;
pub use egress_policy::{EgressErrorClass, EgressPolicy, RotationDecision};
pub use egress_resolver::{EgressResolver, ResolveRequest, ResolvedEgress};
pub use health_delivery::{
    HealthComponentDeliveryDto, HealthDeliveryService, HealthSnapshotDeliveryDto,
};
pub use heartbeat::{ApplyOutcomeCommand, ApplyOutcomeResult, HeartbeatPolicy, HeartbeatService};
pub use manual_edit::{
    ArticleDetailDto, ManualEditService, PatchArticleContentCommand, RefetchResponse, ReocrCommand,
    ReocrResponse,
};
pub use markdown_builder::{ContentBlock, MarkdownBuilder};
pub use ocr_planner::{OCR_VERSION, OcrArticleImagePayload, OcrJobPlanner};
pub use plan_eligibility::{
    EligibilityResult, EligibleSource, evaluate_eligibility, should_skip_at_runtime,
};
pub use qr_login::{
    AdvanceQrSessionCommand, CreateQrSessionCommand, PatchCollectorAccountCommand, QrLoginService,
    QrSessionResponse, ReauthorizeCommand,
};
pub use request_limiter::{
    RequestLimitContext, RequestLimitError, RequestPurpose, RequestRateConfig, RequestRateLimiter,
};
pub use retention::{RetentionCleanupService, RetentionRunSummary};
pub use revision::{PublishRevisionInput, PublishRevisionOutcome, RevisionService};
pub use slot_allocator::{
    SlotAllocator, SlotAllocatorConfig, SlotAssignment, SlotRequest, assert_not_equal_intervals,
};
pub use source_parser::{ParsedSource, SourceParser};
pub use source_validation::{
    ConfirmSourceCommand, FetchNowCommand, FetchNowResponse, PatchSourceCommand,
    SourceValidationService, ValidateSourceCommand, ValidateSourceResponse,
};
