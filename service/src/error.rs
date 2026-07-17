//! 统一错误类型：re-export server-core。

pub use server_core::{AppError, ErrorBody, ErrorResponse};

pub type AppResult<T> = Result<T, AppError>;
