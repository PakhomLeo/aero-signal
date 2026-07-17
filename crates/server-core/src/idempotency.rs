//! Idempotency 中间件骨架：读 `Idempotency-Key`，按 body hash 缓存响应占位。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::error::AppError;

/// 标准幂等键请求头。
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// 幂等键。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdempotencyKey(pub String);

impl IdempotencyKey {
    pub fn parse(headers: &HeaderMap) -> Option<Self> {
        headers
            .get(IDEMPOTENCY_KEY_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| Self(s.to_string()))
    }
}

/// 缓存的幂等响应占位。
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub body_hash: String,
    pub status: u16,
    pub body: Vec<u8>,
    pub content_type: Option<String>,
}

/// 幂等存储接口（M02 可换 Redis/PG）。
pub trait IdempotencyStore: Send + Sync + 'static {
    fn get(&self, key: &IdempotencyKey) -> Option<IdempotencyRecord>;
    fn put(&self, key: IdempotencyKey, record: IdempotencyRecord);
}

/// 进程内内存实现。
#[derive(Debug, Default, Clone)]
pub struct InMemoryIdempotencyStore {
    inner: Arc<Mutex<HashMap<IdempotencyKey, IdempotencyRecord>>>,
}

impl InMemoryIdempotencyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdempotencyStore for InMemoryIdempotencyStore {
    fn get(&self, key: &IdempotencyKey) -> Option<IdempotencyRecord> {
        self.inner.lock().ok()?.get(key).cloned()
    }

    fn put(&self, key: IdempotencyKey, record: IdempotencyRecord) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(key, record);
        }
    }
}

/// 计算请求体 SHA-256 hex。
pub fn body_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// 同 key 同 body hash：返回缓存响应；同 key 不同 hash：冲突错误。
pub fn replay_or_conflict(
    store: &dyn IdempotencyStore,
    key: &IdempotencyKey,
    request_body_hash: &str,
) -> Result<Option<Response>, AppError> {
    match store.get(key) {
        None => Ok(None),
        Some(record) if record.body_hash == request_body_hash => {
            let mut response = Response::builder()
                .status(StatusCode::from_u16(record.status).unwrap_or(StatusCode::OK))
                .body(Body::from(record.body))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            if let Some(ct) = record.content_type
                && let Ok(v) = HeaderValue::from_str(&ct)
            {
                response
                    .headers_mut()
                    .insert(axum::http::header::CONTENT_TYPE, v);
            }
            Ok(Some(response))
        }
        Some(_) => Err(AppError::new(
            "idempotency.key_reuse_mismatch",
            "同一 Idempotency-Key 不能用于不同请求体",
        )
        .with_status(StatusCode::CONFLICT)),
    }
}

/// 将成功响应写入幂等存储（骨架：调用方在 handler 后调用）。
pub fn remember_response(
    store: &dyn IdempotencyStore,
    key: IdempotencyKey,
    request_body_hash: String,
    status: StatusCode,
    body: Vec<u8>,
    content_type: Option<String>,
) {
    store.put(
        key,
        IdempotencyRecord {
            body_hash: request_body_hash,
            status: status.as_u16(),
            body,
            content_type,
        },
    );
}

/// Axum 中间件骨架：有 key 时查缓存；命中则短路返回。
///
/// 注意：完整缓冲请求体并回写需在产品层按路由启用；此处提供可测的核心逻辑。
pub async fn idempotency_lookup_middleware<S>(
    store: axum::extract::State<Arc<S>>,
    request: Request,
    next: Next,
) -> Result<Response, AppError>
where
    S: IdempotencyStore,
{
    let key = match IdempotencyKey::parse(request.headers()) {
        Some(k) => k,
        None => return Ok(next.run(request).await),
    };

    // 骨架：无 body 缓冲时用空 hash 占位；真实写接口应先读 body。
    let hash = body_hash(b"");
    if let Some(cached) = replay_or_conflict(store.as_ref(), &key, &hash)? {
        return Ok(cached);
    }

    Ok(next.run(request).await)
}

/// 便于测试的「查缓存或执行」辅助。
pub fn cached_json_or_compute<T, F, S>(
    store: &S,
    key: &IdempotencyKey,
    body_bytes: &[u8],
    compute: F,
) -> Result<(StatusCode, Vec<u8>), AppError>
where
    T: serde::Serialize,
    F: FnOnce() -> Result<(StatusCode, T), AppError>,
    S: IdempotencyStore,
{
    let hash = body_hash(body_bytes);
    if let Some(record) = store.get(key) {
        if record.body_hash == hash {
            let status = StatusCode::from_u16(record.status).unwrap_or(StatusCode::OK);
            return Ok((status, record.body));
        }
        return Err(AppError::new(
            "idempotency.key_reuse_mismatch",
            "同一 Idempotency-Key 不能用于不同请求体",
        )
        .with_status(StatusCode::CONFLICT));
    }

    let (status, value) = compute()?;
    let body =
        serde_json::to_vec(&value).map_err(|e| AppError::internal(format!("序列化失败: {e}")))?;
    remember_response(
        store,
        key.clone(),
        hash,
        status,
        body.clone(),
        Some("application/json".to_string()),
    );
    Ok((status, body))
}

/// 将缓存字节包装为 JSON 响应（测试辅助）。
pub fn json_bytes_response(status: StatusCode, body: Vec<u8>) -> Response {
    (
        status,
        [(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn same_key_same_body_hash_returns_cached_placeholder() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey("k-1".into());
        let body = br#"{"name":"demo"}"#;

        let (status1, bytes1) = cached_json_or_compute(&store, &key, body, || {
            Ok((StatusCode::CREATED, json!({"id": "job-1", "cached": false})))
        })
        .unwrap();
        assert_eq!(status1, StatusCode::CREATED);

        let (status2, bytes2) = cached_json_or_compute(
            &store,
            &key,
            body,
            || -> Result<(StatusCode, serde_json::Value), AppError> {
                panic!("不应再次执行业务逻辑");
            },
        )
        .unwrap();

        assert_eq!(status2, StatusCode::CREATED);
        assert_eq!(bytes1, bytes2);
        let value: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
        assert_eq!(value["id"], "job-1");
    }

    #[test]
    fn same_key_different_body_conflicts() {
        let store = InMemoryIdempotencyStore::new();
        let key = IdempotencyKey("k-2".into());

        cached_json_or_compute(&store, &key, b"a", || {
            Ok((StatusCode::OK, json!({"ok": true})))
        })
        .unwrap();

        let err = cached_json_or_compute(&store, &key, b"b", || {
            Ok((StatusCode::OK, json!({"ok": false})))
        })
        .unwrap_err();
        assert_eq!(err.code, "idempotency.key_reuse_mismatch");
    }

    #[test]
    fn parse_idempotency_key_header() {
        let mut headers = HeaderMap::new();
        headers.insert(IDEMPOTENCY_KEY_HEADER, HeaderValue::from_static(" abc "));
        assert_eq!(
            IdempotencyKey::parse(&headers),
            Some(IdempotencyKey("abc".into()))
        );
    }
}
