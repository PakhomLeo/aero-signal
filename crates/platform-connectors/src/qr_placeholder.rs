//! 骨架阶段共用的可解码 PNG data URL。
//!
//! 真实协议接入前，connector 用此占位图验证管理端 `<img>` 能流畅展示。
//! 禁止把第三方原始二维码 URL 下发给前端。

/// 168×168 PNG（类 QR finder 图案），base64 无换行。
pub const PLACEHOLDER_QR_PNG_B64: &str = concat!(
    "iVBORw0KGgoAAAANSUhEUgAAAKgAAACoCAIAAAD7KTLjAAACGklEQVR4nO3dMY7CQBAAQfvE/7/sCy4G",
    "ySes9birYgK0rQmWMbBvQxzHsU2w7/s2wc/qN8AawkcJHyV8lPBRwkcJH/W62715yj34rLudp4mPEj5K",
    "+Cjho4SPEj5K+Ki39/ir79lT9utXW3WeJj5K+Cjho4SPEj5K+Cjho4SPEj5K+Cjho4SPEj5K+Cjho4SP",
    "Ej5K+Cjho4SPEj5K+Cjho4SPEj5K+Cjho04/V+95+O9adZ4mPkr4KOGjhI8SPkr4KOGj9un33XffL7/b",
    "6+/GxEcJHyV8lPBRwkcJHyV81G6/3mTio4SPEj5K+Cjho4SPEj7q9HP1V7vb/vt46O/qm/go4aOEjxI+",
    "Svgo4aOEjxI+6nW3e/PV/yPHHxMfJXyU8FHCRwkfJXyU8FGvVfds+/W1THyU8FHCRwkfJXyU8FHCRwkf",
    "JXyU8FHCRwkfJXyU8FHCRwkfJXyU8FHCRwkfJXyU8FHCR51+rt7z8M9g4qOEjxI+Svgo4aOEjxI+asyP",
    "yvn84LtMfJTwUcJHCR8lfJTwUcJvTb/fd1hAHClnhgAAAABJRU5ErkJggg==",
);

/// 返回 `data:image/png;base64,...`，供管理端直接展示。
pub fn placeholder_qr_data_url() -> String {
    format!("data:image/png;base64,{PLACEHOLDER_QR_PNG_B64}")
}
