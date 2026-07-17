//! 出口错误分类：决定是否允许轮换动态出口。

use serde::{Deserialize, Serialize};

/// 抓取/出口相关错误类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressErrorClass {
    /// 网络不可达：可换下一健康动态出口。
    NetworkUnreachable,
    /// 验证码：不轮换。
    Captcha,
    /// WAF：不轮换。
    Waf,
    /// 平台拒绝：不轮换。
    PlatformDenied,
    /// 认证过期：不轮换（需重新扫码）。
    AuthenticationExpired,
    /// 限流：进入 needs_action，不轮换规避。
    RateLimited,
}

impl EgressErrorClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NetworkUnreachable => "network_unreachable",
            Self::Captcha => "captcha",
            Self::Waf => "waf",
            Self::PlatformDenied => "platform_denied",
            Self::AuthenticationExpired => "authentication_expired",
            Self::RateLimited => "rate_limited",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "network_unreachable" | "egress.network_unreachable" => Some(Self::NetworkUnreachable),
            "captcha" => Some(Self::Captcha),
            "waf" => Some(Self::Waf),
            "platform_denied" => Some(Self::PlatformDenied),
            "authentication_expired" => Some(Self::AuthenticationExpired),
            "rate_limited" => Some(Self::RateLimited),
            _ => None,
        }
    }

    /// 是否允许尝试下一个健康动态出口。
    pub fn allows_dynamic_rotation(self) -> bool {
        matches!(self, Self::NetworkUnreachable)
    }

    /// 对应的应用错误码风格。
    pub fn error_code(self) -> &'static str {
        match self {
            Self::NetworkUnreachable => "egress.network_unreachable",
            Self::Captcha => "crawler.blocked_by_protection",
            Self::Waf => "crawler.blocked_by_protection",
            Self::PlatformDenied => "crawler.blocked_by_protection",
            Self::AuthenticationExpired => "collector.needs_relogin",
            Self::RateLimited => "crawler.blocked_by_protection",
        }
    }
}

/// 轮换决策。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationDecision {
    /// 可换下一健康动态出口。
    TryNextDynamic,
    /// 进入 needs_action，不得轮换。
    NeedsAction,
    /// 需重新扫码。
    NeedsRelogin,
}

/// 出口策略：根据错误类别给出决策。
pub struct EgressPolicy;

impl EgressPolicy {
    pub fn decide(class: EgressErrorClass) -> RotationDecision {
        match class {
            EgressErrorClass::NetworkUnreachable => RotationDecision::TryNextDynamic,
            EgressErrorClass::AuthenticationExpired => RotationDecision::NeedsRelogin,
            EgressErrorClass::Captcha
            | EgressErrorClass::Waf
            | EgressErrorClass::PlatformDenied
            | EgressErrorClass::RateLimited => RotationDecision::NeedsAction,
        }
    }

    pub fn allows_rotation(class: EgressErrorClass) -> bool {
        matches!(Self::decide(class), RotationDecision::TryNextDynamic)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_unreachable_allows_rotation() {
        assert!(EgressPolicy::allows_rotation(
            EgressErrorClass::NetworkUnreachable
        ));
        assert_eq!(
            EgressPolicy::decide(EgressErrorClass::NetworkUnreachable),
            RotationDecision::TryNextDynamic
        );
    }

    #[test]
    fn platform_limits_do_not_rotate() {
        for c in [
            EgressErrorClass::Captcha,
            EgressErrorClass::Waf,
            EgressErrorClass::PlatformDenied,
            EgressErrorClass::AuthenticationExpired,
        ] {
            assert!(!EgressPolicy::allows_rotation(c), "{c:?} must not rotate");
        }
        assert_eq!(
            EgressPolicy::decide(EgressErrorClass::AuthenticationExpired),
            RotationDecision::NeedsRelogin
        );
        assert_eq!(
            EgressPolicy::decide(EgressErrorClass::Captcha),
            RotationDecision::NeedsAction
        );
    }
}
