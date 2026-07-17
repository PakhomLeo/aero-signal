//! ConnectorBackend trait：capability-first 微信连接器后端。

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use crate::error::ConnectorResult;
use crate::types::{
    ArticleCandidate, CapabilitySet, CollectorAccountView, DiscoverRequest, LoginContext,
    LoginCredentialRef, ProbeResult, QrPollResult, QrPollStatus, QrSession,
};

/// 连接器后端。
///
/// # 硬约束
/// - 必须如实声明 `capabilities`；未验收能力不得假装可用。
/// - `discover_recent` 只返回候选；**不得**把采集账号登录 token 挂到文章详情 URL。
/// - 详情全文抓取不在本 trait 职责内（即便声明了 `FetchFullArticle`，也由独立详情服务执行）。
#[async_trait]
pub trait ConnectorBackend: Send + Sync {
    /// 稳定名称：`mp_admin` / `weread` / `fake` 等。
    fn name(&self) -> &str;

    /// 适配器实现版本（capability 版本化，供 registry 暴露）。
    fn version(&self) -> &str;

    /// 声明的能力集合（静态声明；实际可用性以 `probe` 为准）。
    fn capabilities(&self) -> CapabilitySet;

    /// 最小连通/授权探测；禁止发现文章或下载详情。
    async fn probe(
        &self,
        account: &CollectorAccountView,
        credentials_json: Option<&str>,
        now: DateTime<Utc>,
    ) -> ConnectorResult<ProbeResult>;

    /// 发现近期文章候选；`limit` 为上限。
    ///
    /// 返回的 `canonical_url` 不得携带登录凭据查询参数。
    async fn discover_recent(&self, req: DiscoverRequest)
    -> ConnectorResult<Vec<ArticleCandidate>>;

    /// 创建二维码登录会话（可选；不支持时返回 `connector.unsupported`）。
    async fn create_qr_session(&self, ctx: LoginContext) -> ConnectorResult<QrSession> {
        let _ = ctx;
        Err(crate::error::ConnectorError::unsupported(format!(
            "连接器 {} 不支持扫码登录",
            self.name()
        )))
    }

    /// 轮询二维码状态。
    async fn poll_qr_session(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<QrPollResult> {
        let _ = (ticket, provider_state_json);
        Err(crate::error::ConnectorError::unsupported(format!(
            "连接器 {} 不支持扫码登录",
            self.name()
        )))
    }

    /// 在同一固定出口上完成登录，产出凭据引用。
    async fn finalize_login(
        &self,
        ticket: &str,
        provider_state_json: Option<&str>,
    ) -> ConnectorResult<LoginCredentialRef> {
        let _ = (ticket, provider_state_json);
        Err(crate::error::ConnectorError::unsupported(format!(
            "连接器 {} 不支持扫码登录",
            self.name()
        )))
    }

    /// 推进二维码会话状态（仅 fake/dev 连接器实现；真实连接器默认拒绝）。
    fn advance_qr(&self, ticket: &str, status: QrPollStatus) -> ConnectorResult<()> {
        let _ = (ticket, status);
        Err(crate::error::ConnectorError::new(
            "collector.advance_not_allowed",
            format!("连接器 {} 不支持推进二维码状态", self.name()),
        ))
    }
}
