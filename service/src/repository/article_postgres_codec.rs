//! PostgreSQL 文章仓储的枚举编解码。

use crate::domain::article::{ArticleStatus, ChangeType, EditorKind, OcrBlockStatus, OutboxStatus};

pub(crate) fn parse_article_status(s: &str) -> Result<ArticleStatus, String> {
    ArticleStatus::parse(s).ok_or_else(|| format!("unknown article status: {s}"))
}

pub(crate) fn parse_editor_kind(s: &str) -> Result<EditorKind, String> {
    match s {
        "crawler" => Ok(EditorKind::Crawler),
        "ocr" => Ok(EditorKind::Ocr),
        "administrator" => Ok(EditorKind::Administrator),
        other => Err(format!("unknown editor_kind: {other}")),
    }
}

pub(crate) fn parse_ocr_status(s: &str) -> Result<OcrBlockStatus, String> {
    match s {
        "pending" => Ok(OcrBlockStatus::Pending),
        "succeeded" => Ok(OcrBlockStatus::Succeeded),
        "failed" => Ok(OcrBlockStatus::Failed),
        "manual_override" => Ok(OcrBlockStatus::ManualOverride),
        other => Err(format!("unknown ocr status: {other}")),
    }
}

pub(crate) fn parse_change_type(s: &str) -> Result<ChangeType, String> {
    match s {
        "article_created" => Ok(ChangeType::ArticleCreated),
        "article_updated" => Ok(ChangeType::ArticleUpdated),
        "article_refetched" => Ok(ChangeType::ArticleRefetched),
        "article_edited" => Ok(ChangeType::ArticleEdited),
        "article_assets_changed" => Ok(ChangeType::ArticleAssetsChanged),
        "article_withdrawn" => Ok(ChangeType::ArticleWithdrawn),
        other => Err(format!("unknown change_type: {other}")),
    }
}

pub(crate) fn parse_outbox_status(s: &str) -> Result<OutboxStatus, String> {
    match s {
        "pending" => Ok(OutboxStatus::Pending),
        "delivering" => Ok(OutboxStatus::Delivering),
        "delivered" => Ok(OutboxStatus::Delivered),
        "failed" => Ok(OutboxStatus::Failed),
        "dead_letter" => Ok(OutboxStatus::DeadLetter),
        other => Err(format!("unknown outbox status: {other}")),
    }
}
