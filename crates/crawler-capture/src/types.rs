//! 抓取领域类型。

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct CaptureRequest {
    pub url: String,
    #[serde(default)]
    pub need_assets: bool,
    pub platform_hint: Option<String>,
    /// Internal fixed-egress proxy. Debug output deliberately redacts this value.
    #[serde(default, skip_serializing)]
    pub proxy_url: Option<String>,
}

impl std::fmt::Debug for CaptureRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CaptureRequest")
            .field("url", &self.url)
            .field("need_assets", &self.need_assets)
            .field("platform_hint", &self.platform_hint)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureResult {
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<String>,
    pub content_html: String,
    pub content_md: String,
    pub candidates: Vec<ImageCandidate>,
    pub raw_html: Option<String>,
    pub source: CaptureSource,
    pub used_browser: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    HttpDirect,
    BrowserRender,
}

/// 正文候选图片（分类前）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageCandidate {
    pub url: String,
    pub alt: Option<String>,
    pub in_body: bool,
    pub hidden: bool,
    pub lazy: bool,
    pub decorative_hint: bool,
    /// 若 HTML 带宽高属性则填入；否则分类时用下载后实测。
    pub declared_width: Option<u32>,
    pub declared_height: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageDecisionKind {
    Kept,
    Decorative,
    TooSmall,
    Hidden,
    Duplicate,
    Unsupported,
    DownloadFailed,
}

impl ImageDecisionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::Decorative => "decorative",
            Self::TooSmall => "too_small",
            Self::Hidden => "hidden",
            Self::Duplicate => "duplicate",
            Self::Unsupported => "unsupported",
            Self::DownloadFailed => "download_failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageDecision {
    pub candidate_url: String,
    pub decision: ImageDecisionKind,
    pub measured_width: Option<u32>,
    pub measured_height: Option<u32>,
    pub reason_json: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageThresholds {
    /// PRD 默认：min(width,height) >= 80
    pub min_side_px: u32,
    /// PRD 默认：width × height >= 14400
    pub min_area_px: u32,
}

impl Default for ImageThresholds {
    fn default() -> Self {
        Self {
            min_side_px: 80,
            min_area_px: 14_400,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    BodyImage,
    RawHtml,
    Screenshot,
}

impl AssetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BodyImage => "body_image",
            Self::RawHtml => "raw_html",
            Self::Screenshot => "screenshot",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredObject {
    pub storage_key: String,
    pub content_hash: String,
    pub byte_size: u64,
    pub mime_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub deduplicated: bool,
}
