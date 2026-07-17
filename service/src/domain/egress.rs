//! 出口与代理供应商领域模型。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// 出口用途。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressPurpose {
    Login,
    Discover,
    ArticleDetail,
    AssetDownload,
}

impl EgressPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Discover => "discover",
            Self::ArticleDetail => "article_detail",
            Self::AssetDownload => "asset_download",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "login" => Ok(Self::Login),
            "discover" => Ok(Self::Discover),
            "article_detail" => Ok(Self::ArticleDetail),
            "asset_download" => Ok(Self::AssetDownload),
            other => Err(format!("未知用途: {other}")),
        }
    }

    pub fn is_fixed_family(self) -> bool {
        matches!(self, Self::Login | Self::Discover)
    }

    pub fn is_dynamic_family(self) -> bool {
        matches!(self, Self::ArticleDetail | Self::AssetDownload)
    }
}

impl std::fmt::Display for EgressPurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 出口种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressKind {
    LocalDirect,
    Fixed,
    Dynamic,
}

impl EgressKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalDirect => "local_direct",
            Self::Fixed => "fixed",
            Self::Dynamic => "dynamic",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "local_direct" => Ok(Self::LocalDirect),
            "fixed" => Ok(Self::Fixed),
            "dynamic" => Ok(Self::Dynamic),
            other => Err(format!("未知出口种类: {other}")),
        }
    }

    /// 该种类允许的用途集合。
    pub fn allowed_purpose_family(self) -> &'static [EgressPurpose] {
        match self {
            Self::LocalDirect => &[
                EgressPurpose::Login,
                EgressPurpose::Discover,
                EgressPurpose::ArticleDetail,
                EgressPurpose::AssetDownload,
            ],
            Self::Fixed => &[EgressPurpose::Login, EgressPurpose::Discover],
            Self::Dynamic => &[EgressPurpose::ArticleDetail, EgressPurpose::AssetDownload],
        }
    }

    pub fn allows_purpose(self, purpose: EgressPurpose) -> bool {
        self.allowed_purpose_family().contains(&purpose)
    }
}

impl std::fmt::Display for EgressKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 供应商模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProviderMode {
    StaticEndpoint,
    Gateway,
    VendorApi,
}

impl ProxyProviderMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StaticEndpoint => "static_endpoint",
            Self::Gateway => "gateway",
            Self::VendorApi => "vendor_api",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "static_endpoint" => Ok(Self::StaticEndpoint),
            "gateway" => Ok(Self::Gateway),
            "vendor_api" => Ok(Self::VendorApi),
            other => Err(format!("未知供应商模式: {other}")),
        }
    }
}

/// 出口状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressStatus {
    Active,
    Disabled,
    Degraded,
}

impl EgressStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Disabled => "disabled",
            Self::Degraded => "degraded",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "active" => Ok(Self::Active),
            "disabled" => Ok(Self::Disabled),
            "degraded" => Ok(Self::Degraded),
            other => Err(format!("未知出口状态: {other}")),
        }
    }
}

/// 代理供应商（密钥仅存引用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyProvider {
    pub id: Uuid,
    pub name: String,
    pub mode: ProxyProviderMode,
    pub api_base_url: Option<String>,
    pub auth_secret_ref: Option<String>,
    pub request_template_json: Value,
    pub response_mapping_json: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 对外公开的供应商视图：不回显密钥内容，仅暴露是否已配置引用。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyProviderPublic {
    pub id: Uuid,
    pub name: String,
    pub mode: ProxyProviderMode,
    pub api_base_url: Option<String>,
    pub has_auth_secret: bool,
    pub request_template_json: Value,
    pub response_mapping_json: Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ProxyProvider> for ProxyProviderPublic {
    fn from(p: &ProxyProvider) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            mode: p.mode,
            api_base_url: p.api_base_url.clone(),
            has_auth_secret: p.auth_secret_ref.is_some(),
            request_template_json: p.request_template_json.clone(),
            response_mapping_json: p.response_mapping_json.clone(),
            enabled: p.enabled,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

/// 出口配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressProfile {
    pub id: Uuid,
    pub name: String,
    pub kind: EgressKind,
    pub provider_id: Option<Uuid>,
    pub endpoint_secret_ref: Option<String>,
    pub priority: i32,
    pub allowed_purposes: Vec<EgressPurpose>,
    pub status: EgressStatus,
    pub last_preflight_status: Option<String>,
    pub last_preflight_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 对外公开的出口视图：不回显密钥。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EgressProfilePublic {
    pub id: Uuid,
    pub name: String,
    pub kind: EgressKind,
    pub provider_id: Option<Uuid>,
    pub has_endpoint_secret: bool,
    pub priority: i32,
    pub allowed_purposes: Vec<EgressPurpose>,
    pub status: EgressStatus,
    pub last_preflight_status: Option<String>,
    pub last_preflight_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&EgressProfile> for EgressProfilePublic {
    fn from(p: &EgressProfile) -> Self {
        Self {
            id: p.id,
            name: p.name.clone(),
            kind: p.kind,
            provider_id: p.provider_id,
            has_endpoint_secret: p.endpoint_secret_ref.is_some(),
            priority: p.priority,
            allowed_purposes: p.allowed_purposes.clone(),
            status: p.status,
            last_preflight_status: p.last_preflight_status.clone(),
            last_preflight_at: p.last_preflight_at,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
}

/// 迁移种子：默认本机出口稳定 UUID。
pub const LOCAL_DIRECT_PROFILE_ID: Uuid = Uuid::from_bytes([
    0xa0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
]);

/// 校验用途与种类是否匹配（固定/动态不得混用）。
pub fn validate_purposes_for_kind(
    kind: EgressKind,
    purposes: &[EgressPurpose],
) -> Result<(), String> {
    if purposes.is_empty() {
        return Err("allowed_purposes 不能为空".into());
    }
    for p in purposes {
        if !kind.allows_purpose(*p) {
            return Err(format!(
                "出口种类 {kind} 不允许用途 {p}（固定仅 login/discover，动态仅 article_detail/asset_download）"
            ));
        }
    }
    Ok(())
}

/// 解析用途字符串列表。
pub fn parse_purposes(raw: &[String]) -> Result<Vec<EgressPurpose>, String> {
    let mut out = Vec::with_capacity(raw.len());
    for s in raw {
        out.push(EgressPurpose::parse(s)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_rejects_detail_purpose() {
        let err = validate_purposes_for_kind(EgressKind::Fixed, &[EgressPurpose::ArticleDetail])
            .unwrap_err();
        assert!(err.contains("不允许"));
    }

    #[test]
    fn dynamic_rejects_login_purpose() {
        let err =
            validate_purposes_for_kind(EgressKind::Dynamic, &[EgressPurpose::Login]).unwrap_err();
        assert!(err.contains("不允许"));
    }

    #[test]
    fn local_allows_all() {
        validate_purposes_for_kind(
            EgressKind::LocalDirect,
            &[
                EgressPurpose::Login,
                EgressPurpose::Discover,
                EgressPurpose::ArticleDetail,
                EgressPurpose::AssetDownload,
            ],
        )
        .unwrap();
    }
}
