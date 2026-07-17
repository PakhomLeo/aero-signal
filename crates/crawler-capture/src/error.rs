//! 抓取错误（`module.reason`）。

use thiserror::Error;

pub type CrawlerResult<T> = Result<T, CrawlerError>;

#[derive(Debug, Clone, Error)]
#[error("{code}: {message}")]
pub struct CrawlerError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl CrawlerError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable,
        }
    }

    pub fn blocked_by_protection(msg: impl Into<String>) -> Self {
        Self::new("crawler.blocked_by_protection", msg, true)
    }

    pub fn fetch_failed(msg: impl Into<String>) -> Self {
        Self::new("crawler.fetch_failed", msg, true)
    }

    pub fn parse_failed(msg: impl Into<String>) -> Self {
        Self::new("crawler.parse_failed", msg, false)
    }

    pub fn empty_article(msg: impl Into<String>) -> Self {
        Self::new("crawler.empty_article", msg, false)
    }

    pub fn image_filtered(msg: impl Into<String>) -> Self {
        Self::new("image.filtered", msg, false)
    }

    pub fn store_failed(msg: impl Into<String>) -> Self {
        Self::new("crawler.store_failed", msg, true)
    }
}
