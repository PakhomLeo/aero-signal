//! Article capture 的输入转换、稳定标识与取消检查。

use platform_connectors::ArticleCandidate;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{FetchArticleDetailPayload, FetchSourcePayload};

pub(crate) fn candidate_to_detail_payload(
    source: &FetchSourcePayload,
    c: &ArticleCandidate,
) -> FetchArticleDetailPayload {
    let (biz, mid, idx, sn) = parse_wechat_from_url(&c.canonical_url);
    FetchArticleDetailPayload {
        source_account_id: source.source_account_id,
        collector_account_id: Some(source.collector_account_id),
        canonical_url: c.canonical_url.clone(),
        wechat_biz: biz,
        wechat_mid: mid,
        wechat_idx: idx,
        wechat_sn: sn,
        title_hint: Some(c.title.clone()),
        candidate_published_at: c.published_at,
        candidate_author: c.author.clone(),
        candidate_cover_url: c.cover_url.clone(),
        candidate_evidence: c.evidence.clone(),
        source_heartbeat_event_id: Some(source.heartbeat_event_id.clone()),
        heartbeat_manual: source.manual,
        heartbeat_recalculate_next: source.recalculate_next,
        discovered_via: format!("fetch_source:{}", source.heartbeat_event_id),
    }
}

pub(crate) fn parse_wechat_from_url(
    url: &str,
) -> (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
) {
    let Ok(parsed) = url::Url::parse(url) else {
        return (None, None, None, None);
    };
    let mut biz = None;
    let mut mid = None;
    let mut idx = None;
    let mut sn = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "__biz" => biz = Some(v.into_owned()),
            "mid" => mid = Some(v.into_owned()),
            "idx" => idx = Some(v.into_owned()),
            "sn" => sn = Some(v.into_owned()),
            _ => {}
        }
    }
    (biz, mid, idx, sn)
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

pub(crate) fn stable_uuid(value: &str) -> Uuid {
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub(crate) fn ensure_not_cancelled(
    cancel_requested: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    if cancel_requested.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst)) {
        Err("job.cancelled: 抓取任务已取消".into())
    } else {
        Ok(())
    }
}
