//! 数据保留策略领域模型（M15）。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 与 `retention_policies.target_kind` CHECK 一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionTargetKind {
    RawHtml,
    Screenshot,
    BodyImage,
    OcrText,
    ApiCallLog,
    AuditLog,
    ChangeEvent,
    DeliveryOutbox,
    FailedValidationAsset,
}

impl RetentionTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RawHtml => "raw_html",
            Self::Screenshot => "screenshot",
            Self::BodyImage => "body_image",
            Self::OcrText => "ocr_text",
            Self::ApiCallLog => "api_call_log",
            Self::AuditLog => "audit_log",
            Self::ChangeEvent => "change_event",
            Self::DeliveryOutbox => "delivery_outbox",
            Self::FailedValidationAsset => "failed_validation_asset",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "raw_html" => Some(Self::RawHtml),
            "screenshot" => Some(Self::Screenshot),
            "body_image" => Some(Self::BodyImage),
            "ocr_text" => Some(Self::OcrText),
            "api_call_log" => Some(Self::ApiCallLog),
            "audit_log" => Some(Self::AuditLog),
            "change_event" => Some(Self::ChangeEvent),
            "delivery_outbox" => Some(Self::DeliveryOutbox),
            "failed_validation_asset" => Some(Self::FailedValidationAsset),
            _ => None,
        }
    }

    pub const ALL: &'static [Self] = &[
        Self::RawHtml,
        Self::Screenshot,
        Self::BodyImage,
        Self::OcrText,
        Self::ApiCallLog,
        Self::AuditLog,
        Self::ChangeEvent,
        Self::DeliveryOutbox,
        Self::FailedValidationAsset,
    ];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub id: Uuid,
    pub target_kind: RetentionTargetKind,
    pub retention_days: i32,
    pub hard_delete_after_days: Option<i32>,
    pub enabled: bool,
    pub updated_at: DateTime<Utc>,
}

/// 候选删除项（清理 job 扫描结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetentionCandidate {
    pub target_kind: RetentionTargetKind,
    pub record_id: Uuid,
    /// 若为资产类，关联文章；日志类可为 None。
    pub article_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

/// 引用检查结果：仍被当前 revision / 未投递 outbox 等引用则不可删。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetentionDeleteDecision {
    Allowed,
    Blocked { reason: &'static str },
}

impl RetentionDeleteDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }
}
