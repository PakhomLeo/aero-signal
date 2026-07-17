//! 文章领域模型（M09 初表 + M10 revision/OCR/outbox）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleLink {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub canonical_url: String,
    pub wechat_biz: Option<String>,
    pub wechat_mid: Option<String>,
    pub wechat_idx: Option<String>,
    pub wechat_sn: Option<String>,
    pub title_hint: Option<String>,
    pub discovered_via: String,
    pub discovered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArticleStatus {
    Captured,
    Ready,
    ReadyWithOcrWarning,
    Failed,
    Withdrawn,
}

impl ArticleStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Captured => "captured",
            Self::Ready => "ready",
            Self::ReadyWithOcrWarning => "ready_with_ocr_warning",
            Self::Failed => "failed",
            Self::Withdrawn => "withdrawn",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "captured" => Some(Self::Captured),
            "ready" => Some(Self::Ready),
            "ready_with_ocr_warning" => Some(Self::ReadyWithOcrWarning),
            "failed" => Some(Self::Failed),
            "withdrawn" => Some(Self::Withdrawn),
            _ => None,
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Captured, Self::Ready)
                | (Self::Captured, Self::ReadyWithOcrWarning)
                | (Self::Captured, Self::Failed)
                | (Self::Ready, Self::ReadyWithOcrWarning)
                | (Self::Ready, Self::Withdrawn)
                | (Self::ReadyWithOcrWarning, Self::Ready)
                | (Self::ReadyWithOcrWarning, Self::Withdrawn)
                | (Self::Failed, Self::Captured)
        )
    }

    pub fn transition(self, next: Self) -> Result<Self, String> {
        if self == next {
            return Ok(self);
        }
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(format!(
                "非法文章状态跳转: {} → {}",
                self.as_str(),
                next.as_str()
            ))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: Uuid,
    pub source_account_id: Uuid,
    pub article_link_id: Option<Uuid>,
    pub original_url: String,
    pub canonical_url: String,
    pub original_title: String,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub current_revision: i32,
    pub content_md: String,
    pub content_hash: String,
    pub asset_manifest_hash: Option<String>,
    pub status: ArticleStatus,
    pub withdrawn_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleAsset {
    pub id: Uuid,
    pub article_id: Uuid,
    pub original_url: Option<String>,
    pub storage_key: String,
    pub asset_hash: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub byte_size: Option<i64>,
    pub mime_type: Option<String>,
    pub asset_kind: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleImageDecision {
    pub id: Uuid,
    pub article_id: Uuid,
    pub candidate_url: String,
    pub decision: String,
    pub measured_width: Option<i32>,
    pub measured_height: Option<i32>,
    pub reason_json: serde_json::Value,
    pub asset_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorKind {
    Crawler,
    Ocr,
    Administrator,
}

impl EditorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Crawler => "crawler",
            Self::Ocr => "ocr",
            Self::Administrator => "administrator",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleRevision {
    pub id: Uuid,
    pub article_id: Uuid,
    pub revision: i32,
    pub editor_kind: EditorKind,
    pub content_md: String,
    pub content_hash: String,
    pub change_reason: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrBlockStatus {
    Pending,
    Succeeded,
    Failed,
    ManualOverride,
}

impl OcrBlockStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ManualOverride => "manual_override",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleOcrBlock {
    pub id: Uuid,
    pub article_id: Uuid,
    pub asset_id: Uuid,
    pub block_anchor: String,
    pub recognized_text: Option<String>,
    pub confidence: Option<f64>,
    pub status: OcrBlockStatus,
    pub provider_ref: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub provider_version: Option<String>,
    pub ocr_version: Option<String>,
    pub provider_call_id: Option<String>,
    pub error_code: Option<String>,
    pub attempt: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    ArticleCreated,
    ArticleUpdated,
    ArticleRefetched,
    ArticleEdited,
    ArticleAssetsChanged,
    ArticleWithdrawn,
}

impl ChangeType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ArticleCreated => "article_created",
            Self::ArticleUpdated => "article_updated",
            Self::ArticleRefetched => "article_refetched",
            Self::ArticleEdited => "article_edited",
            Self::ArticleAssetsChanged => "article_assets_changed",
            Self::ArticleWithdrawn => "article_withdrawn",
        }
    }
}

pub const CHANGE_SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleChangeEvent {
    pub id: Uuid,
    pub event_id: Uuid,
    pub change_sequence: i64,
    pub article_id: Uuid,
    pub article_revision: i32,
    pub change_type: ChangeType,
    pub schema_version: i32,
    pub payload_json: serde_json::Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxStatus {
    Pending,
    Delivering,
    Delivered,
    Failed,
    DeadLetter,
}

impl OutboxStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivering => "delivering",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::DeadLetter => "dead_letter",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryOutbox {
    pub id: Uuid,
    pub event_id: Uuid,
    pub change_sequence: i64,
    pub event_type: ChangeType,
    pub article_id: Uuid,
    pub article_revision: i32,
    pub payload_schema_version: i32,
    pub payload_json: serde_json::Value,
    pub status: OutboxStatus,
    pub attempt_count: i32,
    pub next_attempt_at: Option<DateTime<Utc>>,
    pub last_error_code: Option<String>,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
}
