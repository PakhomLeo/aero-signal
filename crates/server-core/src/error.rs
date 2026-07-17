//! 统一应用错误与 JSON 错误响应。

use std::collections::BTreeMap;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 标准错误响应外壳。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

/// 错误体：`code` 使用 `module.reason` 点分英文。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub details: BTreeMap<String, Value>,
    pub trace_id: String,
}

/// 应用错误。
#[derive(Debug, Clone)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub details: BTreeMap<String, Value>,
    pub status: StatusCode,
    pub trace_id: Option<String>,
}

impl AppError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: BTreeMap::new(),
            status: StatusCode::BAD_REQUEST,
            trace_id: None,
        }
    }

    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = status;
        self
    }

    pub fn with_detail(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.details.insert(key.into(), value.into());
        self
    }

    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    pub fn not_found(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message).with_status(StatusCode::NOT_FOUND)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new("auth.unauthorized", message).with_status(StatusCode::UNAUTHORIZED)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new("auth.forbidden", message).with_status(StatusCode::FORBIDDEN)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("server.internal", message).with_status(StatusCode::INTERNAL_SERVER_ERROR)
    }

    pub fn cursor_invalid(message: impl Into<String>) -> Self {
        Self::new("cursor.invalid", message).with_status(StatusCode::BAD_REQUEST)
    }

    /// 转为标准 JSON 外壳。
    pub fn into_response_body(self) -> ErrorResponse {
        ErrorResponse {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                details: self.details,
                trace_id: self.trace_id.unwrap_or_default(),
            },
        }
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = self.into_response_body();
        (status, Json(body)).into_response()
    }
}

/// 便于手工构造与快照断言的辅助。
pub fn error_json(
    code: &str,
    message: &str,
    details: BTreeMap<String, Value>,
    trace_id: &str,
) -> Value {
    json!({
        "error": {
            "code": code,
            "message": message,
            "details": details,
            "trace_id": trace_id,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_response_serializes_to_standard_shape() {
        let err = AppError::new("source.not_found", "Source account not found")
            .with_status(StatusCode::NOT_FOUND)
            .with_detail("source_id", "abc")
            .with_trace_id("trace-001");

        let body = err.into_response_body();
        let value = serde_json::to_value(&body).expect("serialize");

        assert_eq!(
            value,
            json!({
                "error": {
                    "code": "source.not_found",
                    "message": "Source account not found",
                    "details": { "source_id": "abc" },
                    "trace_id": "trace-001"
                }
            })
        );
    }

    #[test]
    fn empty_details_serialize_as_object() {
        let err = AppError::cursor_invalid("bad cursor").with_trace_id("t-2");
        let value = serde_json::to_value(err.into_response_body()).unwrap();
        assert_eq!(value["error"]["details"], json!({}));
        assert_eq!(value["error"]["code"], "cursor.invalid");
    }
}
