//! mp_admin 适配器：仅授权管理范围内能力；禁止任意第三方公众号枚举。

mod protocol;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};

use crate::backend::ConnectorBackend;
use crate::error::{ConnectorError, ConnectorResult};
use crate::types::{
    ArticleCandidate, CapabilitySet, CollectorAccountView, ConnectorCapability, DiscoverRequest,
    LoginContext, LoginCredentialRef, NAME_MP_ADMIN, ProbeResult, QrPollResult, QrPollStatus,
    QrSession,
};

use protocol::{
    MpAdminQrSession, QrAskStatus, discover_recent_articles, env_protocol_enabled, finalize_login,
    parse_credentials, poll_qr, probe_credentials, start_qr_session,
};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MpAdminQrState {
    status: QrPollStatus,
    expires_at: DateTime<Utc>,
    session: MpAdminQrSession,
}

/// 公众号后台管理连接器。
pub struct MpAdminConnector {
    version: String,
    protocol_ready: bool,
    qr_ttl_secs: i64,
}

impl Default for MpAdminConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl MpAdminConnector {
    pub fn new() -> Self {
        Self {
            version: if env_protocol_enabled() {
                "0.2.0-mp-admin".into()
            } else {
                "0.1.0-skeleton".into()
            },
            protocol_ready: env_protocol_enabled(),
            qr_ttl_secs: 120,
        }
    }

    pub fn with_protocol_ready(mut self, ready: bool) -> Self {
        self.protocol_ready = ready;
        if ready {
            self.version = "0.2.0-mp-admin".into();
        }
        self
    }

    fn ensure_authorized(req: &DiscoverRequest) -> ConnectorResult<()> {
        let identity = req.source.identity.trim();
        if identity.is_empty() {
            return Err(ConnectorError::unauthorized_enumeration(
                "缺少公众号标识，拒绝枚举",
            ));
        }
        if req.account.authorized_mp_ids.is_empty() {
            return Err(ConnectorError::unauthorized_enumeration(
                "采集账号无授权管理范围，禁止枚举公众号",
            ));
        }
        if !req
            .account
            .authorized_mp_ids
            .iter()
            .any(|id| id == identity)
        {
            return Err(ConnectorError::unauthorized_enumeration(format!(
                "公众号 {identity} 不在授权管理范围内"
            )));
        }
        Ok(())
    }

    fn resolve_source_fakeid(
        req: &DiscoverRequest,
        _creds: &protocol::MpAdminCredentials,
    ) -> String {
        req.source.identity.trim().to_string()
    }
}

#[async_trait]
impl ConnectorBackend for MpAdminConnector {
    fn name(&self) -> &str {
        NAME_MP_ADMIN
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::new([
            ConnectorCapability::QrLogin,
            ConnectorCapability::DiscoverRecent,
            ConnectorCapability::FetchFullArticle,
        ])
    }

    async fn probe(
        &self,
        account: &CollectorAccountView,
        credentials_json: Option<&str>,
        now: DateTime<Utc>,
    ) -> ConnectorResult<ProbeResult> {
        if account.needs_relogin {
            return Ok(ProbeResult::needs_relogin("mp_admin 会话需重新扫码", now));
        }
        if !account.active {
            return Ok(ProbeResult::failed("mp_admin 账号未启用", now));
        }
        if !self.protocol_ready {
            return Ok(ProbeResult::unsupported("mp_admin 真实协议未启用", now));
        }
        let credentials_json = match credentials_json {
            Some(value) => value,
            None => return Ok(ProbeResult::needs_relogin("mp_admin 缺少凭据", now)),
        };
        let credentials = match parse_credentials(credentials_json) {
            Ok(value) => value,
            Err(_) => return Ok(ProbeResult::needs_relogin("mp_admin 凭据无效", now)),
        };
        match probe_credentials(&credentials).await {
            Ok(()) => Ok(ProbeResult::healthy(self.capabilities(), now)),
            Err(error) if error.reason_code == crate::error::codes::NEEDS_RELOGIN => {
                Ok(ProbeResult::needs_relogin("mp_admin 登录态已失效", now))
            }
            Err(_) => Ok(ProbeResult::failed("mp_admin 登录态探测失败", now)),
        }
    }

    async fn discover_recent(
        &self,
        req: DiscoverRequest,
    ) -> ConnectorResult<Vec<ArticleCandidate>> {
        Self::ensure_authorized(&req)?;

        if !self.protocol_ready {
            return Err(ConnectorError::unsupported("mp_admin 真实发现协议未启用"));
        }

        let creds_json = req
            .credentials_json
            .as_deref()
            .ok_or_else(|| ConnectorError::needs_relogin("缺少 mp_admin 凭据"))?;
        let creds = parse_credentials(creds_json)?;
        let fakeid = Self::resolve_source_fakeid(&req, &creds);
        discover_recent_articles(&creds, &fakeid, req.limit, req.cursor, req.page_budget).await
    }

    async fn create_qr_session(&self, ctx: LoginContext) -> ConnectorResult<QrSession> {
        if !self.protocol_ready {
            return Err(ConnectorError::unsupported("mp_admin 真实扫码协议未启用"));
        }
        let now = Utc::now();
        let expires_at = now + Duration::seconds(self.qr_ttl_secs);
        let (qr_data_url, session) = start_qr_session(ctx.proxy_url).await?;
        let state = MpAdminQrState {
            status: QrPollStatus::Waiting,
            expires_at,
            session,
        };
        Ok(QrSession {
            ticket: ctx.ticket,
            qr_data_url,
            expires_at,
            poll_after_ms: 2000,
            provider_state_json: Some(
                serde_json::to_string(&state)
                    .map_err(|_| ConnectorError::failed("序列化二维码状态失败"))?,
            ),
        })
    }

    async fn poll_qr_session(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<QrPollResult> {
        let _ = ticket;
        if !self.protocol_ready {
            return Err(ConnectorError::unsupported("mp_admin 真实扫码协议未启用"));
        }
        let mut state: MpAdminQrState = serde_json::from_str(
            provider_state_json.ok_or_else(|| ConnectorError::failed("缺少二维码会话状态"))?,
        )
        .map_err(|_| ConnectorError::failed("二维码会话状态无效"))?;
        if Utc::now() >= state.expires_at {
            state.status = QrPollStatus::Expired;
            return Ok(QrPollResult {
                status: state.status,
                message: None,
                provider_state_json: Some(
                    serde_json::to_string(&state)
                        .map_err(|_| ConnectorError::failed("序列化二维码状态失败"))?,
                ),
            });
        }
        let (ask, message) = poll_qr(&mut state.session).await?;
        let next_status = match ask {
            QrAskStatus::Waiting => QrPollStatus::Waiting,
            QrAskStatus::Scanned => QrPollStatus::Scanned,
            QrAskStatus::Confirmed => QrPollStatus::Confirmed,
            QrAskStatus::Expired => QrPollStatus::Expired,
            QrAskStatus::Failed => QrPollStatus::Failed,
        };

        state.status = next_status;
        Ok(QrPollResult {
            status: next_status,
            message: Some(message),
            provider_state_json: Some(
                serde_json::to_string(&state)
                    .map_err(|_| ConnectorError::failed("序列化二维码状态失败"))?,
            ),
        })
    }

    async fn finalize_login(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<LoginCredentialRef> {
        if !self.protocol_ready {
            let _ = ticket;
            return Err(ConnectorError::unsupported(
                "mp_admin 真实扫码登录协议尚未接入，无法完成确认",
            ));
        }

        let state: MpAdminQrState = serde_json::from_str(
            provider_state_json.ok_or_else(|| ConnectorError::failed("缺少二维码会话状态"))?,
        )
        .map_err(|_| ConnectorError::failed("二维码会话状态无效"))?;
        if state.status != QrPollStatus::Confirmed {
            return Err(ConnectorError::failed("二维码会话尚未确认"));
        }
        let creds = finalize_login(&state.session).await?;
        let authorized_mp_ids = if creds.authorized_mp_ids.is_empty() {
            creds.fakeid.clone().map(|f| vec![f]).unwrap_or_default()
        } else {
            creds.authorized_mp_ids.clone()
        };
        let display_name = creds
            .nickname
            .clone()
            .or_else(|| creds.fakeid.clone())
            .unwrap_or_else(|| "公众号管理员".into());
        let account_identity = creds
            .fakeid
            .clone()
            .or_else(|| creds.cookies.get("wxuin").cloned());

        let store_payload = serde_json::to_string(&creds)
            .map_err(|e| ConnectorError::failed(format!("序列化凭据失败: {e}")))?;

        Ok(LoginCredentialRef {
            secret_ref: format!("mp-admin:{ticket}"),
            display_name: Some(display_name),
            account_identity,
            expires_at: Some(Utc::now() + Duration::days(4)),
            store_payload: Some(store_payload),
            authorized_mp_ids,
        })
    }
}
