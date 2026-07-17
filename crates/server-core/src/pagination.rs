//! 仅支持 cursor 分页；无效 cursor 返回 `cursor.invalid`。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// 分页信息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PageInfo {
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// 标准列表页响应。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CursorPage<T> {
    pub items: Vec<T>,
    pub page: PageInfo,
}

impl<T> CursorPage<T> {
    pub fn new(items: Vec<T>, next_cursor: Option<String>, has_more: bool) -> Self {
        Self {
            items,
            page: PageInfo {
                next_cursor,
                has_more,
            },
        }
    }

    pub fn empty() -> Self {
        Self::new(Vec::new(), None, false)
    }
}

/// 将结构化 cursor 编码为 URL-safe base64 JSON。
pub fn encode_cursor<T: Serialize>(value: &T) -> Result<String, AppError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|e| AppError::new("cursor.encode_failed", format!("cursor 编码失败: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// 解码 cursor；失败时返回 `cursor.invalid`（不得静默回第一页）。
pub fn decode_cursor<T: DeserializeOwned>(raw: &str) -> Result<T, AppError> {
    if raw.is_empty() {
        return Err(AppError::cursor_invalid("cursor 为空"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(raw.as_bytes())
        .map_err(|_| AppError::cursor_invalid("cursor 不是合法的 base64"))?;
    serde_json::from_slice(&bytes).map_err(|_| AppError::cursor_invalid("cursor JSON 无效"))
}

/// 编码 trait 便捷别名。
pub trait EncodeCursor: Serialize + Sized {
    fn encode_cursor(&self) -> Result<String, AppError> {
        encode_cursor(self)
    }
}

impl<T: Serialize> EncodeCursor for T {}

/// 解码 trait 便捷别名。
pub trait DecodeCursor: DeserializeOwned + Sized {
    fn decode_cursor(raw: &str) -> Result<Self, AppError> {
        decode_cursor(raw)
    }
}

impl<T: DeserializeOwned> DecodeCursor for T {}

/// 时间流常用复合 cursor：排序键 + 稳定 tie-breaker。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompositeCursor<K, I> {
    pub key: K,
    pub id: I,
}

impl<K, I> CompositeCursor<K, I> {
    pub fn new(key: K, id: I) -> Self {
        Self { key, id }
    }
}

/// 列表查询参数（limit 带默认与上限）。
#[derive(Debug, Clone)]
pub struct ListParams {
    pub limit: u32,
    pub cursor: Option<String>,
}

impl ListParams {
    pub fn new(limit: Option<u32>, cursor: Option<String>, default: u32, max: u32) -> Self {
        let limit = limit.unwrap_or(default).clamp(1, max);
        Self { limit, cursor }
    }

    pub fn decode_cursor<T: DeserializeOwned>(&self) -> Result<Option<T>, AppError> {
        match &self.cursor {
            None => Ok(None),
            Some(raw) => Ok(Some(decode_cursor(raw)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct ArticleCursor {
        fetched_at: String,
        id: String,
    }

    #[test]
    fn invalid_cursor_returns_cursor_invalid() {
        let err = decode_cursor::<ArticleCursor>("%%%not-base64%%%").unwrap_err();
        assert_eq!(err.code, "cursor.invalid");
    }

    #[test]
    fn empty_page_shape() {
        let page: CursorPage<String> = CursorPage::empty();
        let value = serde_json::to_value(&page).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "items": [],
                "page": { "next_cursor": null, "has_more": false }
            })
        );
    }

    #[test]
    fn composite_cursor_roundtrip_is_stable() {
        let c = CompositeCursor::new("2026-07-09T10:00:00Z".to_string(), "art-42".to_string());
        let encoded = encode_cursor(&c).unwrap();
        let decoded: CompositeCursor<String, String> = decode_cursor(&encoded).unwrap();
        assert_eq!(decoded, c);

        // 同一内容再次编码应得到相同字符串（稳定排序依赖稳定编码）。
        let encoded2 = encode_cursor(&c).unwrap();
        assert_eq!(encoded, encoded2);
    }

    #[test]
    fn malformed_json_cursor_is_invalid() {
        let raw = URL_SAFE_NO_PAD.encode(b"{\"oops\":true}");
        let err = decode_cursor::<ArticleCursor>(&raw).unwrap_err();
        assert_eq!(err.code, "cursor.invalid");
    }

    #[test]
    fn list_params_clamp_limit() {
        let p = ListParams::new(Some(9999), None, 20, 100);
        assert_eq!(p.limit, 100);
        assert!(p.decode_cursor::<ArticleCursor>().unwrap().is_none());
    }
}
