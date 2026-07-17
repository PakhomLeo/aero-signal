//! Signal 预定义 job kind 与幂等键约定（BACKEND_MODULES §10）。

pub const KIND_DAILY_PREFLIGHT: &str = "daily_preflight";
pub const KIND_BUILD_DAILY_PLAN: &str = "build_daily_plan";
pub const KIND_COLLECTOR_PROBE: &str = "collector_probe";
pub const KIND_SOURCE_VALIDATION: &str = "source_validation";
pub const KIND_SOURCE_VALIDATION_AGGREGATE: &str = "source_validation_aggregate";
pub const KIND_FETCH_SOURCE: &str = "fetch_source";
pub const KIND_HISTORY_BACKFILL: &str = "history_backfill";
pub const KIND_FETCH_ARTICLE_DETAIL: &str = "fetch_article_detail";
pub const KIND_RETRY_ARTICLE_ASSETS: &str = "retry_article_assets";
pub const KIND_OCR_ARTICLE_IMAGE: &str = "ocr_article_image";
pub const KIND_REFETCH_ARTICLE: &str = "refetch_article";
pub const KIND_RETENTION_CLEANUP: &str = "retention_cleanup";

/// Signal 必须注册的全部 kind 名称。
pub const SIGNAL_JOB_KINDS: &[&str] = &[
    KIND_DAILY_PREFLIGHT,
    KIND_BUILD_DAILY_PLAN,
    KIND_COLLECTOR_PROBE,
    KIND_SOURCE_VALIDATION,
    KIND_SOURCE_VALIDATION_AGGREGATE,
    KIND_FETCH_SOURCE,
    KIND_HISTORY_BACKFILL,
    KIND_FETCH_ARTICLE_DETAIL,
    KIND_RETRY_ARTICLE_ASSETS,
    KIND_OCR_ARTICLE_IMAGE,
    KIND_REFETCH_ARTICLE,
    KIND_RETENTION_CLEANUP,
];

/// 幂等键构造辅助（与 BACKEND_MODULES §10 一致）。
pub mod idempotency {
    pub fn preflight(date: &str) -> String {
        format!("preflight:{date}")
    }

    pub fn daily_plan(date: &str) -> String {
        format!("daily-plan:{date}")
    }

    pub fn collector_probe(collector: &str, date: &str) -> String {
        format!("collector-probe:{collector}:{date}")
    }

    pub fn source_validate(url_hash: &str) -> String {
        format!("source-validate:{url_hash}")
    }

    pub fn source_fetch(source: &str, heartbeat_event: &str) -> String {
        format!("source-fetch:{source}:{heartbeat_event}")
    }

    pub fn detail(article_identity: &str) -> String {
        format!("detail:{article_identity}")
    }

    pub fn history_backfill(run_id: &str, cursor: u32) -> String {
        format!("history-backfill:{run_id}:{cursor}")
    }

    pub fn asset_retry(article_identity: &str, failure_set_hash: &str) -> String {
        format!("asset-retry:{article_identity}:{failure_set_hash}")
    }

    pub fn ocr(article_id: &str, asset_id: &str, asset_hash: &str, ocr_version: &str) -> String {
        format!("ocr:{article_id}:{asset_id}:{asset_hash}:{ocr_version}")
    }

    pub fn ocr_rerun(
        article_id: &str,
        asset_id: &str,
        asset_hash: &str,
        ocr_version: &str,
        run_id: &str,
    ) -> String {
        format!("ocr-rerun:{article_id}:{asset_id}:{asset_hash}:{ocr_version}:{run_id}")
    }

    pub fn refetch(article: &str, requested_at: &str) -> String {
        format!("refetch:{article}:{requested_at}")
    }

    pub fn retention_cleanup(slot: &str) -> String {
        format!("retention-cleanup:{slot}")
    }
}
