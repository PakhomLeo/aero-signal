//! 连接器领域类型：能力、探测、发现、二维码会话。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ConnectorError;

/// 连接器名称（稳定标识）。
pub const NAME_MP_ADMIN: &str = "mp_admin";
pub const NAME_WEREAD: &str = "weread";
pub const NAME_FAKE: &str = "fake";

/// 连接器能力（capability-first）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorCapability {
    /// 扫码登录。
    QrLogin,
    /// 发现近期文章候选（不含详情正文抓取）。
    DiscoverRecent,
    /// 声明可取全文；**详情抓取不得把登录 token 挂到文章 URL**，由独立详情服务读取 canonical URL。
    FetchFullArticle,
}

impl ConnectorCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::QrLogin => "qr_login",
            Self::DiscoverRecent => "discover_recent",
            Self::FetchFullArticle => "fetch_full_article",
        }
    }
}

/// 能力集合。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilitySet {
    pub items: Vec<ConnectorCapability>,
}

impl CapabilitySet {
    pub fn new(items: impl IntoIterator<Item = ConnectorCapability>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    pub fn contains(&self, cap: ConnectorCapability) -> bool {
        self.items.contains(&cap)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// 探测状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Healthy,
    Degraded,
    NeedsRelogin,
    Unsupported,
    Failed,
}

/// 探测结果（原因已脱敏，可供 health API 输出）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub status: ProbeStatus,
    /// 脱敏 reason_code，如 `collector.needs_relogin`。
    pub reason_code: Option<String>,
    /// 脱敏说明，禁止 Cookie/token/代理地址。
    pub message: Option<String>,
    /// 探测时确认仍启用的能力。
    pub enabled_capabilities: CapabilitySet,
    pub probed_at: DateTime<Utc>,
}

impl ProbeResult {
    pub fn healthy(caps: CapabilitySet, now: DateTime<Utc>) -> Self {
        Self {
            status: ProbeStatus::Healthy,
            reason_code: None,
            message: None,
            enabled_capabilities: caps,
            probed_at: now,
        }
    }

    pub fn unsupported(message: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            status: ProbeStatus::Unsupported,
            reason_code: Some(crate::error::codes::UNSUPPORTED.to_string()),
            message: Some(message.into()),
            enabled_capabilities: CapabilitySet::default(),
            probed_at: now,
        }
    }

    pub fn needs_relogin(message: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            status: ProbeStatus::NeedsRelogin,
            reason_code: Some(crate::error::codes::NEEDS_RELOGIN.to_string()),
            message: Some(message.into()),
            enabled_capabilities: CapabilitySet::default(),
            probed_at: now,
        }
    }

    pub fn degraded(
        reason_code: impl Into<String>,
        message: impl Into<String>,
        caps: CapabilitySet,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            status: ProbeStatus::Degraded,
            reason_code: Some(reason_code.into()),
            message: Some(message.into()),
            enabled_capabilities: caps,
            probed_at: now,
        }
    }

    pub fn failed(message: impl Into<String>, now: DateTime<Utc>) -> Self {
        Self {
            status: ProbeStatus::Failed,
            reason_code: Some(crate::error::codes::FAILED.to_string()),
            message: Some(message.into()),
            enabled_capabilities: CapabilitySet::default(),
            probed_at: now,
        }
    }
}

/// 采集登录账号摘要（连接器侧视图；不含凭据明文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectorAccountView {
    pub id: Uuid,
    pub connector: String,
    pub display_name: Option<String>,
    pub active: bool,
    pub needs_relogin: bool,
    /// 绑定固定出口是否健康。
    pub fixed_egress_healthy: bool,
    /// 固定出口标识；connector 只能使用该受控上下文，不得自行轮换出口。
    pub fixed_egress_id: Option<Uuid>,
    /// 是否处于冷却。
    pub cooling: bool,
    /// 当日预算是否已耗尽。
    pub budget_exhausted: bool,
    /// 账号侧已启用的能力（来自上次 probe / 配置）。
    pub enabled_capabilities: CapabilitySet,
    /// 授权管理范围内的公众号标识（mp_admin 用）；空表示无授权范围。
    pub authorized_mp_ids: Vec<String>,
}

/// 监测公众号标识（发现请求用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    pub id: Option<Uuid>,
    /// 强标识：biz / fakeid / alias / weread_mp_id 等。
    pub identity: String,
    /// 可选等级提示（LO0/LO1 不得因等级假装 weread 可用）。
    pub level_hint: Option<String>,
}

/// 发现近期文章请求。
///
/// 连接器只返回候选链接/标识；**不得**把登录 token 附加到详情 URL。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverRequest {
    pub source: SourceRef,
    pub account: CollectorAccountView,
    /// 最多返回篇数（调用方通常 ≤3）。
    pub limit: u8,
    /// 非敏感列表偏移；正常增量为 0，历史回填由持久 Job 逐页推进。
    #[serde(default)]
    pub cursor: u32,
    /// 单次连接器调用允许读取的页数。服务层历史回填固定为 1，便于形成 checkpoint。
    #[serde(default = "default_page_budget")]
    pub page_budget: u8,
    /// mp_admin 等连接器使用的凭据 JSON（token/cookies/fakeid）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_json: Option<String>,
}

fn default_page_budget() -> u8 {
    3
}

impl std::fmt::Debug for DiscoverRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DiscoverRequest")
            .field("source", &self.source)
            .field("account", &self.account)
            .field("limit", &self.limit)
            .field("cursor", &self.cursor)
            .field("page_budget", &self.page_budget)
            .field(
                "credentials_json",
                &self.credentials_json.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// 文章候选（发现结果）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArticleCandidate {
    pub title: String,
    /// Canonical 文章 URL，不含登录凭据。
    pub canonical_url: String,
    pub published_at: Option<DateTime<Utc>>,
    pub identity: Option<String>,
    /// 列表响应可提供的作者；详情抓取可以用更可信的值覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// 列表响应提供的封面 URL；它不代表已经下载或信任该资源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// 脱敏后的列表候选证据。不得包含 cookie、token 或原始响应。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ArticleCandidateEvidence>,
}

/// 用于排查候选来源的可持久化、脱敏证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArticleCandidateEvidence {
    pub connector: String,
    /// 连接器请求中的非敏感分页偏移。
    pub cursor: u32,
    /// 由脱敏后候选摘要计算的 SHA-256，而非原始响应哈希。
    pub response_summary_hash: String,
    pub observed_at: DateTime<Utc>,
}

/// 登录上下文（创建二维码会话）。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginContext {
    /// Runtime-only values; never serialize connector credentials or session state.
    #[serde(skip)]
    pub ticket: String,
    pub fixed_egress_id: Option<Uuid>,
    #[serde(skip)]
    pub administrator_session_hash: String,
    /// 由服务端解析的固定出口代理；敏感，不得进入日志或 API。
    #[serde(skip)]
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for LoginContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LoginContext")
            .field("ticket", &self.ticket)
            .field("fixed_egress_id", &self.fixed_egress_id)
            .field("administrator_session_hash", &"[REDACTED]")
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// 二维码会话创建结果。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrSession {
    pub ticket: String,
    /// data URL，供前端展示；禁止把原始第三方二维码 URL 写入审计。
    pub qr_data_url: String,
    pub expires_at: DateTime<Utc>,
    pub poll_after_ms: u64,
    /// 连接器私有临时状态。调用方必须将其作为敏感数据持久化，不得返回 API 或写日志。
    #[serde(skip)]
    pub provider_state_json: Option<String>,
}

impl std::fmt::Debug for QrSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QrSession")
            .field("ticket", &self.ticket)
            .field("qr_data_url", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .field("poll_after_ms", &self.poll_after_ms)
            .field(
                "provider_state_json",
                &self.provider_state_json.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// 二维码轮询状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QrPollStatus {
    Waiting,
    Scanned,
    Confirmed,
    Expired,
    Cancelled,
    Failed,
}

/// 二维码轮询结果。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrPollResult {
    pub status: QrPollStatus,
    pub message: Option<String>,
    /// 下一次轮询/完成登录所需的更新状态；敏感，不得返回 API 或写日志。
    #[serde(skip)]
    pub provider_state_json: Option<String>,
}

impl std::fmt::Debug for QrPollResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QrPollResult")
            .field("status", &self.status)
            .field("message", &self.message)
            .field(
                "provider_state_json",
                &self.provider_state_json.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

/// 登录完成产出的凭据引用（仅 secret ref，无明文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginCredentialRef {
    pub secret_ref: String,
    pub display_name: Option<String>,
    pub account_identity: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    /// 写入 SecretStore 的凭据 JSON；由 QrLoginService 落库（不进 API）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_payload: Option<String>,
    /// 登录后授权管理范围内的公众号标识（fakeid 等）。
    #[serde(default)]
    pub authorized_mp_ids: Vec<String>,
}

/// 选择器输入：候选账号列表 + 所需能力。
#[derive(Debug, Clone)]
pub struct SelectRequest {
    pub required_capability: ConnectorCapability,
    pub preferred_connector: Option<String>,
    pub candidates: Vec<CollectorAccountView>,
}

/// 选择成功。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedAccount {
    pub account: CollectorAccountView,
}

/// 选择失败原因（结构化，供计划/验证写 needs_action）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionFailure {
    pub reason_code: String,
    pub message: String,
    /// 各候选账号被拒绝的原因（脱敏）。
    pub rejected: Vec<RejectedCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedCandidate {
    pub account_id: Uuid,
    pub connector: String,
    pub reason_code: String,
    pub message: String,
}

impl SelectionFailure {
    pub fn to_error(&self) -> ConnectorError {
        ConnectorError::new(&self.reason_code, &self.message)
    }
}

/// 选择结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionResult {
    Selected(SelectedAccount),
    Unavailable(SelectionFailure),
}
