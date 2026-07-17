//! Aero 服务端统一底座：配置、错误、分页、时钟、观测、健康检查与幂等骨架。

pub mod app_state;
pub mod auth_extractors;
pub mod clock;
pub mod config;
pub mod error;
pub mod health;
pub mod idempotency;
pub mod openapi;
pub mod pagination;
pub mod trace;

pub use app_state::AppState;
pub use clock::{Clock, FrozenClock, SystemClock};
pub use config::{ServiceConfig, SignalEnvironment, StoreMode};
pub use error::{AppError, ErrorBody, ErrorResponse};
pub use health::{HealthStatus, healthz_router};
pub use idempotency::{
    IdempotencyKey, IdempotencyRecord, IdempotencyStore, InMemoryIdempotencyStore, body_hash,
};
pub use pagination::{CursorPage, DecodeCursor, EncodeCursor, PageInfo};
pub use trace::{TRACE_ID_HEADER, TraceId, inject_trace_id, new_trace_id};
