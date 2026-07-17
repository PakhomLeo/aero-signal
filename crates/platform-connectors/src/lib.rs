//! Aero Signal `platform-connectors`：capability-first 微信连接器。
//!
//! - `mp_admin`：仅授权管理范围；禁止任意第三方公众号枚举。
//! - `weread`：默认 off，未验收返回 `connector.unsupported`。
//! - `fake`：可控状态机，供 M05/M07 与单元测试。
//!
//! 详情抓取不在本 crate：连接器不得把登录 token 挂到文章 URL。

mod backend;
mod error;
mod fake;
mod mp_admin;
mod probe_recorder;
mod qr_placeholder;
mod registry;
mod selector;
mod types;
mod weread;

#[cfg(test)]
mod tests;

pub use backend::ConnectorBackend;
pub use error::{ConnectorError, ConnectorResult, codes};
pub use fake::{FakeConnector, FakeProbeBehavior, assert_probe_sanitized};
pub use mp_admin::MpAdminConnector;
pub use probe_recorder::{MemoryProbeRecorder, ProbeRecord, ProbeRecorder};
pub use registry::{ConnectorDescriptor, ConnectorRegistry};
pub use selector::ConnectorSelector;
pub use types::{
    ArticleCandidate, ArticleCandidateEvidence, CapabilitySet, CollectorAccountView,
    ConnectorCapability, DiscoverRequest, LoginContext, LoginCredentialRef, NAME_FAKE,
    NAME_MP_ADMIN, NAME_WEREAD, ProbeResult, ProbeStatus, QrPollResult, QrPollStatus, QrSession,
    RejectedCandidate, SelectRequest, SelectedAccount, SelectionFailure, SelectionResult,
    SourceRef,
};
pub use weread::WereadConnector;

use std::sync::Arc;

/// 构建默认注册表：fake + mp_admin 骨架 + weread(off)。
pub fn default_registry() -> ConnectorRegistry {
    registry_for_environment(true)
}

/// 按运行环境构建连接器注册表。production 必须传 `false`，避免暴露 Fake connector。
pub fn registry_for_environment(include_fake: bool) -> ConnectorRegistry {
    let mut registry = ConnectorRegistry::new();
    if include_fake {
        registry.register(Arc::new(FakeConnector::new()));
    }
    registry.register(Arc::new(MpAdminConnector::new()));
    registry.register(Arc::new(WereadConnector::new()));
    registry
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    #[test]
    fn production_registry_does_not_contain_fake() {
        let registry = registry_for_environment(false);
        assert!(registry.get(NAME_FAKE).is_none());
        assert!(registry.get(NAME_MP_ADMIN).is_some());
    }
}
