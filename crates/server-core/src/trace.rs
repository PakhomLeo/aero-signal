//! 请求 trace id。

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use uuid::Uuid;

/// 对外响应/日志使用的 trace id 头。
pub const TRACE_ID_HEADER: &str = "x-trace-id";

/// 生成新的 trace id（UUID v4）。
pub fn new_trace_id() -> String {
    Uuid::new_v4().to_string()
}

/// Trace id 新类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceId(pub String);

impl TraceId {
    pub fn new() -> Self {
        Self(new_trace_id())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// 从请求头读取或生成 trace id，并写入响应头与扩展。
pub async fn inject_trace_id(mut request: Request, next: Next) -> Response {
    let incoming = request
        .headers()
        .get(TRACE_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(new_trace_id);

    request.extensions_mut().insert(TraceId(incoming.clone()));

    let mut response = next.run(request).await;
    if let Ok(value) = http::HeaderValue::from_str(&incoming) {
        response.headers_mut().insert(TRACE_ID_HEADER, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trace_id_is_non_empty_uuid_like() {
        let id = new_trace_id();
        assert_eq!(id.len(), 36);
        assert!(id.contains('-'));
    }
}
