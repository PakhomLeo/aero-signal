//! weread 连接器：发现/采集默认 off；扫码展示可在骨架阶段启用。
//!
//! 不得因监测等级为 LO0/LO1 而假装本连接器可用于发现文章。

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::backend::ConnectorBackend;
use crate::error::{ConnectorError, ConnectorResult};
use crate::types::{
    ArticleCandidate, CapabilitySet, CollectorAccountView, ConnectorCapability, DiscoverRequest,
    LoginContext, LoginCredentialRef, NAME_WEREAD, ProbeResult, QrPollResult, QrSession,
};

/// 微信读书连接器。
///
/// - 发现/采集：默认关闭，需 `with_integration_accepted(true)`。
/// - 扫码展示：骨架阶段始终提供可解码 PNG data URL，供管理端验证 UI；
///   轮询保持 `waiting`，不推进真实扫码登录。
pub struct WereadConnector {
    version: String,
    /// 部署方是否已完成合法、可复现、已验收的集成。默认 `false`。
    integration_accepted: bool,
}

impl Default for WereadConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl WereadConnector {
    /// 默认 off（发现/采集）。
    pub fn new() -> Self {
        Self {
            version: "0.1.0-off".into(),
            integration_accepted: false,
        }
    }

    /// 仅在部署验收通过后由运维显式打开发现/采集。
    pub fn with_integration_accepted(mut self, accepted: bool) -> Self {
        self.integration_accepted = accepted;
        if accepted {
            self.version = "0.1.0-accepted".into();
        }
        self
    }

    pub fn is_enabled(&self) -> bool {
        self.integration_accepted
    }
}

#[async_trait]
impl ConnectorBackend for WereadConnector {
    fn name(&self) -> &str {
        NAME_WEREAD
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn capabilities(&self) -> CapabilitySet {
        // 骨架：允许管理端创建扫码会话以验证二维码展示。
        // 发现/全文能力仍仅在验收后声明，避免 selector / 计划误用。
        if self.integration_accepted {
            CapabilitySet::new([
                ConnectorCapability::QrLogin,
                ConnectorCapability::DiscoverRecent,
            ])
        } else {
            CapabilitySet::new([ConnectorCapability::QrLogin])
        }
    }

    async fn probe(
        &self,
        account: &CollectorAccountView,
        credentials_json: Option<&str>,
        now: DateTime<Utc>,
    ) -> ConnectorResult<ProbeResult> {
        let _ = (account, credentials_json);
        // LO0/LO1 等级提示不得改变本结论。
        if !self.integration_accepted {
            return Ok(ProbeResult::unsupported(
                "weread 未完成部署验收，保持关闭",
                now,
            ));
        }
        Ok(ProbeResult::unsupported(
            "weread 已标记验收但实现尚未接入",
            now,
        ))
    }

    async fn discover_recent(
        &self,
        req: DiscoverRequest,
    ) -> ConnectorResult<Vec<ArticleCandidate>> {
        let _ = req;
        Err(ConnectorError::unsupported(
            "weread 未启用或未验收，拒绝发现",
        ))
    }

    async fn create_qr_session(&self, ctx: LoginContext) -> ConnectorResult<QrSession> {
        let _ = ctx;
        Err(ConnectorError::unsupported(
            "weread 扫码协议未实现，连接器保持关闭",
        ))
    }

    async fn poll_qr_session(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<QrPollResult> {
        let _ = (ticket, provider_state_json);
        Err(ConnectorError::unsupported(
            "weread 扫码协议未实现，连接器保持关闭",
        ))
    }

    async fn finalize_login(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<LoginCredentialRef> {
        let _ = (ticket, provider_state_json);
        Err(ConnectorError::unsupported(
            "weread 真实扫码登录协议尚未接入，无法完成确认",
        ))
    }
}
