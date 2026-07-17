//! FakeConnector：可控 QR 状态机、probe、discover_recent，供 M05/M07 与单元测试。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::backend::ConnectorBackend;
use crate::error::{ConnectorError, ConnectorResult};
use crate::qr_placeholder::placeholder_qr_data_url;
use crate::types::{
    ArticleCandidate, CapabilitySet, CollectorAccountView, ConnectorCapability, DiscoverRequest,
    LoginContext, LoginCredentialRef, NAME_FAKE, ProbeResult, ProbeStatus, QrPollResult,
    QrPollStatus, QrSession,
};

/// Fake 探测行为。
#[derive(Debug, Clone)]
pub enum FakeProbeBehavior {
    Healthy,
    Degraded {
        reason_code: String,
        message: String,
    },
    NeedsRelogin,
    Failed {
        message: String,
    },
}

/// 内存中的 QR 会话状态。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct FakeQrState {
    status: QrPollStatus,
    expires_at: DateTime<Utc>,
    display_name: String,
}

/// 可控假连接器。
pub struct FakeConnector {
    version: String,
    capabilities: CapabilitySet,
    probe_behavior: Arc<Mutex<FakeProbeBehavior>>,
    /// 发现时返回的候选模板；实际条数受 limit 截断。
    discover_pool: Arc<Mutex<Vec<ArticleCandidate>>>,
    qr_sessions: Arc<Mutex<HashMap<String, FakeQrState>>>,
    qr_ttl_secs: i64,
}

impl Default for FakeConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeConnector {
    pub fn new() -> Self {
        Self {
            version: "0.1.0-fake".into(),
            capabilities: CapabilitySet::new([
                ConnectorCapability::QrLogin,
                ConnectorCapability::DiscoverRecent,
                ConnectorCapability::FetchFullArticle,
            ]),
            probe_behavior: Arc::new(Mutex::new(FakeProbeBehavior::Healthy)),
            discover_pool: Arc::new(Mutex::new(default_candidates())),
            qr_sessions: Arc::new(Mutex::new(HashMap::new())),
            qr_ttl_secs: 120,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn set_probe_behavior(&self, behavior: FakeProbeBehavior) {
        *self.probe_behavior.lock().expect("fake probe lock") = behavior;
    }

    pub fn set_discover_pool(&self, candidates: Vec<ArticleCandidate>) {
        *self.discover_pool.lock().expect("fake discover lock") = candidates;
    }

    /// 测试 / dev 辅助：将已有 ticket 推进到指定状态。
    pub fn advance_qr(&self, ticket: &str, status: QrPollStatus) -> ConnectorResult<()> {
        self.advance_qr_inner(ticket, status)
    }

    fn advance_qr_inner(&self, ticket: &str, status: QrPollStatus) -> ConnectorResult<()> {
        let mut guard = self.qr_sessions.lock().expect("fake qr lock");
        let state = guard
            .get_mut(ticket)
            .ok_or_else(|| ConnectorError::failed(format!("未知 ticket: {ticket}")))?;
        state.status = status;
        Ok(())
    }
}

fn default_candidates() -> Vec<ArticleCandidate> {
    (1..=5)
        .map(|i| ArticleCandidate {
            title: format!("假文章 {i}"),
            canonical_url: format!("https://mp.weixin.qq.com/s/fake-article-{i}"),
            published_at: None,
            identity: Some(format!("fake-{i}")),
            author: None,
            cover_url: None,
            evidence: None,
        })
        .collect()
}

#[async_trait]
impl ConnectorBackend for FakeConnector {
    fn name(&self) -> &str {
        NAME_FAKE
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn capabilities(&self) -> CapabilitySet {
        self.capabilities.clone()
    }

    async fn probe(
        &self,
        account: &CollectorAccountView,
        credentials_json: Option<&str>,
        now: DateTime<Utc>,
    ) -> ConnectorResult<ProbeResult> {
        let _ = credentials_json;
        if account.needs_relogin {
            return Ok(ProbeResult::needs_relogin("账号标记需重新登录", now));
        }
        let behavior = self.probe_behavior.lock().expect("fake probe lock").clone();
        let caps = self.capabilities.clone();
        Ok(match behavior {
            FakeProbeBehavior::Healthy => ProbeResult::healthy(caps, now),
            FakeProbeBehavior::Degraded {
                reason_code,
                message,
            } => ProbeResult::degraded(reason_code, message, caps, now),
            FakeProbeBehavior::NeedsRelogin => {
                ProbeResult::needs_relogin("Fake 模拟需重新登录", now)
            }
            FakeProbeBehavior::Failed { message } => ProbeResult::failed(message, now),
        })
    }

    async fn discover_recent(
        &self,
        req: DiscoverRequest,
    ) -> ConnectorResult<Vec<ArticleCandidate>> {
        if !req.account.active {
            return Err(ConnectorError::new(
                crate::error::codes::INACTIVE,
                "账号未启用，无法发现",
            ));
        }
        let pool = self.discover_pool.lock().expect("fake discover lock");
        let limit = req.limit as usize;
        Ok(pool
            .iter()
            .skip(req.cursor as usize)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn create_qr_session(&self, ctx: LoginContext) -> ConnectorResult<QrSession> {
        let now = Utc::now();
        let expires_at = now + Duration::seconds(self.qr_ttl_secs);
        let mut guard = self.qr_sessions.lock().expect("fake qr lock");
        guard.insert(
            ctx.ticket.clone(),
            FakeQrState {
                status: QrPollStatus::Waiting,
                expires_at,
                display_name: "Fake Collector".into(),
            },
        );
        // Valid PNG data URL — invalid base64 like "fakeqr" breaks <img> in admin UI.
        Ok(QrSession {
            ticket: ctx.ticket,
            qr_data_url: placeholder_qr_data_url(),
            expires_at,
            poll_after_ms: 1500,
            provider_state_json: Some(
                serde_json::to_string(&FakeQrState {
                    status: QrPollStatus::Waiting,
                    expires_at,
                    display_name: "Fake Collector".into(),
                })
                .map_err(|_| ConnectorError::failed("序列化 Fake 二维码状态失败"))?,
            ),
        })
    }

    async fn poll_qr_session(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<QrPollResult> {
        let now = Utc::now();
        let mut state: FakeQrState = match provider_state_json {
            Some(json) => serde_json::from_str(json)
                .map_err(|_| ConnectorError::failed("Fake 二维码状态无效"))?,
            None => self
                .qr_sessions
                .lock()
                .expect("fake qr lock")
                .get(ticket)
                .cloned()
                .ok_or_else(|| ConnectorError::failed(format!("未知 ticket: {ticket}")))?,
        };
        if let Some(advanced) = self.qr_sessions.lock().expect("fake qr lock").get(ticket) {
            state.status = advanced.status;
        }
        if state.status == QrPollStatus::Waiting && now >= state.expires_at {
            state.status = QrPollStatus::Expired;
        }
        Ok(QrPollResult {
            status: state.status,
            message: None,
            provider_state_json: Some(
                serde_json::to_string(&state)
                    .map_err(|_| ConnectorError::failed("序列化 Fake 二维码状态失败"))?,
            ),
        })
    }

    async fn finalize_login(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<LoginCredentialRef> {
        let mut state: FakeQrState = match provider_state_json {
            Some(json) => serde_json::from_str(json)
                .map_err(|_| ConnectorError::failed("Fake 二维码状态无效"))?,
            None => self
                .qr_sessions
                .lock()
                .expect("fake qr lock")
                .get(ticket)
                .cloned()
                .ok_or_else(|| ConnectorError::failed(format!("未知 ticket: {ticket}")))?,
        };
        if let Some(advanced) = self.qr_sessions.lock().expect("fake qr lock").get(ticket) {
            state.status = advanced.status;
        }
        match state.status {
            QrPollStatus::Confirmed => Ok(LoginCredentialRef {
                secret_ref: format!("secret:fake-cred:{ticket}"),
                display_name: Some(state.display_name.clone()),
                account_identity: Some(format!("fake-account:{ticket}")),
                expires_at: Some(Utc::now() + Duration::days(30)),
                store_payload: None,
                authorized_mp_ids: vec![],
            }),
            QrPollStatus::Expired => {
                Err(ConnectorError::new("collector.qr_expired", "二维码已过期"))
            }
            QrPollStatus::Cancelled => Err(ConnectorError::failed("二维码会话已取消")),
            QrPollStatus::Failed => Err(ConnectorError::failed("二维码会话失败")),
            other => Err(ConnectorError::failed(format!(
                "会话尚未确认，当前状态: {other:?}"
            ))),
        }
    }

    fn advance_qr(&self, ticket: &str, status: QrPollStatus) -> ConnectorResult<()> {
        self.advance_qr_inner(ticket, status)
    }
}

/// 断言探测结果不含敏感片段（供测试与 health 脱敏校验）。
pub fn assert_probe_sanitized(result: &ProbeResult) -> Result<(), String> {
    let forbidden = ["cookie=", "token=", "proxy://", "password="];
    if let Some(msg) = &result.message {
        let lower = msg.to_lowercase();
        for f in forbidden {
            if lower.contains(f) {
                return Err(format!("探测消息含敏感片段: {f}"));
            }
        }
    }
    if result.status == ProbeStatus::Degraded && result.reason_code.is_none() {
        return Err("降级探测缺少 reason_code".into());
    }
    Ok(())
}
