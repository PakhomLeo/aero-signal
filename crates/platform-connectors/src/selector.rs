//! 连接器选择器：按账号状态 / 出口 / capability / 冷却 / 预算筛选。

use crate::error::codes;
use crate::types::{
    CollectorAccountView, ConnectorCapability, RejectedCandidate, SelectRequest, SelectedAccount,
    SelectionFailure, SelectionResult,
};

/// 连接器账号选择器。
///
/// 任一条件不满足都产生结构化 reason，**不得**静默切换到不同平台路径。
pub struct ConnectorSelector;

impl ConnectorSelector {
    /// 从候选中选出第一个满足条件的账号。
    pub fn select(req: &SelectRequest) -> SelectionResult {
        let mut rejected = Vec::new();

        for account in &req.candidates {
            if let Some(pref) = &req.preferred_connector
                && &account.connector != pref
            {
                rejected.push(RejectedCandidate {
                    account_id: account.id,
                    connector: account.connector.clone(),
                    reason_code: codes::UNSUPPORTED.to_string(),
                    message: format!("偏好连接器为 {pref}，跳过 {}", account.connector),
                });
                continue;
            }

            if let Some(reason) = Self::reject_reason(account, req.required_capability) {
                rejected.push(RejectedCandidate {
                    account_id: account.id,
                    connector: account.connector.clone(),
                    reason_code: reason.0.to_string(),
                    message: reason.1,
                });
                continue;
            }

            return SelectionResult::Selected(SelectedAccount {
                account: account.clone(),
            });
        }

        let (reason_code, message) = Self::aggregate_failure(&rejected, req);
        SelectionResult::Unavailable(SelectionFailure {
            reason_code: reason_code.to_string(),
            message,
            rejected,
        })
    }

    fn reject_reason(
        account: &CollectorAccountView,
        required: ConnectorCapability,
    ) -> Option<(&'static str, String)> {
        if !account.active {
            return Some((codes::INACTIVE, "采集账号未启用".into()));
        }
        if account.needs_relogin {
            return Some((codes::NEEDS_RELOGIN, "采集账号需重新扫码登录".into()));
        }
        if !account.fixed_egress_healthy {
            return Some((codes::EGRESS_UNHEALTHY, "绑定固定出口不健康".into()));
        }
        if account.cooling {
            return Some((codes::COOLING, "采集账号处于冷却期".into()));
        }
        if account.budget_exhausted {
            return Some((codes::BUDGET_EXHAUSTED, "采集账号当日预算已耗尽".into()));
        }
        if !account.enabled_capabilities.contains(required) {
            // weread 未验收等场景：能力未启用 → unsupported
            return Some((
                codes::UNSUPPORTED,
                format!("账号未启用能力 {}", required.as_str()),
            ));
        }
        None
    }

    fn aggregate_failure(
        rejected: &[RejectedCandidate],
        req: &SelectRequest,
    ) -> (&'static str, String) {
        if rejected.is_empty() {
            return (codes::NO_ACCOUNT, "没有可用的采集登录账号".into());
        }

        // 优先暴露最有行动价值的原因
        let priority = [
            codes::UNSUPPORTED,
            codes::NEEDS_RELOGIN,
            codes::EGRESS_UNHEALTHY,
            codes::BUDGET_EXHAUSTED,
            codes::COOLING,
            codes::INACTIVE,
        ];
        for code in priority {
            if rejected.iter().any(|r| r.reason_code == code) {
                let msg = match code {
                    codes::UNSUPPORTED => format!(
                        "无可用连接器满足能力 {}（含未验收/未启用）",
                        req.required_capability.as_str()
                    ),
                    codes::NEEDS_RELOGIN => "可用账号均需重新扫码登录".into(),
                    codes::EGRESS_UNHEALTHY => "可用账号绑定的固定出口均不健康".into(),
                    codes::BUDGET_EXHAUSTED => "可用账号当日预算均已耗尽".into(),
                    codes::COOLING => "可用账号均处于冷却期".into(),
                    codes::INACTIVE => "候选采集账号均未启用".into(),
                    _ => "没有可用的采集登录账号".into(),
                };
                return (code, msg);
            }
        }

        (codes::NO_ACCOUNT, "没有可用的采集登录账号".into())
    }
}
